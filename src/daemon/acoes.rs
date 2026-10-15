//! `nd map` e as ações sobre elementos. Cada ação lê a página antes, resolve o alvo (ref ou
//! texto), age, espera a página assentar e devolve o que mudou.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nodriver::Tab;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Daemon;
use super::comandos::aba_atual;
use super::pagina::{self, Instantaneo, OpFormato, OpMapa, Quadro, estabilizar, js, js_ref};

type R = Result<String, String>;

/// Espera máxima pela página depois de uma ação.
const ESPERA_ACAO: Duration = Duration::from_secs(10);
/// Tempo para um alvo dado por texto aparecer na página.
const ESPERA_ALVO: Duration = Duration::from_secs(5);
/// Tempo para as opções de uma lista customizada aparecerem (`pick`).
const ESPERA_OPCOES: Duration = Duration::from_secs(5);

fn e(x: impl std::fmt::Display) -> String {
    x.to_string()
}

#[derive(Deserialize)]
pub struct MapArgs {
    #[serde(default)]
    tudo: bool,
    dentro: Option<String>,
    max: Option<usize>,
    #[serde(default)]
    href: bool,
}

pub async fn map(d: &Arc<Daemon>, a: MapArgs) -> R {
    let tab = aba_atual(d).await?;
    let inst = pagina::instantaneo(
        d,
        &tab,
        &OpMapa {
            dentro: a.dentro,
            href: a.href,
            texto: false,
        },
    )
    .await?;
    let max = a.max.unwrap_or(if a.tudo { 2000 } else { 300 });
    Ok(pagina::formatar(&inst, &OpFormato { tudo: a.tudo, max }))
}

/// Elemento sobre o qual uma ação age.
pub struct Alvo {
    pub r: String,
    pub q: Quadro,
    pub l: String,
    /// Observação sobre como o alvo foi achado (ref refeita pelo localizador).
    pub nota: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Filtro {
    Qualquer,
    Campo,
    Marcavel,
}

impl Filtro {
    fn aceita(self, p: &str) -> bool {
        match self {
            Filtro::Qualquer => true,
            Filtro::Campo => matches!(
                p,
                "input" | "textarea" | "editor" | "combobox" | "textbox" | "searchbox" | "spinbutton"
            ),
            Filtro::Marcavel => matches!(
                p,
                "checkbox" | "radio" | "switch" | "menuitemcheckbox" | "menuitemradio"
            ),
        }
    }
}

pub fn eh_ref(s: &str) -> bool {
    s.len() > 1 && s.starts_with('e') && s[1..].bytes().all(|b| b.is_ascii_digit())
}

/// Minúsculas, sem acento e com espaços normalizados (como `chave` no nd.js).
pub fn chave(s: &str) -> String {
    let sem: String = s
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            c => c,
        })
        .collect();
    sem.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Erro de resolução; `esperar` = vale tentar de novo (o alvo pode ainda não ter aparecido).
struct Falha {
    esperar: bool,
    msg: String,
}

impl From<String> for Falha {
    fn from(msg: String) -> Self {
        Falha { esperar: false, msg }
    }
}

