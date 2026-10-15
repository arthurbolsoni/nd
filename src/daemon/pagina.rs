//! A página vista pelo nd: um mundo isolado por quadro com a biblioteca `nd.js`, o mapa (com os
//! iframes de outro domínio lidos pelo target ou pelo quadro deles), as refs e o diff pós-ação.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use nodriver::{Error, Tab};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Daemon;

const ND_JS: &str = include_str!("../js/nd.js");
/// Versão da biblioteca na página; outra versão do nd (outro daemon) a substitui.
const VERSAO: usize = ND_JS.len();
static BIBLIOTECA: LazyLock<String> = LazyLock::new(|| ND_JS.replace("__VERSAO__", &VERSAO.to_string()));

/// Rede sem requisição em andamento por este tempo = ociosa.
const REDE_OCIOSA: Duration = Duration::from_millis(500);
/// DOM sem mudança de estrutura ou texto por este tempo = parado.
const DOM_PARADO_MS: u64 = 300;
/// Espera máxima pelo DOM parado depois da rede ociosa (página com conteúdo sempre mudando).
const DOM_MAX: Duration = Duration::from_secs(3);
/// Iframes de outro domínio lidos por mapa, e a profundidade máxima de aninhamento.
const MAX_QUADROS: usize = 10;
const MAX_PROFUNDIDADE: usize = 3;
/// Linhas de "apareceu" e "mudou" no diff.
const MAX_DIFF: usize = 60;
/// Linhas de texto novo no diff.
const MAX_TEXTO: usize = 12;

type R<T> = Result<T, String>;

fn e(x: impl std::fmt::Display) -> String {
    x.to_string()
}

/// Quadro (frame) de uma aba: o target que recebe os comandos (a aba, ou o iframe de outro
/// processo) e o id do frame.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Quadro {
    pub alvo: String,
    pub frame: String,
}

#[derive(Debug, Clone)]
pub struct RefInfo {
    pub quadro: Quadro,
    /// Documento do quadro quando a ref foi dada (`performance.timeOrigin`).
    pub doc: String,
    pub p: String,
    pub nm: String,
    /// Posição entre os elementos de mesmo papel e nome.
    pub k: usize,
    /// Iframe de outro domínio (o conteúdo está noutro quadro).
    pub externo: bool,
}

#[derive(Default)]
pub struct EstadoAba {
    /// Documento do quadro principal; quando muda, as refs recomeçam de e1.
    doc: String,
    prox: u64,
    frame_principal: Option<String>,
    refs: HashMap<String, RefInfo>,
    /// Quadro -> (quadro pai, ref do iframe no pai).
    pais: HashMap<Quadro, (Quadro, String)>,
    dialogos: Vec<String>,
    ultima_acao: Option<Instant>,
    /// Maior número de ref dado no documento anterior (para avisar de ref de página anterior).
    refs_anteriores: u64,
}

/// Estado das páginas por aba (target id) e os contextos isolados por quadro.
#[derive(Default)]
pub struct Paginas {
    abas: Mutex<HashMap<String, EstadoAba>>,
    contextos: Mutex<HashMap<Quadro, i64>>,
}

impl Paginas {
    fn com<T>(&self, aba: &str, f: impl FnOnce(&mut EstadoAba) -> T) -> T {
        let mut abas = self.abas.lock().unwrap();
        f(abas.entry(aba.to_string()).or_default())
    }

    pub fn remover(&self, aba: &str) {
        self.abas.lock().unwrap().remove(aba);
        self.contextos.lock().unwrap().retain(|q, _| q.alvo != aba);
    }

    pub fn dialogo(&self, aba: &str, texto: String) {
        self.com(aba, |ea| ea.dialogos.push(texto));
    }

    pub fn marcar_acao(&self, aba: &str) {
        self.com(aba, |ea| {
            ea.ultima_acao = Some(Instant::now());
            ea.dialogos.clear();
        });
    }

    pub fn ultima_acao(&self, aba: &str) -> Option<Instant> {
        self.com(aba, |ea| ea.ultima_acao)
    }

