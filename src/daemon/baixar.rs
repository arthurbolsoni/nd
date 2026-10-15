//! Arquivos: downloads do navegador (sempre ligados, gravados em `~/.nd/downloads`), `nd download`
//! (clique que gera arquivo, URL baixada com a sessão da aba, lista) e `nd pdf`.
//!
//! `nd download <alvo>` cobre os jeitos de um site entregar arquivo: download comum (anexo ou
//! `a[download]`), resposta que abre na mesma aba ou numa aba nova (PDF por GET ou POST, pego pelo
//! `Fetch` no alvo do navegador) e blob criado pela página (aberto numa aba ou não).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nodriver::{Connection, Tab};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Daemon;
use super::acoes::{Alvo, clicar};
use super::comandos::aba_atual;
use super::net::{base64_decodificar, tamanho};
use super::pagina::{self, js};
use crate::estado;

type R = Result<String, String>;

fn e(x: impl std::fmt::Display) -> String {
    x.to_string()
}

/// Espera por downloads em andamento no fim de uma ação comum.
pub const ESPERA_DOWNLOAD: Duration = Duration::from_secs(15);
/// Blob criado pela página sem download nem aba: depois deste tempo, ele é o arquivo.
const BLOB_SOZINHO: Duration = Duration::from_secs(2);
/// Pedaço de blob lido por vez (em bytes, antes do base64).
const PEDACO_BLOB: u64 = 4 * 1024 * 1024;

pub fn pasta_padrao() -> PathBuf {
    estado::dir().join("downloads")
}

#[derive(Debug, Clone, PartialEq)]
pub enum Situacao {
    Andamento,
    Concluido,
    Cancelado,
}

/// Download feito pelo navegador.
#[derive(Debug, Clone)]
pub struct Baixado {
    guid: String,
    url: String,
    nome: String,
    inicio: Instant,
    situacao: Situacao,
    recebidos: u64,
    total: u64,
    caminho: Option<PathBuf>,
}

/// Arquivo gravado por `nd download`, `nd pdf` ou pelo navegador.
#[derive(Debug, Clone)]
pub struct Arquivo {
    /// Como chegou: download, resposta, aba nova, blob, url, pdf.
    origem: String,
    caminho: PathBuf,
    bytes: u64,
    tipo: String,
}

impl Arquivo {
    fn linha(&self) -> String {
        let tipo = if self.tipo.is_empty() {
            String::new()
        } else {
            format!(", {}", self.tipo)
        };
        format!(
            "arquivo ({}): {} ({}{tipo})",
            self.origem,
            self.caminho.display(),
            tamanho(self.bytes)
        )
    }
}

#[derive(Default)]
pub struct Downloads {
    lista: Mutex<Vec<Baixado>>,
    gravados: Mutex<Vec<Arquivo>>,
}

