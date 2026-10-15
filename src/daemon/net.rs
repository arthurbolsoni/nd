//! `nd net` (chamadas capturadas, com a forma do JSON) e `nd api` (refaz uma chamada por `fetch`
//! de dentro da página, com os cookies e a origem do site).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::Daemon;
use super::comandos::aba_atual;
use super::pagina::{self, js};
use super::rede::Chamada;

type R = Result<String, String>;

/// Tamanho máximo da forma de um JSON numa linha do `nd net`.
const MAX_FORMA: usize = 300;
/// Chaves mostradas por objeto na forma.
const MAX_CHAVES: usize = 12;
/// Profundidade da forma.
const MAX_PROF: usize = 5;
/// Chamadas listadas no resumo de rede de uma ação.
const MAX_RESUMO: usize = 6;

/// Cabeçalhos que o `fetch` não aceita ou que o navegador põe sozinho.
const CAB_FORA: &[&str] = &[
    "cookie",
    "host",
    "origin",
    "referer",
    "content-length",
    "connection",
    "accept-encoding",
    "user-agent",
    "keep-alive",
    "transfer-encoding",
    "te",
    "upgrade",
    "priority",
];

// ---------------------------------------------------------------- forma e corpo

/// Forma de um JSON: chaves e tipos, listas com o tamanho (`[{id: num, nome: str} ×20]`).
pub fn forma(v: &Value) -> String {
    let mut s = String::new();
    forma_em(v, 0, &mut s);
    if s.chars().count() > MAX_FORMA {
        let c: String = s.chars().take(MAX_FORMA - 1).collect();
        return format!("{c}…");
    }
    s
}

fn forma_em(v: &Value, prof: usize, s: &mut String) {
    match v {
        Value::Null => s.push_str("null"),
        Value::Bool(_) => s.push_str("bool"),
        Value::Number(_) => s.push_str("num"),
        Value::String(_) => s.push_str("str"),
        Value::Array(a) if a.is_empty() => s.push_str("[]"),
        Value::Array(a) => {
            if prof >= MAX_PROF {
                s.push_str(&format!("[… ×{}]", a.len()));
                return;
            }
            s.push('[');
            // Lista de objetos: as chaves de todos os itens (até 20), não só do primeiro.
            if a.iter().take(20).all(Value::is_object) {
                let mut junto = Map::new();
                for x in a.iter().take(20) {
                    for (k, val) in x.as_object().unwrap() {
                        if !junto.contains_key(k) || junto[k].is_null() {
                            junto.insert(k.clone(), val.clone());
                        }
                    }
                }
                forma_em(&Value::Object(junto), prof + 1, s);
            } else {
                forma_em(&a[0], prof + 1, s);
            }
            s.push_str(&format!(" ×{}]", a.len()));
        }
        Value::Object(m) if m.is_empty() => s.push_str("{}"),
        Value::Object(m) => {
            if prof >= MAX_PROF {
                s.push_str("{…}");
                return;
            }
            s.push('{');
            for (i, (k, val)) in m.iter().enumerate() {
                if i == MAX_CHAVES {
                    s.push_str(&format!(", …+{}", m.len() - MAX_CHAVES));
                    break;
                }
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(k);
                s.push_str(": ");
                forma_em(val, prof + 1, s);
            }
            s.push('}');
        }
    }
}

/// JSON da resposta. Aceita os prefixos anti-JSON-hijacking (`for (;;);`, `)]}'`) e vários JSON
/// em sequência, um por linha (vira uma lista; o segundo valor diz quantos eram).
pub fn ler_json(texto: &str) -> Option<(Value, usize)> {
    let mut t = texto.trim_start();
    for prefixo in ["for (;;);", ")]}'", "while(1);"] {
        if let Some(r) = t.strip_prefix(prefixo) {
            t = r.trim_start();
        }
    }
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Some((v, 1));
    }
    let partes: Vec<Value> = serde_json::Deserializer::from_str(t)
        .into_iter::<Value>()
        .collect::<Result<_, _>>()
        .ok()?;
    if partes.len() > 1 {
        let n = partes.len();
        Some((Value::Array(partes), n))
    } else {
        None
    }
}

