//! `nd map` e as ações sobre elementos. Cada ação lê a página antes, resolve o alvo (ref ou
//! texto), age, espera a página assentar e devolve o que mudou.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nodriver::Tab;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Daemon;
use super::baixar;
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
    /// Nome que acha o elemento de novo por texto (para o histórico e o atlas).
    pub nm: String,
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
                nm: info.nm.clone(),
            });
        }
        if js(d, &info.quadro, "achar", json!({ "ref": alvo })).await? == json!(true) {
            let st = js(d, &info.quadro, "estado", json!({ "ref": alvo })).await?;
            return Ok(Alvo {
                r: alvo.into(),
                q: info.quadro,
                l: st["l"].as_str().unwrap_or("").into(),
                nota: None,
                nm: info.nm.clone(),
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
                nm: info.nm.clone(),
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
                nm: alvo.to_string(),
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
                nm: alvo.to_string(),
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
    let (x, y) = (v["x"].as_f64().unwrap_or(0.0), v["y"].as_f64().unwrap_or(0.0));
    let (x, y) = no_viewport(d, aba, &alvo.q, x, y).await?;
    Ok(Ponto::Em(x, y))
}

/// Ponto `(x, y)` do viewport do quadro `q` levado ao viewport da aba, somando a origem de cada
/// iframe de outro domínio até o quadro principal.
async fn no_viewport(
    d: &Arc<Daemon>,
    aba: &str,
    q: &Quadro,
    mut x: f64,
    mut y: f64,
) -> Result<(f64, f64), String> {
    let mut q = q.clone();
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
    Ok((x, y))
}

/// Clique de mouse (CDP) no elemento; sem caixa visível, `el.click()`.
pub async fn clicar(d: &Arc<Daemon>, tab: &Tab, alvo: &Alvo, via_js: bool) -> Result<Vec<String>, String> {
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
    let senha = st["senha"] == json!(true);
    if !atual.is_empty() && so_alfanum(&atual) == so_alfanum(valor) {
        notas.push(if senha {
            "o campo de senha formatou o valor".into()
        } else {
            format!("o campo formatou o valor: {atual:?}")
        });
        return Ok(notas);
    }
    let st = js(d, &alvo.q, "definir", json!({ "ref": alvo.r, "valor": valor })).await?;
    let atual = st["valor"].as_str().unwrap_or("");
    if atual == valor {
        notas.push("a digitação não pegou; valor definido por JS".into());
    } else if senha {
        notas.push("o campo de senha ficou com outro valor".into());
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
            let amb: Vec<&str> = v["ambiguas"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if !amb.is_empty() {
                return Err(format!(
                    "a opção \"{opcao}\" bate com mais de uma: {}; use o texto completo",
                    amb.join(" | ")
                ));
            }
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
    let mut ambiguas: Vec<String> = Vec::new();
    let mut buscou = tipo == "texto" || tipo == "editor";
    loop {
        let v = js_ref(
            d,
            aba,
            &alvo.q,
            "opcoes",
            json!({ "texto": opcao, "ref": alvo.r }),
        )
        .await?;
        if !buscou && v["busca"] == json!(true) && v["escolhida"].is_null() {
            // A lista abriu com um campo de busca focado; digitar filtra (e desenha) a opção.
            tab.insert_text(opcao).await.map_err(e)?;
            notas.push("digitado na busca da lista".into());
            buscou = true;
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }
        if let Some(esc) = v.get("escolhida").filter(|x| !x.is_null()) {
            let op = Alvo {
                r: esc["r"].as_str().unwrap_or("").into(),
                q: alvo.q.clone(),
                l: String::new(),
                nota: None,
                nm: String::new(),
            };
            notas.extend(clicar(d, tab, &op, false).await?);
            notas.push(format!("escolhido \"{}\"", esc["t"].as_str().unwrap_or("")));
            return Ok(notas);
        }
        visiveis = v["textos"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        // Empate entre opções: com a mesma lista em duas leituras seguidas, para na hora e deixa a
        // lista aberta; o agente clica numa das refs listadas.
        let amb: Vec<String> = v["ambiguas"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|x| format!("{} option \"{}\"", x["r"].as_str().unwrap_or(""), x["t"].as_str().unwrap_or("")))
                    .collect()
            })
            .unwrap_or_default();
        if !amb.is_empty() && amb == ambiguas {
            return Err(format!(
                "a opção \"{opcao}\" bate com mais de uma; a lista segue aberta, clique numa: {}",
                amb.join(" | ")
            ));
        }
        ambiguas = amb;
        if Instant::now() >= fim {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    if !ambiguas.is_empty() {
        Err(format!(
            "a opção \"{opcao}\" bate com mais de uma; a lista segue aberta, clique numa: {}",
            ambiguas.join(" | ")
        ))
    } else if visiveis.is_empty() {
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

/// Limite de rolagens de `nd scroll --fim` sem `--vezes`.
pub const ROLAGENS_FIM: u32 = 30;
/// Espera pela página depois de cada rolagem.
const ESPERA_ROLAGEM: Duration = Duration::from_secs(5);
/// Fração da altura visível rolada por vez (sobra um pedaço da tela anterior).
const PASSO_ROLAGEM: f64 = 0.9;

/// Posição de rolagem lida pelo `rolagem` do nd.js.
struct Medida {
    x: f64,
    y: f64,
    top: f64,
    cliente: f64,
    total: f64,
}

impl Medida {
    fn de(v: &Value) -> Self {
        let n = |k: &str| v[k].as_f64().unwrap_or(0.0);
        Medida {
            x: n("x"),
            y: n("y"),
            top: n("top"),
            cliente: n("cliente"),
            total: n("total"),
        }
    }

    fn no_limite(&self, subir: bool) -> bool {
        if subir {
            self.top <= 1.0
        } else {
            (self.top + self.cliente).ceil() >= self.total - 1.0
        }
    }
}

/// Rola a página (ou a área com rolagem própria do alvo) com gestos sintetizados pela
/// biblioteca, esperando a página assentar depois de cada um. Com `fim`, segue até a posição
/// chegar ao limite e a altura parar de crescer, até `vezes` (padrão [`ROLAGENS_FIM`]).
async fn rolar(
    d: &Arc<Daemon>,
    tab: &Tab,
    alvo: Option<&Alvo>,
    subir: bool,
    vezes: Option<u32>,
    fim: bool,
) -> Result<Vec<String>, String> {
    let aba = tab.target_id();
    let q = match alvo {
        Some(al) => al.q.clone(),
        None => pagina::principal(d, tab).await?,
    };
    let mut args = alvo.map_or(json!({}), |al| json!({ "ref": al.r }));
    let v = js_ref(d, aba, &q, "rolagem", args.clone()).await?;
    let mut notas = Vec::new();
    if let Some(al) = alvo {
        if v["semRolagem"] == json!(true) {
            return Err(format!(
                "{} não tem rolagem própria nem está numa área com rolagem; sem --dentro rola a página",
                al.r
            ));
        }
        let r = v["r"].as_str().unwrap_or("");
        if r != al.r {
            notas.push(format!("área com rolagem: {r} {}", v["l"].as_str().unwrap_or("")));
        }
        args = json!({ "ref": r });
    }
    let medir = || async {
        js_ref(d, aba, &q, "rolagem", args.clone())
            .await
            .map(|v| Medida::de(&v))
    };
    let limite = vezes.unwrap_or(if fim { ROLAGENS_FIM } else { 1 }).max(1);
    let sentido = if subir { -1.0 } else { 1.0 };
    let mut m = Medida::de(&v);
    let inicio = m.top;
    let mut n = 0;
    let mut chegou = false;
    while n < limite {
        let (x, y) = no_viewport(d, aba, &q, m.x, m.y).await?;
        let dist = (m.cliente * PASSO_ROLAGEM).max(50.0) * sentido;
        tab.scroll_gesture(x, y, 0.0, dist).await.map_err(e)?;
        n += 1;
        estabilizar(d, tab, Instant::now(), ESPERA_ROLAGEM).await;
        let mut depois = medir().await?;
        if depois.no_limite(subir) && (subir || depois.total <= m.total) {
            // Lista que carrega mais pode demorar um instante para pedir a próxima página.
            if !subir {
                tokio::time::sleep(Duration::from_secs(1)).await;
                estabilizar(d, tab, Instant::now(), ESPERA_ROLAGEM).await;
                depois = medir().await?;
            }
            if depois.no_limite(subir) && (subir || depois.total <= m.total) {
                chegou = true;
                m = depois;
                break;
            }
        }
        m = depois;
    }
    let onde = if alvo.is_some() { "a área" } else { "a página" };
    let direcao = if subir { "para cima" } else { "para baixo" };
    notas.push(format!(
        "{n} rolagem(ns) {direcao}: posição {:.0} → {:.0} de {:.0} px",
        inicio, m.top, m.total
    ));
    if chegou {
        notas.push(if subir {
            format!("{onde} está no topo")
        } else {
            format!("fim: {onde} parou de crescer")
        });
    } else if fim {
        notas.push(format!(
            "parou no limite de {limite} rolagens; {onde} ainda crescia (--vezes N aumenta o limite)"
        ));
    }
    Ok(notas)
}

pub enum Acao {
    Scroll {
        subir: bool,
        vezes: Option<u32>,
        fim: bool,
    },
    Click {
        js: bool,
    },
    Fill {
        valor: String,
        teclas: bool,
    },
    Pick {
        opcao: String,
    },
    Check {
        marcar: bool,
    },
    Upload {
        arquivos: Vec<String>,
    },
    Key {
        teclas: Vec<String>,
    },
    Download {
        js: bool,
        max: u64,
        pasta: Option<String>,
    },
    Login {
        enviar: Option<String>,
        checar: Option<String>,
        usuario: Option<String>,
    },
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
            Acao::Download { .. } => "download por",
            Acao::Scroll { .. } => "rolagem dentro de",
            Acao::Login { .. } => "login",
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
        (Acao::Scroll { subir, vezes, fim }, al) => rolar(d, &tab, al.as_ref(), *subir, *vezes, *fim).await,
        (
            Acao::Login {
                enviar,
                usuario,
                checar,
            },
            _,
        ) => {
            logar(
                d,
                &tab,
                &antes,
                enviar.as_deref(),
                usuario.as_deref(),
                checar.as_deref(),
            )
            .await
        }
        (_, None) => Err("informe o alvo (ref ou texto)".into()),
        (Acao::Click { js }, Some(al)) => clicar(d, &tab, al, *js).await,
        (Acao::Fill { valor, teclas }, Some(al)) => preencher(d, &tab, al, valor, *teclas).await,
        (Acao::Pick { opcao }, Some(al)) => escolher(d, &tab, al, opcao).await,
        (Acao::Check { marcar: m }, Some(al)) => marcar(d, &tab, al, *m).await,
        (Acao::Upload { arquivos }, Some(al)) => enviar_arquivos(d, &tab, al, arquivos).await,
        (Acao::Download { js, max, pasta }, Some(al)) => {
            let max = Duration::from_secs(*max);
            baixar::clicar_e_baixar(d, &tab, al, *js, max, pasta.as_deref()).await
        }
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
    if let Some(p) = passo(&acao, a.alvo.as_deref(), alvo.as_ref()) {
        d.registrar(p);
    }
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
    if !matches!(acao, Acao::Download { .. }) {
        out.extend(baixar::resumo(d, t0, baixar::ESPERA_DOWNLOAD).await);
    }
    out.extend(abas_novas(d, t0).await);
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
        let dif = pagina::diferenca(&antes, &depois);
        // Na rolagem o que está na tela muda sempre; o diff fala do que carregou.
        out.push(
            if matches!(acao, Acao::Scroll { .. }) && dif == pagina::SEM_MUDANCA {
                "nada novo carregou".into()
            } else {
                dif
            },
        );
    }
    if let Acao::Login { checar, .. } = &acao {
        out.push(resultado_login(d, &tab, checar.as_deref()).await);
    }
    for av in espera.avisos() {
        out.push(format!("aviso: {av}"));
    }
    Ok(out.join("\n"))
}

/// Formulário de login achado num quadro.
struct FormLogin {
    q: Quadro,
    v: Value,
}

async fn achar_login(d: &Arc<Daemon>, tab: &Tab) -> Result<Option<FormLogin>, String> {
    let aba = tab.target_id();
    let mut quadros = vec![pagina::principal(d, tab).await?];
    quadros.extend(d.paginas.quadros_filhos(aba));
    for q in quadros {
        let Ok(v) = js_ref(d, aba, &q, "formLogin", json!({})).await else {
            continue;
        };
        if v["achou"] == json!(true) {
            return Ok(Some(FormLogin { q, v }));
        }
    }
    Ok(None)
}

fn alvo_em(q: &Quadro, r: &str) -> Alvo {
    Alvo {
        r: r.to_string(),
        q: q.clone(),
        l: String::new(),
        nota: None,
        nm: String::new(),
    }
}

/// Login com a senha salva no Edge: o navegador preenche (ao carregar ou pela sugestão escolhida
/// pelo teclado) e o `nd` só envia. O valor da senha nunca passa pelo daemon.
async fn logar(
    d: &Arc<Daemon>,
    tab: &Tab,
    inst: &Instantaneo,
    enviar: Option<&str>,
    usuario: Option<&str>,
    checar: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut notas = Vec::new();
    let mut f = match achar_login(d, tab).await? {
        Some(f) => f,
        // Sessão já aberta: nada a fazer (fluxo do atlas rodando logado).
        None if checar.is_some() && tab.evaluate(checar.unwrap_or("")).await.ok() == Some(json!(true)) => {
            return Ok(vec!["já logado; nada enviado".into()]);
        }
        None => {
            let Some(u) = usuario else {
                return Err(
                    "nenhum campo de senha visível; em login de duas etapas (usuário numa tela, \
                            senha na outra), use nd login --usuario <cpf ou e-mail>"
                        .into(),
                );
            };
            notas.extend(primeira_etapa(d, tab, u).await?);
            achar_login(d, tab).await?.ok_or(
                "depois do usuário não apareceu campo de senha (usuário recusado, captcha ou verificação?)",
            )?
        }
    };
    let ok = |v: &Value| v["senhaOk"] == json!(true) && v["usuarioOk"] == json!(true);
    if ok(&f.v) {
        notas.push("usuário e senha já preenchidos (pelo Edge ou pela página)".into());
    } else {
        // Sugestão do gerenciador de senhas. Campo marcado new-password/off: primeiro troca a
        // dica, depois recria o campo, para o Edge oferecer a senha salva.
        let mut enviado = false;
        let mut preenchido = false;
        let bloqueado = f.v["bloqueado"] == json!(true);
        for etapa in 0..3 {
            if etapa > 0 {
                if !bloqueado {
                    continue;
                }
                js_ref(
                    d,
                    tab.target_id(),
                    &f.q,
                    "liberarSenhaSalva",
                    json!({ "senha": f.v["senha"], "usuario": f.v["usuario"], "clonar": etapa == 2 }),
                )
                .await?;
                notas.push(if etapa == 1 {
                    "o site marca o login com autocomplete new-password/off; dica trocada para current-password".into()
                } else {
                    "campo de senha recriado para o Edge oferecer a senha salva".into()
                });
                if let Some(g) = achar_login(d, tab).await? {
                    f = g;
                }
            }
            match sugestao(d, tab, &f).await? {
                Sugestao::Enviou => {
                    enviado = true;
                    break;
                }
                Sugestao::Preencheu(g) => {
                    f = g;
                    if ok(&f.v) {
                        preenchido = true;
                        break;
                    }
                }
                Sugestao::Nada(g) => f = g,
            }
        }
        if enviado {
            notas.push("senha salva escolhida na sugestão do Edge; o Enter já enviou o formulário".into());
            return Ok(notas);
        }
        if !preenchido {
            let falta = match (f.v["usuarioOk"] == json!(true), f.v["senhaOk"] == json!(true)) {
                (false, false) => "usuário e senha",
                (false, true) => "usuário",
                _ => "senha",
            };
            return Err(format!(
                "o Edge não preencheu {falta}: não há senha salva deste site neste perfil (ou a sugestão não abriu). \
                 Faça o login uma vez na janela do nd e aceite salvar a senha"
            ));
        }
        notas.push("preenchido pela sugestão de senha salva do Edge".into());
    }
    let botao = match enviar {
        Some(t) => match resolver(d, tab, inst, t, Filtro::Qualquer).await {
            Ok(al) => Some(al),
            Err(x) => return Err(x.msg),
        },
        None => f.v["botao"].as_str().map(|r| alvo_em(&f.q, r)),
    };
    match botao {
        Some(al) => {
            notas.extend(clicar(d, tab, &al, false).await?);
            let descr = if al.l.is_empty() {
                f.v["descrBotao"].as_str().unwrap_or("").to_string()
            } else {
                al.l.clone()
            };
            notas.push(format!("enviado por clique em {} {descr}", al.r));
        }
        None => {
            // Sem botão reconhecível: Enter no campo de senha (o clique nele já é o gesto que
            // libera a senha preenchida pelo navegador).
            let r = f.v["senha"].as_str().unwrap_or("").to_string();
            clicar(d, tab, &alvo_em(&f.q, &r), false).await?;
            tab.press_key("Enter").await.map_err(e)?;
            notas.push("enviado com Enter no campo de senha".into());
        }
    }
    Ok(notas)
}

enum Sugestao {
    /// O formulário sumiu: o Enter que escolheu a sugestão também enviou.
    Enviou,
    Preencheu(FormLogin),
    Nada(FormLogin),
}

/// Clica no campo de usuário (depois no de senha), seta para baixo e, só se o Edge mostrou a
/// prévia de uma credencial salva (campo `:autofill`), Enter. Sem prévia não há lista aberta, e o
/// Enter enviaria o formulário vazio: Escape.
async fn sugestao(d: &Arc<Daemon>, tab: &Tab, f: &FormLogin) -> Result<Sugestao, String> {
    let campos: Vec<String> = [&f.v["usuario"], &f.v["senha"]]
        .iter()
        .filter_map(|x| x.as_str().map(str::to_string))
        .collect();
    let mut atual = FormLogin {
        q: f.q.clone(),
        v: f.v.clone(),
    };
    for r in campos {
        clicar(d, tab, &alvo_em(&atual.q, &r), false).await?;
        tokio::time::sleep(Duration::from_millis(500)).await;
        tab.press_key("ArrowDown").await.map_err(e)?;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let previa = achar_login(d, tab)
            .await?
            .is_some_and(|g| g.v["usuarioAuto"] == json!(true) || g.v["senhaAuto"] == json!(true));
        tab.press_key(if previa { "Enter" } else { "Escape" })
            .await
            .map_err(e)?;
        tokio::time::sleep(Duration::from_millis(700)).await;
        match achar_login(d, tab).await? {
            None if previa => return Ok(Sugestao::Enviou),
            None => return Err("o formulário de login sumiu".into()),
            Some(g) if previa => return Ok(Sugestao::Preencheu(g)),
            Some(g) => atual = g,
        }
    }
    Ok(Sugestao::Nada(atual))
}

/// Primeira tela de login em duas etapas: digita o usuário (não é segredo), continua e espera o
/// campo de senha.
async fn primeira_etapa(d: &Arc<Daemon>, tab: &Tab, usuario: &str) -> Result<Vec<String>, String> {
    let aba = tab.target_id();
    let mut quadros = vec![pagina::principal(d, tab).await?];
    quadros.extend(d.paginas.quadros_filhos(aba));
    let mut achado = None;
    for q in quadros {
        if let Ok(v) = js_ref(d, aba, &q, "etapaUsuario", json!({})).await
            && v["achou"] == json!(true)
        {
            achado = Some((q, v));
            break;
        }
    }
    let (q, v) = achado.ok_or("nenhum campo de senha nem de usuário visível na página")?;
    let campo = alvo_em(&q, v["usuario"].as_str().unwrap_or(""));
    let mut notas = preencher(d, tab, &campo, usuario, false).await?;
    notas.push("usuário preenchido na primeira etapa".into());
    match v["botao"].as_str() {
        Some(b) => {
            notas.extend(clicar(d, tab, &alvo_em(&q, b), false).await?);
        }
        None => tab.press_key("Enter").await.map_err(e)?,
    }
    let fim = Instant::now() + Duration::from_secs(15);
    while Instant::now() < fim {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if achar_login(d, tab).await?.is_some() {
            notas.push("tela da senha aberta".into());
            return Ok(notas);
        }
    }
    Err(
        "depois do usuário não apareceu campo de senha em 15 s (usuário recusado, captcha ou verificação?)"
            .into(),
    )
}

/// Depois do envio: `checar` (JS do atlas) ou, sem ele, se o campo de senha sumiu.
async fn resultado_login(d: &Arc<Daemon>, tab: &Tab, checar: Option<&str>) -> String {
    if let Some(c) = checar {
        return match tab.evaluate(c).await {
            Ok(Value::Bool(true)) => "login ok (login.checar deu true)".into(),
            Ok(v) => format!("aviso: login não confirmado: login.checar deu {v}"),
            Err(x) => format!("aviso: login.checar falhou: {x}"),
        };
    }
    match achar_login(d, tab).await {
        Ok(None) => "login ok: o campo de senha sumiu".into(),
        Ok(Some(_)) => {
            "aviso: o campo de senha continua na página (senha recusada, verificação em duas etapas ou captcha?)"
                .into()
        }
        Err(x) => format!("aviso: não deu para conferir o login: {x}"),
    }
}

/// A ação como passo de atlas: alvo pelo texto dado ou pelo nome do elemento da ref.
fn passo(acao: &Acao, dado: Option<&str>, alvo: Option<&Alvo>) -> Option<Value> {
    let nome = match (dado, alvo) {
        (Some(t), Some(al)) if eh_ref(t) && !al.nm.is_empty() => al.nm.clone(),
        (Some(t), _) => t.to_string(),
        (None, _) => String::new(),
    };
    let com = |extra: Value| {
        let mut m = json!({ "alvo": nome });
        if let (Value::Object(m), Value::Object(x)) = (&mut m, extra) {
            m.extend(x);
        }
        m
    };
    Some(match acao {
        Acao::Login { enviar, usuario, .. } => {
            let mut x = json!({});
            if let Some(t) = enviar {
                x["enviar"] = json!(t);
            }
            if usuario.is_some() {
                // O valor fica como parâmetro do fluxo.
                x["usuario"] = json!("{usuario}");
            }
            json!({ "login": x })
        }
        // Senha digitada não vai para o histórico (nem para o rascunho do atlas).
        Acao::Fill { teclas, .. } if alvo.is_some_and(|a| a.l.contains("input[password]")) => {
            let mut x = json!({ "valor": "{senha}" });
            if *teclas {
                x["teclas"] = json!(true);
            }
            json!({ "fill": com(x) })
        }
        Acao::Scroll { subir, vezes, fim } => {
            if nome.is_empty() && !*subir {
                match (fim, vezes) {
                    (false, None) => return Some(json!({ "scroll": 1 })),
                    (false, Some(n)) => return Some(json!({ "scroll": n })),
                    (true, None) => return Some(json!({ "scroll": "fim" })),
                    (true, Some(_)) => {}
                }
            }
            let mut x = json!({});
            if let Some(n) = vezes {
                x["vezes"] = json!(n);
            }
            if *fim {
                x["fim"] = json!(true);
            }
            if *subir {
                x["subir"] = json!(true);
            }
            if !nome.is_empty() {
                x["dentro"] = json!(nome);
            }
            json!({ "scroll": x })
        }
        Acao::Click { js: false } => json!({ "click": nome }),
        Acao::Click { js: true } => json!({ "click": com(json!({ "js": true })) }),
        Acao::Fill { valor, teclas } => {
            let mut x = json!({ "valor": valor });
            if *teclas {
                x["teclas"] = json!(true);
            }
            json!({ "fill": com(x) })
        }
        Acao::Pick { opcao } => json!({ "pick": com(json!({ "opcao": opcao })) }),
        Acao::Check { marcar: true } => json!({ "check": nome }),
        Acao::Check { marcar: false } => json!({ "check": com(json!({ "nao": true })) }),
        Acao::Upload { arquivos } => json!({ "upload": com(json!({ "arquivos": arquivos })) }),
        Acao::Key { teclas } if nome.is_empty() && teclas.len() == 1 => json!({ "key": teclas[0] }),
        Acao::Key { teclas } if nome.is_empty() => json!({ "key": { "teclas": teclas } }),
        Acao::Key { teclas } => json!({ "key": { "teclas": teclas, "em": nome } }),
        Acao::Download {
            pasta: None,
            js: false,
            ..
        } => json!({ "download": nome }),
        Acao::Download { pasta, js, .. } => {
            let mut x = json!({});
            if *js {
                x["js"] = json!(true);
            }
            if let Some(p) = pasta {
                x["pasta"] = json!(p);
            }
            json!({ "download": com(x) })
        }
    })
}

/// Abas abertas desde `t0` (pop-up, `target=_blank`), com o número para `nd tab`.
async fn abas_novas(d: &Arc<Daemon>, t0: Instant) -> Vec<String> {
    let novas = d.criadas_desde(t0);
    if novas.is_empty() {
        return Vec::new();
    }
    let Ok(abas) = super::comandos::paginas(d).await else {
        return Vec::new();
    };
    abas.iter()
        .enumerate()
        // A lista de downloads que o Edge abre numa aba (headless) não é da página.
        .filter(|(_, (id, _, url))| novas.contains(id) && !url.starts_with("edge://"))
        .map(|(i, (_, titulo, url))| {
            let url = if url.is_empty() { "(carregando)" } else { url };
            let titulo = if titulo.is_empty() || url.contains(titulo.as_str()) {
                String::new()
            } else {
                format!("{titulo} — ")
            };
            format!("aba nova [{}]: {titulo}{url} (nd tab {})", i + 1, i + 1)
        })
        .collect()
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

    #[test]
    fn passo_de_scroll() {
        let s = |subir, vezes, fim, alvo: Option<&str>| {
            let al = alvo.map(|n| Alvo {
                r: "e5".into(),
                q: Quadro {
                    alvo: String::new(),
                    frame: String::new(),
                },
                l: String::new(),
                nota: None,
                nm: n.into(),
            });
            passo(
                &Acao::Scroll { subir, vezes, fim },
                alvo.map(|_| "e5"),
                al.as_ref(),
            )
            .unwrap()
        };
        assert_eq!(s(false, None, false, None), json!({ "scroll": 1 }));
        assert_eq!(s(false, Some(3), false, None), json!({ "scroll": 3 }));
        assert_eq!(s(false, None, true, None), json!({ "scroll": "fim" }));
        assert_eq!(
            s(true, Some(2), false, Some("Resultados")),
            json!({ "scroll": { "vezes": 2, "subir": true, "dentro": "Resultados" } })
        );
    }
}