impl Downloads {
    /// Atualiza com `Browser.downloadWillBegin` / `Browser.downloadProgress`.
    pub fn evento(&self, metodo: &str, p: &Value) {
        let guid = p["guid"].as_str().unwrap_or("").to_string();
        let mut lista = self.lista.lock().unwrap();
        match metodo {
            "Browser.downloadWillBegin" => lista.push(Baixado {
                guid,
                url: p["url"].as_str().unwrap_or("").into(),
                nome: p["suggestedFilename"].as_str().unwrap_or("").into(),
                inicio: Instant::now(),
                situacao: Situacao::Andamento,
                recebidos: 0,
                total: 0,
                caminho: None,
            }),
            "Browser.downloadProgress" => {
                let Some(b) = lista.iter_mut().find(|b| b.guid == guid) else {
                    return;
                };
                b.recebidos = p["receivedBytes"].as_u64().unwrap_or(b.recebidos);
                b.total = p["totalBytes"].as_u64().unwrap_or(b.total);
                match p["state"].as_str() {
                    Some("completed") => {
                        // allowAndName grava com o guid como nome; o arquivo ganha o nome sugerido.
                        let bruto = p["filePath"]
                            .as_str()
                            .map(PathBuf::from)
                            .unwrap_or_else(|| pasta_padrao().join(&b.guid));
                        let destino = livre(bruto.parent().unwrap_or(&pasta_padrao()), &b.nome);
                        let caminho = match std::fs::rename(&bruto, &destino) {
                            Ok(()) => destino,
                            Err(x) => {
                                tracing::warn!("renomeando {}: {x}", bruto.display());
                                bruto
                            }
                        };
                        b.situacao = Situacao::Concluido;
                        self.gravados.lock().unwrap().push(Arquivo {
                            origem: "download".into(),
                            caminho: caminho.clone(),
                            bytes: b.recebidos,
                            tipo: String::new(),
                        });
                        b.caminho = Some(caminho);
                    }
                    Some("canceled") => b.situacao = Situacao::Cancelado,
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn desde(&self, t0: Instant) -> Vec<Baixado> {
        self.lista
            .lock()
            .unwrap()
            .iter()
            .filter(|b| b.inicio >= t0)
            .cloned()
            .collect()
    }

    fn em_andamento_desde(&self, t0: Instant) -> bool {
        self.desde(t0).iter().any(|b| b.situacao == Situacao::Andamento)
    }

    fn gravar(&self, a: Arquivo) {
        self.gravados.lock().unwrap().push(a);
    }
}

/// Liga os downloads no navegador e acompanha os eventos deles.
pub async fn ligar(d: &Arc<Daemon>) {
    let conn = d.browser.connection().clone();
    let mut eventos = conn.subscribe_raw(Some("Browser."), None);
    configurar(&conn).await;
    let fraco = Arc::downgrade(d);
    tokio::spawn(async move {
        while let Some(ev) = eventos.next().await {
            let Some(d) = fraco.upgrade() else { break };
            if ev.method == "Browser.downloadWillBegin" {
                d.rede.virou_download(ev.params["url"].as_str().unwrap_or(""));
            }
            d.downloads.evento(&ev.method, &ev.params);
        }
    });
}

/// Downloads permitidos (headless e perfil novo bloqueiam), na pasta do nd, com eventos.
pub async fn configurar(conn: &Connection) {
    let pasta = pasta_padrao();
    let _ = std::fs::create_dir_all(&pasta);
    let r = conn
        .call(
            "Browser.setDownloadBehavior",
            json!({ "behavior": "allowAndName", "downloadPath": pasta, "eventsEnabled": true }),
            None,
        )
        .await;
    if let Err(x) = r {
        tracing::warn!("Browser.setDownloadBehavior: {x}");
    }
}

/// Linhas sobre os downloads iniciados desde `t0`, esperando os em andamento até `max`.
pub async fn resumo(d: &Daemon, t0: Instant, max: Duration) -> Vec<String> {
    let fim = Instant::now() + max;
    while d.downloads.em_andamento_desde(t0) && Instant::now() < fim {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    d.downloads
        .desde(t0)
        .iter()
        .map(|b| match (&b.situacao, &b.caminho) {
            (Situacao::Concluido, Some(c)) => {
                format!("download: {} ({})", c.display(), tamanho(b.recebidos))
            }
            (Situacao::Cancelado, _) => format!("download cancelado: {}", b.nome),
            _ => format!(
                "download em andamento: {} ({} de {}); nd download mostra quando terminar",
                b.nome,
                tamanho(b.recebidos),
                if b.total > 0 { tamanho(b.total) } else { "?".into() }
            ),
        })
        .collect()
}

/// Nome de arquivo sem caracteres proibidos no Windows.
fn limpar_nome(nome: &str) -> String {
    let s: String = nome
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim().trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() { "download".into() } else { s }
}

/// Caminho em `pasta` para `nome` que ainda não existe: `nome (2).ext`, `nome (3).ext`...
fn livre(pasta: &Path, nome: &str) -> PathBuf {
    let nome = limpar_nome(nome);
    let p = pasta.join(&nome);
    if !p.exists() {
        return p;
    }
    let (base, ext) = match nome.rsplit_once('.') {
        Some((b, x)) if !b.is_empty() => (b.to_string(), format!(".{x}")),
        _ => (nome.clone(), String::new()),
    };
    (2..)
        .map(|i| pasta.join(format!("{base} ({i}){ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

fn extensao(mime: &str) -> &'static str {
    let m = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match m.as_str() {
        "application/pdf" => ".pdf",
        "application/zip" | "application/x-zip-compressed" => ".zip",
        "application/json" => ".json",
        "text/csv" => ".csv",
        "text/plain" => ".txt",
        "application/xml" | "text/xml" => ".xml",
        "image/png" => ".png",
        "image/jpeg" => ".jpg",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "image/svg+xml" => ".svg",
        "application/vnd.ms-excel" => ".xls",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => ".xlsx",
        "application/msword" => ".doc",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => ".docx",
        "text/html" => ".html",
        _ => ".bin",
    }
}

/// Nome do `Content-Disposition` (`filename*=UTF-8''...` ou `filename="..."`).
fn nome_disposicao(disp: &str) -> Option<String> {
    let mut simples = None;
    for parte in disp.split(';') {
        let parte = parte.trim();
        let Some((k, v)) = parte.split_once('=') else {
            continue;
        };
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim().trim_matches('"');
        if k == "filename*" {
            let v = v.rsplit_once('\'').map_or(v, |(_, x)| x);
            return Some(decodificar_pct(v));
        }
        if k == "filename" && !v.is_empty() {
            simples = Some(v.to_string());
        }
    }
    simples
}

fn decodificar_pct(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(x) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(x);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Nome para um arquivo vindo de `url` com estes cabeçalhos.
fn nome_para(url: &str, mime: &str, disp: Option<&str>) -> String {
    if let Some(n) = disp.and_then(nome_disposicao) {
        return n;
    }
    let caminho = url.split(['?', '#']).next().unwrap_or("");
    let ultimo = caminho.rsplit('/').next().unwrap_or("");
    let ultimo = decodificar_pct(ultimo);
    if ultimo.contains('.') {
        return ultimo;
    }
    let base = if ultimo.is_empty() { "arquivo" } else { &ultimo };
    format!("{base}{}", extensao(mime))
}

/// Resposta que é arquivo (não página): fora de HTML, sem redirecionamento, e sem anexo (esse o
/// navegador baixa sozinho).
fn eh_arquivo(status: i64, mime: &str, disp: Option<&str>) -> bool {
    let m = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    (200..300).contains(&status)
        && !m.is_empty()
        && m != "text/html"
        && m != "application/xhtml+xml"
        && !disp.is_some_and(|d| d.trim().to_ascii_lowercase().starts_with("attachment"))
}

fn gravar(pasta: &Path, nome: &str, dados: &[u8]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(pasta).map_err(|x| format!("{}: {x}", pasta.display()))?;
    let p = livre(pasta, nome);
    std::fs::write(&p, dados).map_err(|x| format!("{}: {x}", p.display()))?;
    Ok(p)
}

// ---------------------------------------------------------------- nd download <alvo>

/// Guarda `URL.createObjectURL` no mundo da página: os blobs criados ficam num Map (lidos mesmo
/// se a página revogar a URL). Devolve quantos já havia.
const GANCHO_BLOB: &str = r#"(() => {
  const k = Symbol.for('nd.blobs');
  if (!window[k]) {
    const m = new Map();
    Object.defineProperty(window, k, { value: m });
    URL.createObjectURL = new Proxy(URL.createObjectURL, {
      apply(t, self, args) {
        const u = Reflect.apply(t, self, args);
        if (args[0] instanceof Blob) m.set(u, args[0]);
        return u;
      },
    });
  }
  return window[k].size;
})()"#;

/// Blobs criados depois do n-ésimo: [{u, tamanho, tipo}].
fn js_blobs_desde(n: u64) -> String {
    format!(
        "[...(window[Symbol.for('nd.blobs')] || new Map()).entries()].slice({n})\
         .map(([u, b]) => ({{ u, tamanho: b.size, tipo: b.type }}))"
    )
}

/// Pedaço [ini, fim) do blob `u` em base64 (do Map, ou por fetch se a página não passou pelo gancho).
fn js_ler_blob(u: &str, ini: u64, fim: u64) -> String {
    let u = serde_json::to_string(u).unwrap();
    format!(
        r#"(async () => {{
  const m = window[Symbol.for('nd.blobs')];
  let b = m && m.get({u});
  if (!b) {{ try {{ b = await (await fetch({u})).blob(); }} catch (e) {{ return {{ erro: String(e) }}; }} }}
  const p = new Uint8Array(await b.slice({ini}, {fim}).arrayBuffer());
  let s = '';
  for (let i = 0; i < p.length; i += 0x8000) s += String.fromCharCode.apply(null, p.subarray(i, i + 0x8000));
  return {{ tamanho: b.size, tipo: b.type, b64: btoa(s) }};
}})()"#
    )
}

async fn ler_blob(tab: &Tab, u: &str) -> Result<(Vec<u8>, String), String> {
    let mut dados = Vec::new();
    loop {
        let ini = dados.len() as u64;
        let v = tab
            .evaluate(&js_ler_blob(u, ini, ini + PEDACO_BLOB))
            .await
            .map_err(e)?;
        if let Some(x) = v["erro"].as_str() {
            return Err(format!("blob {u}: {x}"));
        }
        let parte = base64_decodificar(v["b64"].as_str().unwrap_or("")).ok_or("blob em base64 inválido")?;
        let tipo = v["tipo"].as_str().unwrap_or("").to_string();
        let total = v["tamanho"].as_u64().unwrap_or(0);
        let vazio = parte.is_empty();
        dados.extend(parte);
        if vazio || dados.len() as u64 >= total {
            return Ok((dados, tipo));
        }
    }
}

/// Respostas-arquivo pegas pelo `Fetch` enquanto ele está ligado no alvo do navegador.
struct Captura {
    conn: Connection,
    tarefa: tokio::task::JoinHandle<()>,
}

impl Captura {
    async fn ligar(
        conn: &Connection,
        pasta: PathBuf,
        saida: Arc<Mutex<Vec<Arquivo>>>,
    ) -> Result<Self, String> {
        let mut eventos = conn.subscribe_raw(Some("Fetch.requestPaused"), None);
        let c = conn.clone();
        let tarefa = tokio::spawn(async move {
            while let Some(ev) = eventos.next().await {
                let p = &ev.params;
                let id = p["requestId"].as_str().unwrap_or("").to_string();
                if let Some(a) = capturar(&c, p, &id, &pasta).await {
                    saida.lock().unwrap().push(a);
                }
                let _ = c
                    .call("Fetch.continueRequest", json!({ "requestId": id }), None)
                    .await;
            }
        });
        let r = conn
            .call(
                "Fetch.enable",
                json!({ "patterns": [{ "resourceType": "Document", "requestStage": "Response" }] }),
                None,
            )
            .await;
        if let Err(x) = r {
            tarefa.abort();
            return Err(format!("Fetch.enable: {x}"));
        }
        Ok(Self {
            conn: conn.clone(),
            tarefa,
        })
    }

    async fn desligar(self) {
        let _ = self.conn.call("Fetch.disable", json!({}), None).await;
        self.tarefa.abort();
    }
}

async fn capturar(c: &Connection, p: &Value, id: &str, pasta: &Path) -> Option<Arquivo> {
    let status = p["responseStatusCode"].as_i64().unwrap_or(0);
    let cab = |nome: &str| {
        p["responseHeaders"].as_array().and_then(|l| {
            l.iter()
                .find(|h| h["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(nome)))
                .and_then(|h| h["value"].as_str().map(str::to_string))
        })
    };
    let mime = cab("content-type").unwrap_or_default();
    let disp = cab("content-disposition");
    if !eh_arquivo(status, &mime, disp.as_deref()) {
        return None;
    }
    let url = p["request"]["url"].as_str().unwrap_or("");
    let v = match c
        .call("Fetch.getResponseBody", json!({ "requestId": id }), None)
        .await
    {
        Ok(v) => v,
        Err(x) => {
            tracing::warn!("corpo de {url}: {x}");
            return None;
        }
    };
    let corpo = v["body"].as_str().unwrap_or("");
    let dados = if v["base64Encoded"].as_bool().unwrap_or(false) {
        base64_decodificar(corpo)?
    } else {
        corpo.as_bytes().to_vec()
    };
    let caminho = gravar(pasta, &nome_para(url, &mime, disp.as_deref()), &dados).ok()?;
    let metodo = p["request"]["method"].as_str().unwrap_or("GET");
    Some(Arquivo {
        origem: format!("resposta {metodo} {}", super::pagina::url_curta(url)),
        caminho,
        bytes: dados.len() as u64,
        tipo: mime.split(';').next().unwrap_or("").to_string(),
    })
}

/// Clica no alvo e grava o arquivo que o clique gerar, esperando até `max`.
pub async fn clicar_e_baixar(
    d: &Arc<Daemon>,
    tab: &Tab,
    alvo: &Alvo,
    via_js: bool,
    max: Duration,
    pasta: Option<&str>,
) -> Result<Vec<String>, String> {
    let pasta = pasta.map_or_else(pasta_padrao, PathBuf::from);
    let blobs_antes = tab.evaluate(GANCHO_BLOB).await.map_err(e)?.as_u64().unwrap_or(0);
    let abas_antes: HashSet<String> = d
        .browser
        .targets()
        .await
        .map_err(e)?
        .into_iter()
        .map(|t| t.target_id)
        .collect();
    let respostas = Arc::new(Mutex::new(Vec::new()));
    let captura = Captura::ligar(d.browser.connection(), pasta.clone(), respostas.clone()).await?;
    let t0 = Instant::now();
    let r = async {
        let mut notas = clicar(d, tab, alvo, via_js).await?;
        let arquivos =
            esperar_arquivos(d, tab, t0, max, &pasta, blobs_antes, &abas_antes, &respostas).await?;
        if arquivos.is_empty() {
            return Err(format!(
                "o clique em {} não gerou arquivo em {} s (download, resposta, aba nova ou blob)",
                alvo.r,
                max.as_secs()
            ));
        }
        for a in arquivos {
            notas.push(a.linha());
            d.downloads.gravar(a);
        }
        Ok(notas)
    }
    .await;
    captura.desligar().await;
    r
}

#[allow(clippy::too_many_arguments)]
async fn esperar_arquivos(
    d: &Arc<Daemon>,
    tab: &Tab,
    t0: Instant,
    max: Duration,
    pasta: &Path,
    blobs_antes: u64,
    abas_antes: &HashSet<String>,
    respostas: &Arc<Mutex<Vec<Arquivo>>>,
) -> Result<Vec<Arquivo>, String> {
    let fim = t0 + max;
    let mut lidas: HashSet<String> = HashSet::new();
    let mut de_abas: Vec<Arquivo> = Vec::new();
    let mut blob_visto: Option<Instant> = None;
    let mut pronto_em: Option<Instant> = None;
    loop {
        // Aba nova com blob: lido pelo Map da aba de origem.
        for t in d.browser.targets().await.map_err(e)? {
            if t.r#type == "page"
                && !abas_antes.contains(&t.target_id)
                && t.url.starts_with("blob:")
                && lidas.insert(t.url.clone())
            {
                let (dados, tipo) = ler_blob(tab, &t.url).await?;
                let caminho = gravar(pasta, &format!("blob{}", extensao(&tipo)), &dados)?;
                de_abas.push(Arquivo {
                    origem: "blob em aba nova".into(),
                    caminho,
                    bytes: dados.len() as u64,
                    tipo,
                });
            }
        }
        let baixados = d.downloads.desde(t0);
        let andamento = baixados.iter().any(|b| b.situacao == Situacao::Andamento);
        let tem = !de_abas.is_empty()
            || !respostas.lock().unwrap().is_empty()
            || baixados.iter().any(|b| b.situacao == Situacao::Concluido);
        if tem && !andamento {
            // Um instante para um segundo arquivo do mesmo clique.
            let p = *pronto_em.get_or_insert_with(Instant::now);
            if p.elapsed() >= Duration::from_millis(500) {
                break;
            }
        }
        if !tem && blob_visto.is_none() {
            let v = tab.evaluate(&js_blobs_desde(blobs_antes)).await.map_err(e)?;
            if v.as_array().is_some_and(|a| !a.is_empty()) {
                blob_visto = Some(Instant::now());
            }
        }
        if !tem
            && !andamento
            && let Some(b) = blob_visto
            && b.elapsed() >= BLOB_SOZINHO
        {
            // Blob criado e não aberto nem baixado (mostrado num iframe, por exemplo).
            let v = tab.evaluate(&js_blobs_desde(blobs_antes)).await.map_err(e)?;
            for x in v.as_array().cloned().unwrap_or_default() {
                let u = x["u"].as_str().unwrap_or("");
                let (dados, tipo) = ler_blob(tab, u).await?;
                let caminho = gravar(pasta, &format!("blob{}", extensao(&tipo)), &dados)?;
                de_abas.push(Arquivo {
                    origem: "blob da página".into(),
                    caminho,
                    bytes: dados.len() as u64,
                    tipo,
                });
            }
            break;
        }
        if Instant::now() >= fim {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let mut out: Vec<Arquivo> = Vec::new();
    for b in d.downloads.desde(t0) {
        match (b.situacao, b.caminho) {
            (Situacao::Concluido, Some(c)) => {
                let c = mover(c, pasta)?;
                out.push(Arquivo {
                    origem: "download".into(),
                    bytes: b.recebidos,
                    caminho: c,
                    tipo: String::new(),
                });
            }
            (Situacao::Andamento, _) => {
                return Err(format!(
                    "download de {} ainda em andamento depois de {} s ({} de {}); nd download mostra quando terminar",
                    b.nome,
                    max.as_secs(),
                    tamanho(b.recebidos),
                    if b.total > 0 { tamanho(b.total) } else { "?".into() }
                ));
            }
            _ => {}
        }
    }
    out.extend(respostas.lock().unwrap().drain(..));
    out.extend(de_abas);
    Ok(out)
}

/// Move um download da pasta do nd para `pasta`, se for outra.
fn mover(c: PathBuf, pasta: &Path) -> Result<PathBuf, String> {
    if c.parent() == Some(pasta) {
        return Ok(c);
    }
    std::fs::create_dir_all(pasta).map_err(|x| format!("{}: {x}", pasta.display()))?;
    let nome = c
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let destino = livre(pasta, &nome);
    if std::fs::rename(&c, &destino).is_err() {
        std::fs::copy(&c, &destino).map_err(|x| format!("{}: {x}", destino.display()))?;
        let _ = std::fs::remove_file(&c);
    }
    Ok(destino)
}

// ---------------------------------------------------------------- nd download --url / lista

#[derive(Deserialize)]
pub struct DownloadArgs {
    url: Option<String>,
    pasta: Option<String>,
    saida: Option<String>,
}

/// `nd download --url` (baixa com a sessão da aba) ou, sem URL, a lista dos arquivos da sessão.
pub async fn download(d: &Arc<Daemon>, a: DownloadArgs) -> R {
    let Some(url) = a.url else {
        return Ok(lista(d));
    };
    let tab = aba_atual(d).await?;
    let q = pagina::principal(d, &tab).await?;
    let base = tab.url().await.unwrap_or_default();
    let url = absoluta(&base, &url);
    let r = js(
        d,
        &q,
        "api",
        json!({ "url": url, "metodo": "GET", "cabecalhos": {}, "corpo": null, "max": 0, "base64": true }),
    )
    .await?;
    if let Some(x) = r["erro"].as_str() {
        return Err(format!("fetch falhou: {x}"));
    }
    let status = r["status"].as_i64().unwrap_or(0);
    let tipo = r["tipo"].as_str().unwrap_or("").to_string();
    let dados = base64_decodificar(r["b64"].as_str().unwrap_or("")).ok_or("resposta em base64 inválida")?;
    let caminho = match &a.saida {
        Some(s) => {
            let p = PathBuf::from(s);
            if let Some(pai) = p.parent() {
                let _ = std::fs::create_dir_all(pai);
            }
            std::fs::write(&p, &dados).map_err(|x| format!("{s}: {x}"))?;
            p
        }
        None => {
            let pasta = a.pasta.map_or_else(pasta_padrao, PathBuf::from);
            let final_ = r["url"].as_str().unwrap_or(&url);
            gravar(&pasta, &nome_para(final_, &tipo, r["disp"].as_str()), &dados)?
        }
    };
    let arq = Arquivo {
        origem: format!("url, status {status}"),
        caminho,
        bytes: dados.len() as u64,
        tipo: tipo.split(';').next().unwrap_or("").to_string(),
    };
    let l = arq.linha();
    d.downloads.gravar(arq);
    if !(200..300).contains(&status) {
        return Ok(format!("{l}\naviso: status {status}"));
    }
    Ok(l)
}

/// URL absoluta de `u` vista de `base` (aceita `/caminho` e `caminho`).
fn absoluta(base: &str, u: &str) -> String {
    if u.contains("://") || u.starts_with("data:") || u.starts_with("blob:") {
        return u.to_string();
    }
    let (esquema, resto) = base.split_once("://").unwrap_or(("https", base));
    let host = resto.split(['/', '?', '#']).next().unwrap_or("");
    if let Some(r) = u.strip_prefix("//") {
        return format!("{esquema}://{r}");
    }
    if u.starts_with('/') {
        return format!("{esquema}://{host}{u}");
    }
    let caminho = resto.split(['?', '#']).next().unwrap_or("");
    let dir = caminho.rsplit_once('/').map_or(host, |(d, _)| d);
    format!("{esquema}://{dir}/{u}")
}

fn lista(d: &Daemon) -> String {
    let mut linhas: Vec<String> = d
        .downloads
        .gravados
        .lock()
        .unwrap()
        .iter()
        .map(Arquivo::linha)
        .collect();
    for b in d.downloads.lista.lock().unwrap().iter() {
        match b.situacao {
            Situacao::Andamento => linhas.push(format!(
                "download em andamento: {} ({} de {}) {}",
                b.nome,
                tamanho(b.recebidos),
                if b.total > 0 { tamanho(b.total) } else { "?".into() },
                b.url
            )),
            Situacao::Cancelado => linhas.push(format!("download cancelado: {} {}", b.nome, b.url)),
            Situacao::Concluido => {}
        }
    }
    if linhas.is_empty() {
        return format!("nenhum arquivo nesta sessão (pasta {})", pasta_padrao().display());
    }
    linhas.join("\n")
}

// ---------------------------------------------------------------- nd pdf

#[derive(Deserialize)]
pub struct PdfArgs {
    saida: String,
    #[serde(default)]
    paisagem: bool,
}

/// Imprime a aba atual em PDF (`Page.printToPDF`, com fundo e o tamanho de página do CSS).
pub async fn pdf(d: &Arc<Daemon>, a: PdfArgs) -> R {
    let tab = aba_atual(d).await?;
    let v = tab
        .call(
            "Page.printToPDF",
            json!({
                "printBackground": true,
                "preferCSSPageSize": true,
                "landscape": a.paisagem,
            }),
        )
        .await
        .map_err(e)?;
    let dados = base64_decodificar(v["data"].as_str().unwrap_or("")).ok_or("PDF em base64 inválido")?;
    let p = PathBuf::from(&a.saida);
    if let Some(pai) = p.parent() {
        let _ = std::fs::create_dir_all(pai);
    }
    std::fs::write(&p, &dados).map_err(|x| format!("{}: {x}", a.saida))?;
    let arq = Arquivo {
        origem: "pdf da página".into(),
        caminho: p,
        bytes: dados.len() as u64,
        tipo: "application/pdf".into(),
    };
    let l = arq.linha();
    d.downloads.gravar(arq);
    Ok(l)
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn nomes() {
        assert_eq!(limpar_nome("a/b:c?.pdf"), "a_b_c_.pdf");
        assert_eq!(limpar_nome(" . "), "download");
        assert_eq!(
            nome_disposicao(r#"attachment; filename="rel 1.pdf""#).as_deref(),
            Some("rel 1.pdf")
        );
        assert_eq!(
            nome_disposicao("inline; filename=a.pdf; filename*=UTF-8''rela%C3%A7%C3%A3o.pdf").as_deref(),
            Some("relação.pdf")
        );
        assert_eq!(
            nome_para("https://x.com/docs/nota.pdf?v=1", "application/pdf", None),
            "nota.pdf"
        );
        assert_eq!(
            nome_para("https://x.com/gerar", "application/pdf", None),
            "gerar.pdf"
        );
        assert_eq!(nome_para("https://x.com/", "text/csv", None), "arquivo.csv");
    }

    #[test]
    fn arquivo_ou_pagina() {
        assert!(eh_arquivo(200, "application/pdf", None));
        assert!(eh_arquivo(200, "application/pdf", Some("inline; filename=a.pdf")));
        assert!(!eh_arquivo(
            200,
            "application/pdf",
            Some("attachment; filename=a.pdf")
        ));
        assert!(!eh_arquivo(200, "text/html; charset=utf-8", None));
        assert!(!eh_arquivo(302, "application/pdf", None));
    }

    #[test]
    fn nome_livre() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("a.pdf"), b"1").unwrap();
        std::fs::write(t.path().join("a (2).pdf"), b"1").unwrap();
        assert_eq!(livre(t.path(), "a.pdf"), t.path().join("a (3).pdf"));
        assert_eq!(livre(t.path(), "b"), t.path().join("b"));
    }

    #[test]
    fn url_absoluta() {
        assert_eq!(absoluta("https://a.com/x/y?q=1", "/f.pdf"), "https://a.com/f.pdf");
        assert_eq!(absoluta("https://a.com/x/y", "f.pdf"), "https://a.com/x/f.pdf");
        assert_eq!(absoluta("https://a.com", "f.pdf"), "https://a.com/f.pdf");
        assert_eq!(absoluta("https://a.com/x", "https://b.com/z"), "https://b.com/z");
    }
}