/// Seleciona parte de um JSON: `data.itens`, `data.itens.0`, `data.itens[].nome` (aplica o resto
/// do caminho a cada item da lista).
pub fn selecionar(v: &Value, caminho: &str) -> Result<Value, String> {
    let segs: Vec<&str> = caminho.split('.').filter(|s| !s.is_empty()).collect();
    sel(v, &segs, false)
}

fn sel(v: &Value, segs: &[&str], tolerante: bool) -> Result<Value, String> {
    let Some((s, resto)) = segs.split_first() else {
        return Ok(v.clone());
    };
    let (chave, mapear) = match s.strip_suffix("[]") {
        Some(k) => (k, true),
        None => (*s, false),
    };
    let atual = if chave.is_empty() {
        v
    } else {
        let achado = match v {
            Value::Object(m) => m.get(chave),
            Value::Array(a) => chave.parse::<usize>().ok().and_then(|i| a.get(i)),
            _ => None,
        };
        match achado {
            Some(x) => x,
            None if tolerante => return Ok(Value::Null),
            None => {
                return Err(match v {
                    Value::Object(m) => format!(
                        "sem a chave {chave:?}; chaves: {}",
                        m.keys().take(30).cloned().collect::<Vec<_>>().join(", ")
                    ),
                    Value::Array(a) => format!("{chave:?} fora da lista de {} itens", a.len()),
                    _ => format!("{chave:?}: o valor não é objeto nem lista"),
                });
            }
        }
    };
    if mapear {
        let Some(a) = atual.as_array() else {
            return if tolerante {
                Ok(Value::Null)
            } else {
                Err(format!("{s}: não é lista"))
            };
        };
        Ok(Value::Array(
            a.iter().map(|x| sel(x, resto, true)).collect::<Result<_, _>>()?,
        ))
    } else {
        sel(atual, resto, tolerante)
    }
}

fn cortar(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n.saturating_sub(1)).collect::<String>())
    }
}

/// Texto de um corpo para mostrar: JSON formatado (ou a parte do caminho), cortado em `max`.
fn mostrar(texto: &str, caminho: Option<&str>, max: usize) -> Result<String, String> {
    let saida = match (ler_json(texto), caminho) {
        (Some((v, _)), Some(c)) => serde_json::to_string_pretty(&selecionar(&v, c)?).unwrap_or_default(),
        (Some((v, _)), None) => serde_json::to_string_pretty(&v).unwrap_or_default(),
        (None, Some(_)) => return Err("a resposta não é JSON; --caminho não se aplica".into()),
        (None, None) => texto.to_string(),
    };
    let total = saida.chars().count();
    if total <= max {
        Ok(saida)
    } else {
        let c: String = saida.chars().take(max).collect();
        Ok(format!(
            "{c}\n… (+{} caracteres; use --caminho, --max ou --saida)",
            total - max
        ))
    }
}

// ---------------------------------------------------------------- form urlencoded

fn decodificar(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match std::str::from_utf8(&b[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    Some(x) => {
                        out.push(x);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            x => out.push(x),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn codificar(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            x => out.push_str(&format!("%{x:02X}")),
        }
    }
    out
}

pub fn ler_form(s: &str) -> Vec<(String, String)> {
    s.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (decodificar(k), decodificar(v)),
            None => (decodificar(p), String::new()),
        })
        .collect()
}