async fn resolver(
    d: &Arc<Daemon>,
    tab: &Tab,
    inst: &Instantaneo,
    alvo: &str,
    filtro: Filtro,
) -> Result<Alvo, Falha> {
    let aba = tab.target_id();
    if eh_ref(alvo) {
        let Some(info) = d.paginas.ref_info(aba, alvo) else {
            return Err(if d.paginas.de_pagina_anterior(aba, alvo) {
                format!("ref {alvo} é de uma página anterior; rode nd map")
            } else {
                format!("ref {alvo} desconhecida; rode nd map")
            }
            .into());
        };
        if let Some(it) = inst.itens.iter().find(|it| it.r.as_deref() == Some(alvo)) {
            return Ok(Alvo {
                r: alvo.into(),
                q: inst.quadros[it.q].clone(),
                l: it.l.clone(),
                nota: None,
            });
        }
        if js(d, &info.quadro, "achar", json!({ "ref": alvo })).await? == json!(true) {
            let st = js(d, &info.quadro, "estado", json!({ "ref": alvo })).await?;
            return Ok(Alvo {
                r: alvo.into(),
                q: info.quadro,
                l: st["l"].as_str().unwrap_or("").into(),
                nota: None,
            });
        }
        let v = js_ref(
            d,
            aba,
            &info.quadro,
            "localizar",
            json!({ "ref": alvo, "p": info.p, "nm": info.nm, "k": info.k }),
        )
        .await?;
        if v["doc"].as_str() != Some(info.doc.as_str()) {
            return Err(format!("ref {alvo} é de uma página anterior; rode nd map").into());
        }
        if v["achou"] == json!(true) {
            return Ok(Alvo {
                r: alvo.into(),
                q: info.quadro,
                l: v["l"].as_str().unwrap_or("").into(),
                nota: Some(format!(
                    "{alvo} tinha sido recriado pela página; achado pelo papel e nome"
                )),
            });
        }
        return Err(format!(
            "ref {alvo} velha: {} \"{}\" não está mais na página; rode nd map",
            info.p, info.nm
        )
        .into());
    }

    let k = chave(alvo);
    if k.is_empty() {
        return Err("alvo vazio".to_string().into());
    }
    let pontua = |nm: &str| {
        let c = chave(nm);
        if c == k {
            3
        } else if c.starts_with(&k) {
            2
        } else if c.contains(&k) {
            1
        } else {
            0
        }
    };
    let candidatos: Vec<(usize, i32)> = inst
        .itens
        .iter()
        .enumerate()
        .filter(|(_, it)| it.t == "el" && it.r.is_some() && filtro.aceita(it.p.as_deref().unwrap_or("")))
        .map(|(i, it)| (i, pontua(it.nm.as_deref().unwrap_or(""))))
        .filter(|(_, p)| *p > 0)
        .collect();
    if let Some(melhor) = candidatos.iter().map(|(_, p)| *p).max() {
        let mut iguais: Vec<usize> = candidatos
            .iter()
            .filter(|(_, p)| *p == melhor)
            .map(|(i, _)| *i)
            .collect();
        // Com diálogo modal aberto, o que está nele; depois, o que está na área visível.
        if inst.modal && iguais.iter().any(|i| inst.itens[*i].m) {
            iguais.retain(|i| inst.itens[*i].m);
        }
        if iguais.len() > 1 && iguais.iter().any(|i| inst.itens[*i].z == 0) {
            iguais.retain(|i| inst.itens[*i].z == 0);
        }
        if iguais.len() == 1 {
            let it = &inst.itens[iguais[0]];
            return Ok(Alvo {
                r: it.r.clone().unwrap(),
                q: inst.quadros[it.q].clone(),
                l: it.l.clone(),
                nota: None,
            });
        }
        let lista: Vec<String> = iguais
            .iter()
            .take(8)
            .map(|i| {
                let it = &inst.itens[*i];
                format!("  {} {}", it.r.as_deref().unwrap_or(""), it.l)
            })
            .collect();
        return Err(format!(
            "\"{alvo}\" bate com {} elementos; use a ref:\n{}",
            iguais.len(),
            lista.join("\n")
        )
        .into());
    }
    if filtro == Filtro::Qualquer {
        let q = &inst.quadros[0];
        let v = js_ref(d, aba, q, "porTexto", json!({ "texto": alvo })).await?;
        if let Some(r) = v["r"].as_str() {
            return Ok(Alvo {
                r: r.into(),
                q: q.clone(),
                l: v["l"].as_str().unwrap_or("").into(),
                nota: None,
            });
        }
    }
    let o_que = match filtro {
        Filtro::Qualquer => "nenhum elemento",
        Filtro::Campo => "nenhum campo",
        Filtro::Marcavel => "nenhum checkbox/radio",
    };
    Err(Falha {
        esperar: true,
        msg: format!("{o_que} com \"{alvo}\" na página; rode nd map"),
    })
}

enum Ponto {
    Em(f64, f64),
    SemCaixa,
    Coberto(String),
}