    pub fn tirar_dialogos(&self, aba: &str) -> Vec<String> {
        self.com(aba, |ea| std::mem::take(&mut ea.dialogos))
    }

    pub fn ref_info(&self, aba: &str, r: &str) -> Option<RefInfo> {
        self.com(aba, |ea| ea.refs.get(r).cloned())
    }

    /// A ref (eN) pode ter sido dada num documento anterior da aba.
    pub fn de_pagina_anterior(&self, aba: &str, r: &str) -> bool {
        let n: u64 = r[1..].parse().unwrap_or(0);
        self.com(aba, |ea| n <= ea.refs_anteriores)
    }

    pub fn pai(&self, aba: &str, q: &Quadro) -> Option<(Quadro, String)> {
        self.com(aba, |ea| ea.pais.get(q).cloned())
    }

    /// Quadros de outro domínio já vistos nesta aba.
    pub fn quadros_filhos(&self, aba: &str) -> Vec<Quadro> {
        self.com(aba, |ea| ea.pais.keys().cloned().collect())
    }

    fn prox(&self, aba: &str) -> u64 {
        self.com(aba, |ea| ea.prox.max(1))
    }

    fn avancar(&self, aba: &str, prox: Option<u64>) {
        if let Some(p) = prox {
            self.com(aba, |ea| ea.prox = ea.prox.max(p));
        }
    }
}

/// Quadro principal da aba (o frame raiz).
pub async fn principal(d: &Daemon, tab: &Tab) -> R<Quadro> {
    let aba = tab.target_id();
    if let Some(f) = d.paginas.com(aba, |ea| ea.frame_principal.clone()) {
        return Ok(Quadro {
            alvo: aba.into(),
            frame: f,
        });
    }
    let v = tab.call("Page.getFrameTree", json!({})).await.map_err(e)?;
    let f = v["frameTree"]["frame"]["id"]
        .as_str()
        .ok_or("Page.getFrameTree sem frame raiz")?
        .to_string();
    d.paginas.com(aba, |ea| ea.frame_principal = Some(f.clone()));
    Ok(Quadro {
        alvo: aba.into(),
        frame: f,
    })
}

async fn contexto(d: &Daemon, tab: &Tab, q: &Quadro) -> R<i64> {
    if let Some(c) = d.paginas.contextos.lock().unwrap().get(q) {
        return Ok(*c);
    }
    let v = tab
        .call(
            "Page.createIsolatedWorld",
            json!({ "frameId": q.frame, "worldName": "nd", "grantUniveralAccess": false }),
        )
        .await
        .map_err(|x| format!("quadro {} indisponível: {x}", q.frame))?;
    let c = v["executionContextId"]
        .as_i64()
        .ok_or("Page.createIsolatedWorld sem executionContextId")?;
    d.paginas.contextos.lock().unwrap().insert(q.clone(), c);
    Ok(c)
}

fn contexto_perdido(msg: &str) -> bool {
    msg.contains("Cannot find context")
        || msg.contains("Execution context was destroyed")
        || msg.contains("Inspected target navigated or closed")
}

/// Primeira linha da exceção, sem o prefixo "Error: ".
fn texto_excecao(ex: &Value) -> String {
    let msg = ex["exception"]["description"]
        .as_str()
        .or(ex["text"].as_str())
        .unwrap_or("erro de JavaScript");
    let l = msg.lines().next().unwrap_or(msg);
    l.strip_prefix("Error: ").unwrap_or(l).to_string()
}