pub fn escrever_form(pares: &[(String, String)]) -> String {
    pares
        .iter()
        .map(|(k, v)| format!("{}={}", codificar(k), codificar(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn eh_form(c: &Chamada, corpo: &str) -> bool {
    cabecalho(c, "content-type").is_some_and(|t| t.contains("x-www-form-urlencoded"))
        || (!corpo.trim_start().starts_with(['{', '[']) && corpo.contains('=') && !corpo.contains('\n'))
}

fn cabecalho<'a>(c: &'a Chamada, nome: &str) -> Option<&'a str> {
    c.cabecalhos
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(nome))
        .map(|(_, v)| v.as_str())
}

/// Nome da operação (GraphQL e afins): `operationName` no JSON, `fb_api_req_friendly_name` ou
/// `operationName` no form ou na URL.
fn operacao(c: &Chamada) -> Option<String> {
    let chaves = ["operationName", "fb_api_req_friendly_name", "queryName"];
    if let Some(corpo) = &c.enviado {
        if let Ok(v) = serde_json::from_str::<Value>(corpo) {
            for k in chaves {
                if let Some(s) = v[k].as_str() {
                    return Some(s.to_string());
                }
            }
        } else {
            for (k, v) in ler_form(corpo) {
                if chaves.contains(&k.as_str()) && !v.is_empty() {
                    return Some(v);
                }
            }
        }
    }
    let q = c.url.split_once('?')?.1;
    ler_form(q.split('#').next().unwrap_or(""))
        .into_iter()
        .find(|(k, v)| chaves.contains(&k.as_str()) && !v.is_empty())
        .map(|(_, v)| v)
}

// ---------------------------------------------------------------- linhas

fn origem(url: &str) -> String {
    match url.split_once("://") {
        Some((esq, dir)) => format!("{esq}://{}", dir.split(['/', '?', '#']).next().unwrap_or("")),
        None => String::new(),
    }
}

/// URL sem a origem quando é a da página.
fn url_curta(url: &str, origem_pagina: &str) -> String {
    let s = if !origem_pagina.is_empty() && url.starts_with(origem_pagina) {
        let r = &url[origem_pagina.len()..];
        if r.is_empty() { "/" } else { r }.to_string()
    } else {
        url.split_once("://").map_or(url, |(_, r)| r).to_string()
    };
    cortar(&s, 110)
}

pub fn tamanho(b: u64) -> String {
    if b < 1024 {
        format!("{b} B")
    } else if b < 1024 * 1024 {
        format!("{:.1} KB", b as f64 / 1024.0)
    } else {
        format!("{:.1} MB", b as f64 / 1024.0 / 1024.0)
    }
}

fn duracao(c: &Chamada) -> String {
    let d = c.fim.map_or_else(|| c.inicio.elapsed(), |f| f - c.inicio);
    if d < Duration::from_secs(1) {
        format!("{} ms", d.as_millis())
    } else {
        format!("{:.1} s", d.as_secs_f64())
    }
}

/// Uma linha por chamada: `#12 POST 200 /api/x [Op] 1.2 KB 340 ms → {forma}`.
pub fn linha(c: &Chamada, origem_pagina: &str) -> String {
    let status = match (&c.erro, c.fim, c.status) {
        (Some(e), _, _) => format!("falhou ({e})"),
        (None, None, _) => "…".to_string(),
        (None, Some(_), s) => s.to_string(),
    };
    let mut l = format!("#{} {} {status}", c.n, c.metodo);
    if c.tipo == "Document" {
        l.push_str(" doc");
    }
    l.push(' ');
    l.push_str(&url_curta(&c.url, origem_pagina));
    if let Some(op) = operacao(c) {
        l.push_str(&format!(" [{op}]"));
    }
    if let Some(s) = &c.sub {
        l.push_str(&format!(" ({})", s.tipo.replace('_', " ")));
    }
    if c.em_andamento() {
        l.push_str(&format!(" (em andamento há {})", duracao(c)));
        return l;
    }
    if c.download {
        l.push_str(" → download");
        return l;
    }
    if c.erro.is_none() {
        l.push_str(&format!(" {} {}", tamanho(c.tamanho), duracao(c)));
    }
    if c.tipo == "Document" || c.erro.is_some() {
        return l;
    }
    match &c.resposta {
        Some(t) => match ler_json(t) {
            Some((v, 1)) => l.push_str(&format!(" → {}", forma(&v))),
            Some((v, n)) => l.push_str(&format!(" → {n} JSON em sequência: {}", forma(&v[0]))),
            None if t.trim().is_empty() => l.push_str(" → (vazia)"),
            None => l.push_str(&format!(
                " → {}",
                if c.mime.is_empty() { "texto" } else { &c.mime }
            )),
        },
        None if c.binaria => l.push_str(&format!(" → binário ({})", c.mime)),
        None => {}
    }
    if c.cortada {
        l.push_str(" (cortada em 200 KB)");
    }
    l
}

/// Chamadas da aba desde `desde`, para a saída de uma ação.
pub fn resumo(d: &Daemon, aba: &str, desde: Instant, origem_pagina: &str) -> Option<String> {
    let linhas: Vec<String> = d.rede.com_chamadas(|l| {
        l.iter()
            .filter(|c| c.aba == aba && c.inicio >= desde)
            .map(|c| linha(c, origem_pagina))
            .collect()
    });
    if linhas.is_empty() {
        return None;
    }
    let n = linhas.len();
    let mut out = vec![format!("rede ({n}):")];
    out.extend(linhas.into_iter().take(MAX_RESUMO).map(|l| format!("  {l}")));
    if n > MAX_RESUMO {
        out.push(format!("  … mais {} (nd net --novas)", n - MAX_RESUMO));
    }
    Some(out.join("\n"))
}

// ---------------------------------------------------------------- nd net

#[derive(Deserialize)]
pub struct NetArgs {
    filtro: Option<String>,
    #[serde(default)]
    todas: bool,
    #[serde(default = "ultimas_padrao")]
    ultimas: usize,
    #[serde(default)]
    novas: bool,
    #[serde(default)]
    antigas: bool,
    corpo: Option<u64>,
    caminho: Option<String>,
    #[serde(default = "max_padrao")]
    max: usize,
    saida: Option<String>,
}

fn ultimas_padrao() -> usize {
    30
}

fn max_padrao() -> usize {
    4000
}

pub async fn net(d: &Arc<Daemon>, a: NetArgs) -> R {
    let tab = aba_atual(d).await?;
    let aba = tab.target_id().to_string();
    let org = origem(&tab.url().await.unwrap_or_default());
    if let Some(n) = a.corpo {
        let c = d
            .rede
            .chamada(n)
            .ok_or_else(|| format!("chamada #{n} não está mais guardada (ficam as 500 últimas)"))?;
        return detalhe(&c, a.caminho.as_deref(), a.max, a.saida.as_deref());
    }
    let desde = if a.novas {
        d.paginas.ultima_acao(&aba)
    } else {
        None
    };
    let filtro = a.filtro.as_deref().map(str::to_lowercase);
    let linhas: Vec<String> = d.rede.com_chamadas(|l| {
        // Página atual: desde o último documento principal da aba.
        let pagina = if a.antigas || a.todas {
            0
        } else {
            l.iter()
                .rev()
                .find(|c| c.aba == aba && c.principal)
                .map_or(0, |c| c.n)
        };
        let escolhidas: Vec<&Chamada> = l
            .iter()
            .filter(|c| a.todas || (c.aba == aba && c.n >= pagina))
            .filter(|c| desde.is_none_or(|t| c.inicio >= t))
            .filter(|c| {
                filtro.as_ref().is_none_or(|f| {
                    c.url.to_lowercase().contains(f)
                        || c.metodo.to_lowercase() == *f
                        || operacao(c).is_some_and(|o| o.to_lowercase().contains(f))
                })
            })
            .collect();
        let pular = escolhidas.len().saturating_sub(a.ultimas);
        let mut v: Vec<String> = Vec::new();
        if pular > 0 {
            v.push(format!("… {pular} mais antigas (--ultimas N)"));
        }
        v.extend(escolhidas[pular..].iter().map(|c| linha(c, &org)));
        v
    });
    if linhas.is_empty() {
        return Ok(
            "nenhuma chamada XHR/fetch/documento nesta página com esse filtro (--antigas, --todas)".into(),
        );
    }
    Ok(linhas.join("\n"))
}

fn detalhe(c: &Chamada, caminho: Option<&str>, max: usize, saida: Option<&str>) -> R {
    let mut out = vec![format!("#{} {} {} {}", c.n, c.metodo, c.status, c.url)];
    let mut info = vec![c.tipo.clone()];
    if !c.mime.is_empty() {
        info.push(c.mime.clone());
    }
    if c.fim.is_some() {
        info.push(tamanho(c.tamanho));
    }
    info.push(duracao(c));
    out.push(info.join(", "));
    if let Some(e) = &c.erro {
        out.push(format!("erro: {e}"));
    }
    if let Some(op) = operacao(c) {
        out.push(format!("operação: {op}"));
    }
    if let Some(corpo) = &c.enviado {
        if eh_form(c, corpo) {
            out.push("enviado (form):".into());
            for (k, v) in ler_form(corpo) {
                out.push(format!("  {k} = {}", cortar(&v, 300)));
            }
        } else {
            out.push("enviado:".into());
            out.push(mostrar(corpo, None, 2000)?);
        }
    }
    match &c.resposta {
        Some(t) => {
            if let Some(arq) = saida {
                let conteudo = match (caminho, ler_json(t)) {
                    (Some(cm), Some((v, _))) => {
                        serde_json::to_string_pretty(&selecionar(&v, cm)?).unwrap_or_default()
                    }
                    _ => t.clone(),
                };
                std::fs::write(arq, &conteudo).map_err(|e| format!("{arq}: {e}"))?;
                out.push(format!("resposta gravada em {arq} ({} bytes)", conteudo.len()));
            } else {
                if let Some((_, n)) = ler_json(t)
                    && n > 1
                {
                    out.push(format!("resposta: {n} JSON em sequência, tratados como lista"));
                } else {
                    out.push("resposta:".into());
                }
                out.push(mostrar(t, caminho, max)?);
            }
            if c.cortada {
                out.push("(resposta guardada só até 200 KB; nd api refaz a chamada inteira)".into());
            }
        }
        None if c.binaria => out.push(format!("resposta binária ({}); nd api --saida grava", c.mime)),
        None if c.tipo == "Document" => {}
        None if c.fim.is_none() => out.push("resposta: ainda em andamento".into()),
        None => out.push("resposta não guardada".into()),
    }
    Ok(out.join("\n"))
}

// ---------------------------------------------------------------- nd api

#[derive(Deserialize)]
pub struct ApiArgs {
    n: u64,
    json: Option<String>,
    #[serde(default)]
    set: Vec<String>,
    #[serde(default)]
    query: Vec<String>,
    metodo: Option<String>,
    corpo: Option<String>,
    caminho: Option<String>,
    #[serde(default = "max_padrao")]
    max: usize,
    saida: Option<String>,
}

/// Junta `extra` em `base`: objetos se fundem chave a chave, o resto substitui.
fn fundir(base: &mut Value, extra: &Value) {
    match (base, extra) {
        (Value::Object(b), Value::Object(e)) => {
            for (k, v) in e {
                match b.get_mut(k) {
                    Some(x) => fundir(x, v),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, e) => *b = e.clone(),
    }
}

/// Valor de `--set k=v`: JSON se for JSON válido (número, true, objeto...), senão texto.
fn valor_set(v: &str) -> Value {
    serde_json::from_str(v).unwrap_or_else(|_| Value::String(v.to_string()))
}

/// Põe `valor` em `caminho` (`variables.count`), criando objetos no meio.
fn definir(v: &mut Value, caminho: &[&str], valor: Value) -> Result<(), String> {
    let Some((k, resto)) = caminho.split_first() else {
        *v = valor;
        return Ok(());
    };
    if v.is_null() {
        *v = Value::Object(Map::new());
    }
    let filho = match v {
        Value::Object(m) => m.entry(k.to_string()).or_insert(Value::Null),
        Value::Array(a) => {
            let i: usize = k
                .parse()
                .map_err(|_| format!("{k:?}: índice de lista inválido"))?;
            a.get_mut(i).ok_or_else(|| format!("{k:?} fora da lista"))?
        }
        _ => return Err(format!("{k:?}: o valor no caminho não é objeto")),
    };
    definir(filho, resto, valor)
}

fn separar_set(s: &str) -> Result<(Vec<&str>, Value), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("--set {s:?}: use caminho=valor"))?;
    Ok((k.split('.').collect(), valor_set(v)))
}

/// Corpo da chamada com `--corpo`, `--json` e `--set` aplicados.
fn novo_corpo(c: &Chamada, a: &ApiArgs) -> Result<Option<String>, String> {
    let base = a.corpo.clone().or_else(|| c.enviado.clone());
    if a.json.is_none() && a.set.is_empty() {
        return Ok(base);
    }
    let base = base.ok_or("a chamada não tem corpo; --json e --set mudam o corpo")?;
    let extra = match &a.json {
        Some(j) => Some(serde_json::from_str::<Value>(j).map_err(|e| format!("--json: {e}"))?),
        None => None,
    };
    if let Ok(mut v) = serde_json::from_str::<Value>(&base) {
        if let Some(x) = &extra {
            fundir(&mut v, x);
        }
        for s in &a.set {
            let (caminho, valor) = separar_set(s)?;
            definir(&mut v, &caminho, valor)?;
        }
        return Ok(Some(v.to_string()));
    }
    if !eh_form(c, &base) {
        return Err("o corpo da chamada não é JSON nem form; use --corpo".into());
    }
    let mut pares = ler_form(&base);
    // Campo de form com JSON dentro (`variables={...}`): --json e --set entram no JSON.
    let mut por = |k: &str, caminho: &[&str], valor: Value| -> Result<(), String> {
        let i = match pares.iter().position(|(pk, _)| pk == k) {
            Some(i) => i,
            None => {
                pares.push((k.to_string(), String::new()));
                pares.len() - 1
            }
        };
        if caminho.is_empty() {
            let atual = serde_json::from_str::<Value>(&pares[i].1).ok();
            pares[i].1 = match (atual, valor) {
                (Some(mut at @ Value::Object(_)), v @ Value::Object(_)) => {
                    fundir(&mut at, &v);
                    at.to_string()
                }
                (_, Value::String(s)) => s,
                (_, v) => v.to_string(),
            };
        } else {
            let mut dentro = serde_json::from_str::<Value>(&pares[i].1)
                .map_err(|_| format!("o campo {k:?} do form não é JSON"))?;
            definir(&mut dentro, caminho, valor)?;
            pares[i].1 = dentro.to_string();
        }
        Ok(())
    };
    if let Some(Value::Object(m)) = &extra {
        for (k, v) in m {
            por(k, &[], v.clone())?;
        }
    } else if extra.is_some() {
        return Err("--json num corpo de form precisa ser um objeto".into());
    }
    for s in &a.set {
        let (caminho, valor) = separar_set(s)?;
        por(caminho[0], &caminho[1..], valor)?;
    }
    Ok(Some(escrever_form(&pares)))
}

fn nova_url(url: &str, query: &[String]) -> Result<String, String> {
    if query.is_empty() {
        return Ok(url.to_string());
    }
    let (sem_frag, frag) = match url.split_once('#') {
        Some((u, f)) => (u, Some(f)),
        None => (url, None),
    };
    let (base, q) = sem_frag.split_once('?').unwrap_or((sem_frag, ""));
    let mut pares = ler_form(q);
    for s in query {
        let (k, v) = s
            .split_once('=')
            .ok_or_else(|| format!("--query {s:?}: use nome=valor"))?;
        match pares.iter_mut().find(|(pk, _)| pk == k) {
            Some(p) => p.1 = v.to_string(),
            None => pares.push((k.to_string(), v.to_string())),
        }
    }
    let mut u = format!("{base}?{}", escrever_form(&pares));
    if let Some(f) = frag {
        u.push('#');
        u.push_str(f);
    }
    Ok(u)
}

pub async fn api(d: &Arc<Daemon>, a: ApiArgs) -> R {
    let c = d
        .rede
        .chamada(a.n)
        .ok_or_else(|| format!("chamada #{} não está mais guardada (ficam as 500 últimas)", a.n))?;
    let url = nova_url(&c.url, &a.query)?;
    let metodo = a
        .metodo
        .clone()
        .unwrap_or_else(|| c.metodo.clone())
        .to_uppercase();
    let corpo = if metodo == "GET" || metodo == "HEAD" {
        None
    } else {
        novo_corpo(&c, &a)?
    };
    let mut cab = Map::new();
    for (k, v) in &c.cabecalhos {
        let kl = k.to_ascii_lowercase();
        if k.starts_with(':') || kl.starts_with("sec-") || CAB_FORA.contains(&kl.as_str()) {
            continue;
        }
        cab.insert(k.clone(), Value::String(v.clone()));
    }
    // Na aba da chamada, se ainda está aberta; senão na atual.
    let abas = d.browser.tabs().await.map_err(|e| e.to_string())?;
    let tab = match abas.into_iter().find(|t| t.target_id() == c.aba) {
        Some(t) => t,
        None => aba_atual(d).await?,
    };
    let mut avisos = Vec::new();
    // Chamada de iframe de outro site: refeita dentro do iframe (origem e cookies dele), se ele
    // ainda existe. Worker não roda o nd.js; a chamada dele é refeita pela página.
    let iframe = c
        .sub
        .as_ref()
        .filter(|s| s.tipo == "iframe" && d.browser.connection().session_of(&s.alvo).is_some());
    let (q, org_contexto) = match iframe {
        Some(s) => {
            let q = pagina::Quadro {
                alvo: s.alvo.clone(),
                frame: s.alvo.clone(),
            };
            let u = d
                .browser
                .tab(&s.alvo)
                .await
                .map_err(|e| e.to_string())?
                .url()
                .await;
            (q, origem(&u.unwrap_or_default()))
        }
        None => {
            if let Some(s) = &c.sub {
                avisos.push(format!(
                    "aviso: chamada de {} refeita pela página da aba",
                    s.tipo.replace('_', " ")
                ));
            }
            (
                pagina::principal(d, &tab).await?,
                origem(&tab.url().await.unwrap_or_default()),
            )
        }
    };
    if origem(&url) != org_contexto {
        avisos.push(format!(
            "aviso: a {} está em {org_contexto} e a chamada vai para {}; fora da mesma origem o site pode barrar (CORS)",
            if iframe.is_some() { "chamada é do iframe, que" } else { "aba" },
            origem(&url)
        ));
    }
    let t0 = Instant::now();
    let r = js(
        d,
        &q,
        "api",
        json!({
            "url": url,
            "metodo": metodo,
            "cabecalhos": cab,
            "corpo": corpo,
            "max": if a.saida.is_some() { 100 * 1024 * 1024 } else { 5 * 1024 * 1024 },
            "base64": a.saida.is_some(),
        }),
    )
    .await?;
    if let Some(e) = r["erro"].as_str() {
        return Err(format!("fetch falhou: {e}"));
    }
    let status = r["status"].as_i64().unwrap_or(0);
    let tipo = r["tipo"].as_str().unwrap_or("").to_string();
    let bytes = r["tamanho"].as_u64().unwrap_or(0);
    // A chamada refeita também entra na captura; espera o evento chegar para dar o número dela.
    let mut nova = None;
    let fim = Instant::now() + Duration::from_millis(1500);
    while nova.is_none() && Instant::now() < fim {
        nova = d.rede.com_chamadas(|l| {
            l.iter()
                .rev()
                .find(|x| x.aba == tab.target_id() && x.inicio >= t0 && x.url == url)
                .map(|x| x.n)
        });
        if nova.is_none() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let mut out = vec![format!(
        "{metodo} {status} {} {}{}",
        if tipo.is_empty() { "-" } else { &tipo },
        tamanho(bytes),
        nova.map_or(String::new(), |n| format!(" (capturada como #{n})"))
    )];
    out.extend(avisos);
    if let Some(arq) = &a.saida {
        let b64 = r["b64"].as_str().unwrap_or("");
        let dados = base64_decodificar(b64).ok_or("resposta em base64 inválida")?;
        let conteudo = match (&a.caminho, ler_json(&String::from_utf8_lossy(&dados))) {
            (Some(cm), Some((v, _))) => serde_json::to_string_pretty(&selecionar(&v, cm)?)
                .unwrap_or_default()
                .into_bytes(),
            _ => dados,
        };
        std::fs::write(arq, &conteudo).map_err(|e| format!("{arq}: {e}"))?;
        out.push(format!("gravado em {arq} ({} bytes)", conteudo.len()));
        return Ok(out.join("\n"));
    }
    let texto = r["texto"].as_str().unwrap_or("");
    if let Some((v, _)) = ler_json(texto)
        && a.caminho.is_none()
    {
        out.push(format!("forma: {}", forma(&v)));
    }
    out.push(mostrar(texto, a.caminho.as_deref(), a.max)?);
    Ok(out.join("\n"))
}

pub fn base64_decodificar(s: &str) -> Option<Vec<u8>> {
    let mut tabela = [255u8; 256];
    for (i, c) in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        .iter()
        .enumerate()
    {
        tabela[*c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        let v = tabela[c as usize];
        if v == 255 {
            return None;
        }
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn forma_de_json() {
        let v: Value = serde_json::from_str(
            r#"{"data":{"itens":[{"id":1,"nome":"a"},{"id":2,"extra":true}],"total":2},"ok":true}"#,
        )
        .unwrap();
        assert_eq!(
            forma(&v),
            "{data: {itens: [{id: num, nome: str, extra: bool} ×2], total: num}, ok: bool}"
        );
    }

    #[test]
    fn json_com_prefixo_e_em_sequencia() {
        assert_eq!(ler_json("for (;;);{\"a\":1}").unwrap().0, json!({"a": 1}));
        let (v, n) = ler_json("{\"a\":1}\n{\"b\":2}\n").unwrap();
        assert_eq!(n, 2);
        assert_eq!(v, json!([{"a": 1}, {"b": 2}]));
        assert!(ler_json("<html>").is_none());
    }

    #[test]
    fn caminho() {
        let v = json!({"data": {"itens": [{"nome": "a"}, {"nome": "b"}, {"x": 1}]}});
        assert_eq!(selecionar(&v, "data.itens.1.nome").unwrap(), json!("b"));
        assert_eq!(
            selecionar(&v, "data.itens[].nome").unwrap(),
            json!(["a", "b", null])
        );
        assert!(
            selecionar(&v, "data.outro")
                .unwrap_err()
                .contains("chaves: itens")
        );
    }

    #[test]
    fn form_ida_e_volta() {
        let p = ler_form("a=1&variables=%7B%22count%22%3A2%7D&nome=Jo%C3%A3o+Silva");
        assert_eq!(p[1].1, "{\"count\":2}");
        assert_eq!(p[2].1, "João Silva");
        assert_eq!(ler_form(&escrever_form(&p)), p);
    }

    fn chamada(corpo: &str, tipo: &str) -> Chamada {
        Chamada {
            n: 1,
            aba: "a".into(),
            id: "1".into(),
            tipo: "Fetch".into(),
            principal: false,
            metodo: "POST".into(),
            url: "https://x.com/api?pagina=1".into(),
            status: 200,
            mime: "application/json".into(),
            cabecalhos: vec![("Content-Type".into(), tipo.into())],
            enviado: Some(corpo.into()),
            resposta: None,
            cortada: false,
            binaria: false,
            download: false,
            sub: None,
            tamanho: 0,
            inicio: Instant::now(),
            fim: None,
            erro: None,
        }
    }

    fn args(json: Option<&str>, set: &[&str]) -> ApiArgs {
        ApiArgs {
            n: 1,
            json: json.map(str::to_string),
            set: set.iter().map(|s| s.to_string()).collect(),
            query: Vec::new(),
            metodo: None,
            corpo: None,
            caminho: None,
            max: 100,
            saida: None,
        }
    }

    #[test]
    fn corpo_json_alterado() {
        let c = chamada(
            r#"{"variables":{"pagina":1,"termo":"x"},"op":"Busca"}"#,
            "application/json",
        );
        let novo = novo_corpo(
            &c,
            &args(Some(r#"{"variables":{"termo":"y"}}"#), &["variables.pagina=2"]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&novo).unwrap(),
            json!({"variables": {"pagina": 2, "termo": "y"}, "op": "Busca"})
        );
    }

    #[test]
    fn corpo_form_com_json_dentro() {
        let c = chamada(
            "fb_api_req_friendly_name=ListaQuery&variables=%7B%22count%22%3A2%7D",
            "application/x-www-form-urlencoded",
        );
        let novo = novo_corpo(&c, &args(None, &["variables.count=10", "doc_id=99"]))
            .unwrap()
            .unwrap();
        let p = ler_form(&novo);
        assert_eq!(p[1], ("variables".to_string(), "{\"count\":10}".to_string()));
        assert_eq!(p[2], ("doc_id".to_string(), "99".to_string()));
        assert_eq!(operacao(&c).as_deref(), Some("ListaQuery"));
    }

    #[test]
    fn query_alterada() {
        assert_eq!(
            nova_url(
                "https://x.com/a?pagina=1&q=b#f",
                &["pagina=2".into(), "n=3".into()]
            )
            .unwrap(),
            "https://x.com/a?pagina=2&q=b&n=3#f"
        );
    }

    #[test]
    fn base64() {
        assert_eq!(base64_decodificar("aGVsbG8gbXVuZG8=").unwrap(), b"hello mundo");
    }
}