/// Ponto de clique no viewport da aba: o do elemento no quadro dele mais a origem de cada
/// iframe de outro domínio até o quadro principal.
async fn ponto(d: &Arc<Daemon>, tab: &Tab, alvo: &Alvo) -> Result<Ponto, String> {
    let aba = tab.target_id();
    let v = js_ref(d, aba, &alvo.q, "ponto", json!({ "ref": alvo.r })).await?;
    if v["semCaixa"] == json!(true) || v["foraDaTela"] == json!(true) {
        return Ok(Ponto::SemCaixa);
    }
    if let Some(c) = v.get("coberto").filter(|c| !c.is_null()) {
        return Ok(Ponto::Coberto(format!(
            "{} {}",
            c["r"].as_str().unwrap_or(""),
            c["l"].as_str().unwrap_or("")
        )));
    }
    let (mut x, mut y) = (v["x"].as_f64().unwrap_or(0.0), v["y"].as_f64().unwrap_or(0.0));
    let mut q = alvo.q.clone();
    while let Some((pai, r_iframe)) = d.paginas.pai(aba, &q) {
        let o = js(
            d,
            &pai,
            "caixaQuadro",
            json!({ "ref": r_iframe, "px": x, "py": y }),
        )
        .await?;
        x += o["x"].as_f64().unwrap_or(0.0);
        y += o["y"].as_f64().unwrap_or(0.0);
        q = pai;
    }
    Ok(Ponto::Em(x, y))
}

/// Clique de mouse (CDP) no elemento; sem caixa visível, `el.click()`.
async fn clicar(d: &Arc<Daemon>, tab: &Tab, alvo: &Alvo, via_js: bool) -> Result<Vec<String>, String> {
    if via_js {
        js(d, &alvo.q, "clicarJs", json!({ "ref": alvo.r })).await?;
        return Ok(vec!["clique via JS (el.click())".into()]);
    }
    match ponto(d, tab, alvo).await? {
        Ponto::Em(x, y) => {
            tab.mouse_click(x, y).await.map_err(e)?;
            Ok(Vec::new())
        }
        Ponto::SemCaixa => {
            js(d, &alvo.q, "clicarJs", json!({ "ref": alvo.r })).await?;
            Ok(vec!["sem caixa visível; clique via JS".into()])
        }
        Ponto::Coberto(quem) => Err(format!(
            "{} está coberto por {quem}; feche o que cobre ou use --js",
            alvo.r
        )),
    }
}