/// `Runtime.evaluate` no mundo isolado do quadro; refaz o contexto uma vez se o documento trocou.
async fn avaliar(d: &Daemon, q: &Quadro, expr: &str, por_valor: bool) -> R<Value> {
    let tab = d.browser.tab(&q.alvo).await.map_err(e)?;
    for tentativa in 0..2 {
        let ctx = contexto(d, &tab, q).await?;
        let r = tab
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expr,
                    "contextId": ctx,
                    "returnByValue": por_valor,
                    "awaitPromise": true,
                    "userGesture": true,
                    "objectGroup": "nd",
                }),
            )
            .await;
        let perdido = match &r {
            Ok(v) => v
                .get("exceptionDetails")
                .is_some_and(|ex| contexto_perdido(&texto_excecao(ex))),
            Err(Error::Protocol { message, .. }) => contexto_perdido(message),
            Err(_) => false,
        };
        if perdido && tentativa == 0 {
            d.paginas.contextos.lock().unwrap().remove(q);
            continue;
        }
        let v = r.map_err(e)?;
        if let Some(ex) = v.get("exceptionDetails") {
            return Err(texto_excecao(ex));
        }
        return Ok(v);
    }
    Err("o documento da página trocou durante o comando".into())
}

fn expressao(fun: &str, args: &Value) -> String {
    format!(
        "(globalThis.__nd?.v === {VERSAO} ? globalThis.__nd : (globalThis.__nd = {})).{fun}({args})",
        *BIBLIOTECA
    )
}

/// Chama `__nd.<fun>(args)` no quadro e devolve o resultado em JSON.
pub async fn js(d: &Daemon, q: &Quadro, fun: &str, args: Value) -> R<Value> {
    let v = avaliar(d, q, &expressao(fun, &args), true).await?;
    Ok(v["result"]["value"].clone())
}

/// Como [`js`], para funções que podem dar refs novas: passa e atualiza o contador da aba.
pub async fn js_ref(d: &Daemon, aba: &str, q: &Quadro, fun: &str, mut args: Value) -> R<Value> {
    args["prox"] = json!(d.paginas.prox(aba));
    args["principal"] = json!(false);
    let v = js(d, q, fun, args).await?;
    d.paginas.avancar(aba, v["prox"].as_u64());
    Ok(v)
}