fn so_alfanum(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

async fn preencher(
    d: &Arc<Daemon>,
    tab: &Tab,
    alvo: &Alvo,
    valor: &str,
    teclas: bool,
) -> Result<Vec<String>, String> {
    let mut notas = Vec::new();
    let t = js(d, &alvo.q, "tipo", json!({ "ref": alvo.r })).await?;
    match t["tipo"].as_str().unwrap_or("") {
        "select" => return Err(format!("{} é um select; use nd pick", alvo.r)),
        "arquivo" => return Err(format!("{} é um campo de arquivo; use nd upload", alvo.r)),
        "outro" => return Err(format!("{} não é campo de texto: {}", alvo.r, alvo.l)),
        "direto" => {
            js(d, &alvo.q, "definir", json!({ "ref": alvo.r, "valor": valor })).await?;
            return Ok(notas);
        }
        _ => {}
    }
    let raiz = pagina::principal(d, tab).await?;
    let mut prep = js(d, &alvo.q, "preparar", json!({ "ref": alvo.r })).await?;
    // Num iframe de outro domínio o foco do teclado só vai para o quadro com um clique.
    if prep["foco"] != json!(true) || alvo.q != raiz {
        notas.extend(clicar(d, tab, alvo, false).await?);
        prep = js(d, &alvo.q, "preparar", json!({ "ref": alvo.r })).await?;
    }
    let _ = prep;
    if valor.is_empty() {
        tab.press_key("Delete").await.map_err(e)?;
    } else if teclas {
        tab.type_text(valor).await.map_err(e)?;
    } else {
        tab.insert_text(valor).await.map_err(e)?;
    }
    let st = js(d, &alvo.q, "estado", json!({ "ref": alvo.r })).await?;
    let atual = st["valor"].as_str().unwrap_or("").to_string();
    if atual == valor {
        return Ok(notas);
    }
    if !atual.is_empty() && so_alfanum(&atual) == so_alfanum(valor) {
        notas.push(format!("o campo formatou o valor: {atual:?}"));
        return Ok(notas);
    }
    let st = js(d, &alvo.q, "definir", json!({ "ref": alvo.r, "valor": valor })).await?;
    let atual = st["valor"].as_str().unwrap_or("");
    if atual == valor {
        notas.push("a digitação não pegou; valor definido por JS".into());
    } else {
        notas.push(format!("o campo ficou com {atual:?}"));
    }
    Ok(notas)
}

async fn escolher(d: &Arc<Daemon>, tab: &Tab, alvo: &Alvo, opcao: &str) -> Result<Vec<String>, String> {
    let aba = tab.target_id();
    let t = js(d, &alvo.q, "tipo", json!({ "ref": alvo.r })).await?;
    let tipo = t["tipo"].as_str().unwrap_or("");
    if tipo == "select" {
        let v = js(d, &alvo.q, "escolher", json!({ "ref": alvo.r, "opcao": opcao })).await?;
        if v["erro"] == json!(true) {
            let ops: Vec<&str> = v["opcoes"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            return Err(format!("nenhuma opção \"{opcao}\"; opções: {}", ops.join(" | ")));
        }
        return Ok(vec![format!(
            "escolhido \"{}\"",
            v["texto"].as_str().unwrap_or("")
        )]);
    }
    let mut notas = Vec::new();
    if tipo == "texto" || tipo == "editor" {
        preencher(d, tab, alvo, opcao, false).await?;
    } else {
        notas.extend(clicar(d, tab, alvo, false).await?);
    }
    let fim = Instant::now() + ESPERA_OPCOES;
    let mut visiveis: Vec<String>;
    loop {
        let v = js_ref(d, aba, &alvo.q, "opcoes", json!({ "texto": opcao })).await?;
        if let Some(esc) = v.get("escolhida").filter(|x| !x.is_null()) {
            let op = Alvo {
                r: esc["r"].as_str().unwrap_or("").into(),
                q: alvo.q.clone(),
                l: String::new(),
                nota: None,
            };
            notas.extend(clicar(d, tab, &op, false).await?);
            notas.push(format!("escolhido \"{}\"", esc["t"].as_str().unwrap_or("")));
            return Ok(notas);
        }
        visiveis = v["textos"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        if Instant::now() >= fim {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    if visiveis.is_empty() {
        Err(format!(
            "nenhuma lista de opções apareceu depois de {} s",
            ESPERA_OPCOES.as_secs()
        ))
    } else {
        Err(format!(
            "nenhuma opção \"{opcao}\"; visíveis: {}",
            visiveis.join(" | ")
        ))
    }
}

async fn marcar(d: &Arc<Daemon>, tab: &Tab, alvo: &Alvo, desejado: bool) -> Result<Vec<String>, String> {
    let estado = |v: Value| v["marcado"].as_bool();
    let t = js(d, &alvo.q, "tipo", json!({ "ref": alvo.r })).await?;
    let Some(atual) = estado(t) else {
        return Err(format!("{} não é checkbox, radio ou switch: {}", alvo.r, alvo.l));
    };
    let nome = if desejado { "marcado" } else { "desmarcado" };
    if atual == desejado {
        return Ok(vec![format!("já estava {nome}")]);
    }
    let mut notas = clicar(d, tab, alvo, false).await?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let agora = estado(js(d, &alvo.q, "estado", json!({ "ref": alvo.r })).await?);
    if agora == Some(desejado) {
        return Ok(notas);
    }
    js(d, &alvo.q, "clicarJs", json!({ "ref": alvo.r })).await?;
    let agora = estado(js(d, &alvo.q, "estado", json!({ "ref": alvo.r })).await?);
    if agora == Some(desejado) {
        notas.push("o clique não mudou o estado; mudado via JS".into());
        Ok(notas)
    } else {
        Err(format!("{} não ficou {nome}", alvo.r))
    }
}

async fn enviar_arquivos(
    d: &Arc<Daemon>,
    tab: &Tab,
    alvo: &Alvo,
    arquivos: &[String],
) -> Result<Vec<String>, String> {
    for f in arquivos {
        if !std::path::Path::new(f).is_file() {
            return Err(format!("arquivo não encontrado: {f}"));
        }
    }
    let sessao = d.browser.tab(&alvo.q.alvo).await.map_err(e)?;
    let t = js(d, &alvo.q, "tipo", json!({ "ref": alvo.r })).await?;
    if t["tipo"] == json!("arquivo") {
        let obj = pagina::handle(d, &alvo.q, &alvo.r).await?;
        let r = sessao
            .call(
                "DOM.setFileInputFiles",
                json!({ "files": arquivos, "objectId": obj }),
            )
            .await;
        pagina::soltar(d, &alvo.q, &obj).await;
        r.map_err(e)?;
        return Ok(Vec::new());
    }
    // Botão ou label que abre o seletor: intercepta o seletor e entrega os arquivos ao input dele.
    let _ = sessao.call("Page.enable", json!({})).await;
    sessao
        .call("Page.setInterceptFileChooserDialog", json!({ "enabled": true }))
        .await
        .map_err(e)?;
    let mut ev = sessao.subscribe_raw(Some("Page.fileChooserOpened"));
    let clique = clicar(d, tab, alvo, false).await;
    let aberto = match &clique {
        Ok(_) => tokio::time::timeout(Duration::from_secs(5), ev.next())
            .await
            .ok()
            .flatten(),
        Err(_) => None,
    };
    let _ = sessao
        .call("Page.setInterceptFileChooserDialog", json!({ "enabled": false }))
        .await;
    let notas = clique?;
    let ev = aberto.ok_or_else(|| format!("o clique em {} não abriu seletor de arquivo", alvo.r))?;
    let no = ev.params["backendNodeId"]
        .as_i64()
        .ok_or("seletor de arquivo sem backendNodeId")?;
    sessao
        .call(
            "DOM.setFileInputFiles",
            json!({ "files": arquivos, "backendNodeId": no }),
        )
        .await
        .map_err(e)?;
    Ok(notas)
}

async fn teclas(
    d: &Arc<Daemon>,
    tab: &Tab,
    alvo: Option<&Alvo>,
    teclas: &[String],
) -> Result<Vec<String>, String> {
    let mut notas = Vec::new();
    if let Some(alvo) = alvo {
        let raiz = pagina::principal(d, tab).await?;
        let foco = js(d, &alvo.q, "focar", json!({ "ref": alvo.r })).await?;
        if foco != json!(true) || alvo.q != raiz {
            notas.extend(clicar(d, tab, alvo, false).await?);
        }
    }
    for t in teclas {
        tab.press_key(t).await.map_err(|x| format!("tecla {t}: {x}"))?;
    }
    Ok(notas)
}

pub enum Acao {
    Click { js: bool },
    Fill { valor: String, teclas: bool },
    Pick { opcao: String },
    Check { marcar: bool },
    Upload { arquivos: Vec<String> },
    Key { teclas: Vec<String> },
}

impl Acao {
    fn filtro(&self) -> Filtro {
        match self {
            Acao::Fill { .. } => Filtro::Campo,
            Acao::Check { .. } => Filtro::Marcavel,
            _ => Filtro::Qualquer,
        }
    }

    fn verbo(&self) -> &'static str {
        match self {
            Acao::Click { .. } => "clique em",
            Acao::Fill { .. } => "preenchido",
            Acao::Pick { .. } => "escolha em",
            Acao::Check { marcar: true } => "marcado",
            Acao::Check { marcar: false } => "desmarcado",
            Acao::Upload { .. } => "arquivo(s) em",
            Acao::Key { .. } => "teclas em",
        }
    }
}

#[derive(Deserialize)]
pub struct AcaoArgs {
    pub alvo: Option<String>,
    #[serde(default)]
    pub sem_diff: bool,
}

pub async fn agir(d: &Arc<Daemon>, a: AcaoArgs, acao: Acao) -> R {
    let tab = aba_atual(d).await?;
    let op = OpMapa {
        texto: !a.sem_diff,
        ..Default::default()
    };
    let inicio = Instant::now();
    let (antes, alvo) = loop {
        let antes = pagina::instantaneo(d, &tab, &op).await?;
        let Some(txt) = a.alvo.as_deref() else {
            break (antes, None);
        };
        match resolver(d, &tab, &antes, txt, acao.filtro()).await {
            Ok(x) => break (antes, Some(x)),
            Err(f) if f.esperar && inicio.elapsed() < ESPERA_ALVO => {
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            Err(f) => return Err(f.msg),
        }
    };
    let aba = tab.target_id().to_string();
    d.paginas.marcar_acao(&aba);
    let t0 = Instant::now();
    let notas = match (&acao, &alvo) {
        (Acao::Key { teclas: t }, al) => teclas(d, &tab, al.as_ref(), t).await,
        (_, None) => Err("informe o alvo (ref ou texto)".into()),
        (Acao::Click { js }, Some(al)) => clicar(d, &tab, al, *js).await,
        (Acao::Fill { valor, teclas }, Some(al)) => preencher(d, &tab, al, valor, *teclas).await,
        (Acao::Pick { opcao }, Some(al)) => escolher(d, &tab, al, opcao).await,
        (Acao::Check { marcar: m }, Some(al)) => marcar(d, &tab, al, *m).await,
        (Acao::Upload { arquivos }, Some(al)) => enviar_arquivos(d, &tab, al, arquivos).await,
    };
    let notas = match notas {
        Ok(n) => n,
        Err(x) => {
            // Mesmo com erro, o diálogo que a ação abriu foi aceito; avisa.
            let dl = d.paginas.tirar_dialogos(&aba);
            return Err(if dl.is_empty() {
                x
            } else {
                format!("{x}\n{}", dl.join("\n"))
            });
        }
    };
    let espera = estabilizar(d, &tab, t0, ESPERA_ACAO).await;

    let mut out = Vec::new();
    if let Some(al) = &alvo {
        out.push(format!("{} {} {}", acao.verbo(), al.r, al.l));
        if let Some(n) = &al.nota {
            out.push(n.clone());
        }
    }
    out.extend(notas);
    out.extend(d.paginas.tirar_dialogos(&aba));
    let origem = tab.url().await.unwrap_or_default();
    let origem = origem
        .split_once("://")
        .map(|(e, r)| format!("{e}://{}", r.split(['/', '?', '#']).next().unwrap_or("")))
        .unwrap_or_default();
    if let Some(r) = super::net::resumo(d, &aba, t0, &origem) {
        out.push(r);
    }
    if !a.sem_diff {
        let depois = pagina::instantaneo(d, &tab, &op).await?;
        out.push(pagina::diferenca(&antes, &depois));
    }
    for av in espera.avisos() {
        out.push(format!("aviso: {av}"));
    }
    Ok(out.join("\n"))
}

#[derive(Deserialize)]
pub struct WaitArgs {
    tipo: String,
    valor: String,
    #[serde(default)]
    sumir: bool,
    #[serde(default = "espera_padrao")]
    max: u64,
}

fn espera_padrao() -> u64 {
    15
}

pub async fn wait(d: &Arc<Daemon>, a: WaitArgs) -> R {
    let tab = aba_atual(d).await?;
    let aba = tab.target_id().to_string();
    let inicio = Instant::now();
    let fim = inicio + Duration::from_secs(a.max);
    let desde = d.paginas.ultima_acao(&aba).unwrap_or(inicio);
    loop {
        let achou: Option<String> = match a.tipo.as_str() {
            "text" => {
                let mut quadros = vec![pagina::principal(d, &tab).await?];
                quadros.extend(d.paginas.quadros_filhos(&aba));
                let mut sim = None;
                for q in &quadros {
                    if js(d, q, "temTexto", json!({ "texto": a.valor })).await.ok() == Some(json!(true)) {
                        sim = Some(String::new());
                        break;
                    }
                }
                sim
            }
            "sel" => {
                let q = pagina::principal(d, &tab).await?;
                (js(d, &q, "temSel", json!({ "sel": a.valor })).await? == json!(true)).then(String::new)
            }
            "url" => {
                let u = tab.url().await.map_err(e)?;
                u.contains(&a.valor).then_some(u)
            }
            "net" => d.rede.concluida_desde(&aba, desde, &a.valor),
            outro => {
                return Err(format!(
                    "tipo de espera desconhecido: {outro} (text, sel, url, net)"
                ));
            }
        };
        if achou.is_some() != a.sumir {
            let s = format!("ok em {:.1} s", inicio.elapsed().as_secs_f64());
            return Ok(match achou {
                Some(x) if !x.is_empty() => format!("{s}: {x}"),
                _ => s,
            });
        }
        if Instant::now() >= fim {
            let verbo = if a.sumir { "não sumiu" } else { "não apareceu" };
            return Err(format!("{} {:?} {verbo} em {} s", a.tipo, a.valor, a.max));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn refs_e_chave() {
        assert!(eh_ref("e12"));
        assert!(!eh_ref("e"));
        assert!(!eh_ref("enviar"));
        assert_eq!(chave("  São   Paulo "), "sao paulo");
        assert_eq!(so_alfanum("123.456-78"), "12345678");
    }
}