/// Handle (objectId) do elemento da ref, para comandos CDP que pedem o nó.
pub async fn handle(d: &Daemon, q: &Quadro, r: &str) -> R<String> {
    let v = avaliar(d, q, &expressao("el", &json!({ "ref": r })), false).await?;
    v["result"]["objectId"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{r} sem handle"))
}

pub async fn soltar(d: &Daemon, q: &Quadro, objeto: &str) {
    if let Ok(tab) = d.browser.tab(&q.alvo).await {
        let _ = tab
            .call("Runtime.releaseObject", json!({ "objectId": objeto }))
            .await;
    }
}

/// Quadro do conteúdo do iframe `r` (de outro domínio) que está no quadro `pai`.
async fn quadro_filho(d: &Daemon, pai: &Quadro, r: &str, iframes: &mut Option<HashSet<String>>) -> R<Quadro> {
    let obj = handle(d, pai, r).await?;
    let tab = d.browser.tab(&pai.alvo).await.map_err(e)?;
    let no = tab
        .call("DOM.describeNode", json!({ "objectId": obj }))
        .await
        .map_err(e);
    soltar(d, pai, &obj).await;
    let frame = no?["node"]["frameId"]
        .as_str()
        .ok_or("o iframe não tem frameId")?
        .to_string();
    if iframes.is_none() {
        let alvos = d.browser.targets().await.map_err(e)?;
        *iframes = Some(
            alvos
                .into_iter()
                .filter(|t| t.r#type == "iframe")
                .map(|t| t.target_id)
                .collect(),
        );
    }
    // Iframe de outro site roda noutro processo e tem target próprio (id = frame id).
    let alvo = if iframes.as_ref().unwrap().contains(&frame) {
        d.browser.tab(&frame).await.map_err(e)?;
        frame.clone()
    } else {
        pai.alvo.clone()
    };
    Ok(Quadro { alvo, frame })
}

#[derive(Debug, Clone, Deserialize)]
pub struct Item {
    #[serde(default)]
    pub r: Option<String>,
    /// el | grupo | titulo | aviso | quadro (iframe de outro domínio não lido)
    pub t: String,
    pub n: usize,
    pub l: String,
    /// -1 acima da área do mapa, 0 dentro, 1 abaixo.
    #[serde(default)]
    pub z: i8,
    /// Dentro do diálogo modal aberto.
    #[serde(default)]
    pub m: bool,
    #[serde(default)]
    pub p: Option<String>,
    #[serde(default)]
    pub nm: Option<String>,
    #[serde(default)]
    pub k: usize,
    /// Índice do quadro em `Instantaneo::quadros`.
    #[serde(skip)]
    pub q: usize,
}

#[derive(Deserialize)]
struct RespMapa {
    itens: Vec<Item>,
    modal: Option<String>,
    prox: u64,
    doc: String,
    url: String,
    titulo: String,
    #[serde(default)]
    txt: Vec<String>,
}

pub struct Instantaneo {
    pub itens: Vec<Item>,
    pub quadros: Vec<Quadro>,
    pub url: String,
    pub titulo: String,
    /// Documento do quadro principal.
    pub doc: String,
    pub modal: bool,
    /// Linhas do texto visível (de todos os quadros), quando pedidas.
    pub texto: Vec<String>,
}

#[derive(Default)]
pub struct OpMapa {
    /// Só o que está dentro desta ref.
    pub dentro: Option<String>,
    /// Destino de todos os links.
    pub href: bool,
    /// Lê também as linhas de texto visível (para o diff).
    pub texto: bool,
}

/// Lê a página (e os iframes de outro domínio) e atualiza as refs da aba.
pub async fn instantaneo(d: &Arc<Daemon>, tab: &Tab, op: &OpMapa) -> R<Instantaneo> {
    let aba = tab.target_id();
    let raiz = principal(d, tab).await?;
    let (q0, dentro) = match &op.dentro {
        None => (raiz.clone(), None),
        Some(r) => {
            let info = d
                .paginas
                .ref_info(aba, r)
                .ok_or_else(|| format!("ref {r} desconhecida; rode nd map"))?;
            if info.externo {
                let mut iframes = None;
                (quadro_filho(d, &info.quadro, r, &mut iframes).await?, None)
            } else {
                (info.quadro, Some(r.clone()))
            }
        }
    };
    let eh_raiz = q0 == raiz;
    let (doc_antigo, prox) = d.paginas.com(aba, |ea| (ea.doc.clone(), ea.prox.max(1)));
    let v = js(
        d,
        &q0,
        "mapa",
        json!({ "prox": prox, "doc": doc_antigo, "principal": eh_raiz, "dentro": dentro, "href": op.href, "texto": op.texto }),
    )
    .await?;
    let resp: RespMapa = serde_json::from_value(v).map_err(|x| format!("mapa: {x}"))?;
    if eh_raiz && resp.doc != doc_antigo {
        d.paginas.com(aba, |ea| {
            ea.doc = resp.doc.clone();
            ea.refs_anteriores = ea.refs_anteriores.max(ea.prox.saturating_sub(1));
            ea.prox = 1;
            ea.refs.clear();
            ea.pais.clear();
        });
    }
    let mut prox = resp.prox;
    let mut quadros = vec![q0.clone()];
    let mut docs = vec![resp.doc.clone()];
    let mut profundidade = vec![0usize];
    let mut itens = resp.itens;
    let mut texto = resp.txt;
    let mut modal = resp.modal.is_some();
    let mut iframes = None;
    let mut expandidos: HashSet<String> = HashSet::new();
    let mut lidos = 0;
    let mut i = 0;
    while i < itens.len() {
        if itens[i].t != "quadro" {
            i += 1;
            continue;
        }
        let iq = itens[i].q;
        let Some(r) = itens[i].r.clone() else {
            i += 1;
            continue;
        };
        if lidos >= MAX_QUADROS || profundidade[iq] >= MAX_PROFUNDIDADE {
            itens[i].l.push_str(&format!(" (não lido; nd map --dentro {r})"));
            i += 1;
            continue;
        }
        let pai = quadros[iq].clone();
        let sub = match quadro_filho(d, &pai, &r, &mut iframes).await {
            Ok(filho) => js(
                d,
                &filho,
                "mapa",
                json!({ "prox": prox, "principal": false, "href": op.href, "texto": op.texto }),
            )
            .await
            .and_then(|v| serde_json::from_value::<RespMapa>(v).map_err(e))
            .map(|s| (filho, s)),
            Err(x) => Err(x),
        };
        match sub {
            Ok((filho, s)) => {
                lidos += 1;
                prox = s.prox;
                modal |= s.modal.is_some();
                texto.extend(s.txt);
                let idx = quadros.len();
                d.paginas
                    .com(aba, |ea| ea.pais.insert(filho.clone(), (pai.clone(), r.clone())));
                quadros.push(filho);
                docs.push(s.doc);
                profundidade.push(profundidade[iq] + 1);
                let base = itens[i].n + 1;
                let (z, m) = (itens[i].z, itens[i].m);
                itens[i].t = "grupo".into();
                expandidos.insert(r.clone());
                let novos: Vec<Item> = s
                    .itens
                    .into_iter()
                    .map(|mut it| {
                        it.n += base;
                        it.z = z;
                        it.m |= m;
                        it.q = idx;
                        it
                    })
                    .collect();
                let fim = i + 1;
                itens.splice(fim..fim, novos);
            }
            Err(x) => {
                tracing::debug!("iframe {r}: {x}");
                itens[i].l.push_str(" (não lido)");
            }
        }
        i += 1;
    }

    d.paginas.com(aba, |ea| {
        ea.prox = ea.prox.max(prox);
        for it in &itens {
            let Some(r) = &it.r else { continue };
            ea.refs.insert(
                r.clone(),
                RefInfo {
                    quadro: quadros[it.q].clone(),
                    doc: docs[it.q].clone(),
                    p: it.p.clone().unwrap_or_default(),
                    nm: it.nm.clone().unwrap_or_default(),
                    k: it.k,
                    externo: it.t == "quadro" || expandidos.contains(r),
                },
            );
        }
    });

    Ok(Instantaneo {
        itens,
        quadros,
        url: resp.url,
        titulo: resp.titulo,
        doc: if eh_raiz { resp.doc } else { doc_antigo },
        modal,
        texto,
    })
}

pub struct OpFormato {
    /// Página toda: sem recorte pela área visível nem pelo diálogo modal.
    pub tudo: bool,
    pub max: usize,
}

impl Default for OpFormato {
    fn default() -> Self {
        Self {
            tudo: false,
            max: 300,
        }
    }
}

fn cortar(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

pub fn url_curta(u: &str) -> String {
    let s = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    cortar(s, 100)
}

fn linha(it: &Item, base: usize) -> String {
    let ind = "  ".repeat(it.n.saturating_sub(base));
    match &it.r {
        Some(r) => format!("{ind}{r} {}", it.l),
        None => format!("{ind}{}", it.l),
    }
}

/// Itens na área do mapa (viewport, uma tela acima e duas abaixo) e os grupos que os contêm.
fn na_janela(itens: &[&Item]) -> Vec<bool> {
    let mut manter = vec![false; itens.len()];
    let mut pilha: Vec<(usize, usize)> = Vec::new();
    for (i, it) in itens.iter().enumerate() {
        while pilha.last().is_some_and(|(n, _)| *n >= it.n) {
            pilha.pop();
        }
        if it.t == "grupo" {
            pilha.push((it.n, i));
            continue;
        }
        if it.z == 0 {
            manter[i] = true;
            for (_, g) in &pilha {
                manter[*g] = true;
            }
        }
    }
    manter
}

pub fn formatar(inst: &Instantaneo, f: &OpFormato) -> String {
    let mut out = vec![format!(
        "# {} — {}",
        cortar(&inst.titulo, 80),
        url_curta(&inst.url)
    )];
    let so_modal = inst.modal && !f.tudo;
    let base: Vec<&Item> = inst.itens.iter().filter(|it| !so_modal || it.m).collect();
    if so_modal {
        let fora = inst.itens.iter().filter(|it| !it.m && it.t == "el").count();
        out.push(format!(
            "(diálogo modal aberto: {fora} elementos do fundo omitidos; nd map --tudo mostra)"
        ));
    }
    let manter = if f.tudo || base.len() <= f.max {
        vec![true; base.len()]
    } else {
        na_janela(&base)
    };
    let acima = base
        .iter()
        .zip(&manter)
        .filter(|(it, m)| !**m && it.t == "el" && it.z < 0)
        .count();
    let abaixo = base
        .iter()
        .zip(&manter)
        .filter(|(it, m)| !**m && it.t == "el" && it.z > 0)
        .count();
    if acima > 0 {
        out.push(format!("… {acima} elementos acima (nd map --tudo)"));
    }
    let linhas: Vec<String> = base
        .iter()
        .zip(&manter)
        .filter(|(_, m)| **m)
        .map(|(it, _)| linha(it, 0))
        .collect();
    let total = linhas.len();
    out.extend(linhas.into_iter().take(f.max));
    if total > f.max {
        out.push(format!(
            "… +{} linhas (nd map --max N, --dentro <ref>)",
            total - f.max
        ));
    }
    if abaixo > 0 {
        out.push(format!(
            "… {abaixo} elementos abaixo (role a página ou nd map --tudo)"
        ));
    }
    if base.is_empty() {
        out.push("(nenhum elemento interativo visível)".into());
    }
    out.join("\n")
}

/// O que mudou na página entre dois instantâneos.
pub fn diferenca(antes: &Instantaneo, depois: &Instantaneo) -> String {
    if depois.doc != antes.doc {
        return format!("página nova\n{}", formatar(depois, &OpFormato::default()));
    }
    let mut out = Vec::new();
    if depois.url != antes.url {
        out.push(format!("url: {}", url_curta(&depois.url)));
    }
    if depois.titulo != antes.titulo {
        out.push(format!("título: {}", depois.titulo));
    }
    let avisos_antes: HashSet<&str> = antes
        .itens
        .iter()
        .filter(|it| it.t == "aviso")
        .map(|it| it.l.as_str())
        .collect();
    for it in depois.itens.iter().filter(|it| it.t == "aviso") {
        if !avisos_antes.contains(it.l.as_str()) {
            out.push(it.l.clone());
        }
    }

    let por_ref = |inst: &Instantaneo| -> HashMap<String, usize> {
        inst.itens
            .iter()
            .enumerate()
            .filter_map(|(i, it)| it.r.clone().map(|r| (r, i)))
            .collect()
    };
    let ra = por_ref(antes);
    let rb = por_ref(depois);
    // Nó trocado pelo re-render com a mesma descrição não conta como mudança.
    let mut sumidas: HashMap<&str, usize> = HashMap::new();
    for (r, i) in &ra {
        if !rb.contains_key(r) {
            *sumidas.entry(antes.itens[*i].l.as_str()).or_default() += 1;
        }
    }
    let titulos_antes: HashSet<&str> = antes
        .itens
        .iter()
        .filter(|it| it.t == "titulo")
        .map(|it| it.l.as_str())
        .collect();
    let mut novos: Vec<&Item> = Vec::new();
    let mut trocadas: HashSet<&str> = HashSet::new();
    for it in &depois.itens {
        match &it.r {
            Some(r) if !ra.contains_key(r) => {
                if let Some(c) = sumidas.get_mut(it.l.as_str())
                    && *c > 0
                {
                    *c -= 1;
                    trocadas.insert(it.l.as_str());
                    continue;
                }
                novos.push(it);
            }
            None if it.t == "titulo" && !titulos_antes.contains(it.l.as_str()) => novos.push(it),
            _ => {}
        }
    }
    if !novos.is_empty() {
        let base = novos.iter().map(|it| it.n).min().unwrap_or(0);
        for it in novos.iter().take(MAX_DIFF) {
            out.push(format!("+ {}", linha(it, base)));
        }
        if novos.len() > MAX_DIFF {
            out.push(format!("+ … mais {} (nd map)", novos.len() - MAX_DIFF));
        }
    }
    // Texto que apareceu fora dos elementos listados (mensagem, resultado, contador).
    let linhas_antes: HashSet<&str> = antes.texto.iter().map(String::as_str).collect();
    let mut nomes: Vec<&str> = depois
        .itens
        .iter()
        .filter_map(|it| it.nm.as_deref())
        .filter(|n| !n.is_empty())
        .collect();
    nomes.sort_by_key(|n| std::cmp::Reverse(n.len()));
    // Linha que é só nomes de elementos já listados (texto de botões e links lado a lado).
    let so_nomes = |l: &str| {
        let mut resto = l.to_string();
        for n in &nomes {
            if resto.contains(n) {
                resto = resto.replace(n, " ");
            }
        }
        resto.trim().is_empty()
    };
    let texto_novo: Vec<&String> = depois
        .texto
        .iter()
        .filter(|l| !linhas_antes.contains(l.as_str()) && !so_nomes(l))
        .collect();
    if !texto_novo.is_empty() {
        out.push("texto novo:".into());
        for l in texto_novo.iter().take(MAX_TEXTO) {
            out.push(format!("  {l}"));
        }
        if texto_novo.len() > MAX_TEXTO {
            out.push(format!("  … mais {} linhas (nd text)", texto_novo.len() - MAX_TEXTO));
        }
    }
    let mut foram: Vec<String> = Vec::new();
    let mut ordem: Vec<(&String, &usize)> = ra.iter().filter(|(r, _)| !rb.contains_key(*r)).collect();
    ordem.sort_by_key(|(_, i)| **i);
    let mut usadas: HashMap<&str, usize> = HashMap::new();
    for (r, i) in ordem {
        let l = antes.itens[*i].l.as_str();
        if trocadas.contains(l) {
            let c = usadas.entry(l).or_default();
            let total_trocadas = depois
                .itens
                .iter()
                .filter(|it| it.l == l && it.r.as_ref().is_some_and(|x| !ra.contains_key(x)))
                .count();
            if *c < total_trocadas {
                *c += 1;
                continue;
            }
        }
        foram.push(format!("{r} {}", cortar(l, 50)));
    }
    if !foram.is_empty() {
        let n = foram.len();
        let mostra: Vec<String> = foram.into_iter().take(8).collect();
        let resto = if n > 8 {
            format!("; … mais {}", n - 8)
        } else {
            String::new()
        };
        out.push(format!("- sumiram {n}: {}{resto}", mostra.join("; ")));
    }
    let mut mudou = 0;
    for it in &depois.itens {
        if let Some(r) = &it.r
            && let Some(i) = ra.get(r)
            && antes.itens[*i].l != it.l
        {
            mudou += 1;
            if mudou <= MAX_DIFF {
                out.push(format!("~ {r} {}", it.l));
            }
        }
    }
    if mudou > MAX_DIFF {
        out.push(format!("~ … mais {}", mudou - MAX_DIFF));
    }
    if out.is_empty() {
        "sem mudança visível".into()
    } else {
        out.join("\n")
    }
}

pub struct Espera {
    pub ms: u128,
    /// Requisições ainda abertas no fim da espera ("tipo url").
    pub pendentes: Vec<String>,
    pub dom_parado: bool,
}

impl Espera {
    pub fn resumo(&self) -> String {
        format!("estável em {:.1} s", self.ms as f64 / 1000.0)
    }

    pub fn avisos(&self) -> Vec<String> {
        let mut v = Vec::new();
        if !self.pendentes.is_empty() {
            let urls: Vec<String> = self.pendentes.iter().take(3).map(|u| cortar(u, 100)).collect();
            v.push(format!(
                "{} requisição(ões) ainda abertas: {}",
                self.pendentes.len(),
                urls.join(" | ")
            ));
        }
        if !self.dom_parado {
            v.push("o DOM ainda estava mudando".into());
        }
        v
    }
}

/// Espera a rede ficar ociosa (contando a partir de `desde`) e o DOM parar, até `max`. Se a rede
/// voltar a ter requisições enquanto o DOM assenta, espera de novo.
pub async fn estabilizar(d: &Arc<Daemon>, tab: &Tab, desde: Instant, max: Duration) -> Espera {
    let inicio = Instant::now();
    let limite = inicio + max;
    let aba = tab.target_id();
    let mut dom_parado = false;
    loop {
        let agora = Instant::now();
        if agora >= limite {
            break;
        }
        let (n, ociosa) = d.rede.situacao(aba);
        if n > 0 || ociosa.min(desde.elapsed()) < REDE_OCIOSA {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        let restante = (limite - agora).min(DOM_MAX);
        let r = match principal(d, tab).await {
            Ok(q) => {
                js(
                    d,
                    &q,
                    "quieto",
                    json!({ "ms": DOM_PARADO_MS, "max": restante.as_millis().max(1) as u64 }),
                )
                .await
            }
            Err(x) => Err(x),
        };
        match r {
            Ok(v) => {
                dom_parado = v["quieto"].as_bool().unwrap_or(false);
                tracing::debug!("dom parado {dom_parado} em {} ms", inicio.elapsed().as_millis());
                if d.rede.situacao(aba).0 == 0 {
                    break;
                }
            }
            // Documento trocando (navegação): tenta de novo.
            Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
    Espera {
        ms: inicio.elapsed().as_millis(),
        pendentes: d.rede.pendentes(aba).into_iter().map(|(u, _)| u).collect(),
        dom_parado,
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn item(r: Option<&str>, t: &str, n: usize, l: &str, z: i8) -> Item {
        Item {
            r: r.map(str::to_string),
            t: t.into(),
            n,
            l: l.into(),
            z,
            m: false,
            p: Some("button".into()),
            nm: Some(l.split('"').nth(1).unwrap_or(l).into()),
            k: 0,
            q: 0,
        }
    }

    fn inst(itens: Vec<Item>, texto: &[&str]) -> Instantaneo {
        Instantaneo {
            itens,
            quadros: Vec::new(),
            url: "https://x.com/a".into(),
            titulo: "X".into(),
            doc: "1".into(),
            modal: false,
            texto: texto.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn janela_recorta_e_mantem_o_grupo() {
        let mut itens = vec![item(Some("e1"), "el", 0, "acima", -1)];
        itens.push(item(Some("e2"), "grupo", 0, "form", -1));
        itens.push(item(Some("e3"), "el", 1, "dentro", 0));
        for i in 0..5 {
            itens.push(item(Some(&format!("e{}", 10 + i)), "el", 0, "abaixo", 1));
        }
        let s = formatar(&inst(itens, &[]), &OpFormato { tudo: false, max: 3 });
        assert_eq!(
            s,
            "# X — x.com/a\n… 1 elementos acima (nd map --tudo)\ne2 form\n  e3 dentro\n\
             … 5 elementos abaixo (role a página ou nd map --tudo)"
        );
    }

    #[test]
    fn diff_ignora_re_render_e_mostra_texto_novo() {
        let antes = inst(
            vec![
                item(Some("e1"), "el", 0, "button \"Salvar\"", 0),
                item(Some("e2"), "el", 0, "button \"Sair\"", 0),
                item(Some("e3"), "el", 0, "input \"Nome\"", 0),
            ],
            &["Salvar Sair"],
        );
        let depois = inst(
            vec![
                // e1 trocado pelo re-render (e9, mesma linha); e2 sumiu; e3 mudou; e4 novo.
                item(Some("e9"), "el", 0, "button \"Salvar\"", 0),
                item(Some("e3"), "el", 0, "input \"Nome\" = \"a\"", 0),
                item(Some("e4"), "el", 0, "button \"Ok\"", 0),
            ],
            &["Salvar Sair", "Salvo com sucesso", "Ok"],
        );
        assert_eq!(
            diferenca(&antes, &depois),
            "+ e4 button \"Ok\"\ntexto novo:\n  Salvo com sucesso\n- sumiram 1: e2 button \"Sair\"\n\
             ~ e3 input \"Nome\" = \"a\""
        );
    }
}
