//! Atlas por site (`~/nodriver/sites/<site>.yaml`) e replay de fluxos (`nd run`), sem LLM: cada
//! passo vira um comando do daemon, com o alvo por nome (nunca por ref).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::cliente::{Pedido, enviar};
use crate::estado::{self, Estado};

/// Ações aceitas num passo (`esperar` é sinônimo de `wait`).
const ACOES: &[&str] = &[
    "open", "click", "fill", "pick", "check", "upload", "key", "scroll", "wait", "esperar", "download",
    "eval", "text", "tab", "pdf", "login",
];
const ESPERA_SCROLL: Duration = Duration::from_secs(600);
/// Linhas da saída de cada passo no replay (sem `--tudo`).
const LINHAS_PASSO: usize = 8;
const LONGO: Duration = Duration::from_secs(200);

pub fn pasta() -> PathBuf {
    std::env::var_os("ND_ATLAS")
        .map(PathBuf::from)
        .unwrap_or_else(|| estado::home().join("nodriver").join("sites"))
}

fn arquivos() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(pasta())
        .map(|it| {
            it.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn nome(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Arquivo do atlas: `<site>.yaml`, ou o único cujo nome contém o trecho.
fn achar(site: &str) -> Result<PathBuf, String> {
    let todos = arquivos();
    let s = site.to_lowercase();
    let s = s.trim_start_matches("www.");
    if let Some(p) = todos
        .iter()
        .find(|p| nome(p).to_lowercase().trim_start_matches("www.") == s)
    {
        return Ok(p.clone());
    }
    let parecidos: Vec<&PathBuf> = todos
        .iter()
        .filter(|p| nome(p).to_lowercase().contains(s))
        .collect();
    match parecidos.as_slice() {
        [p] => Ok((*p).clone()),
        [] => Err(format!(
            "sem atlas para {site} em {}{}",
            pasta().display(),
            if todos.is_empty() {
                String::new()
            } else {
                format!(
                    "; existem: {}",
                    todos.iter().map(|p| nome(p)).collect::<Vec<_>>().join(", ")
                )
            }
        )),
        v => Err(format!(
            "{site} bate com vários atlas: {}",
            v.iter().map(|p| nome(p)).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// Lê o YAML como JSON (ordem das chaves mantida).
fn ler(p: &Path) -> Result<Value, String> {
    let texto = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let y: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&texto).map_err(|e| format!("{}: {e}", p.display()))?;
    serde_json::to_value(y).map_err(|e| format!("{}: {e}", p.display()))
}

// ---------------------------------------------------------------- nd atlas

pub fn atlas(site: Option<&str>, rascunho: bool) -> Result<String, String> {
    if rascunho {
        return rascunho_da_sessao();
    }
    let Some(site) = site else {
        return Ok(lista());
    };
    let p = achar(site)?;
    let texto = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    ler(&p)?;
    Ok(format!("# {}\n{}", p.display(), texto.trim_end()))
}

fn lista() -> String {
    let todos = arquivos();
    if todos.is_empty() {
        return format!("nenhum atlas em {}", pasta().display());
    }
    todos
        .iter()
        .map(|p| match ler(p) {
            Ok(v) => {
                let fluxos: Vec<String> = v["fluxos"]
                    .as_object()
                    .map(|m| m.keys().cloned().collect())
                    .unwrap_or_default();
                let mut l = nome(p);
                if let Some(a) = v["atualizado"].as_str() {
                    l.push_str(&format!(" (atualizado {a})"));
                }
                if !fluxos.is_empty() {
                    l.push_str(&format!(" — fluxos: {}", fluxos.join(", ")));
                }
                l
            }
            Err(e) => format!("{} — erro: {e}", nome(p)),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Passos feitos nesta sessão do daemon, em YAML, para virar fluxo de atlas.
fn rascunho_da_sessao() -> Result<String, String> {
    let est = estado::ler().ok_or("o daemon não está rodando; use nd start")?;
    let saida = chamar(&est, "historico", json!({}), Duration::from_secs(30))?;
    let passos: Value = serde_json::from_str(&saida).map_err(|e| e.to_string())?;
    if passos.as_array().is_none_or(|a| a.is_empty()) {
        return Ok("nenhum passo nesta sessão".into());
    }
    let yaml = serde_yaml_ng::to_string(&json!({ "passos": passos })).map_err(|e| e.to_string())?;
    Ok(format!(
        "# Passos desta sessão. Troque dados pessoais e valores que mudam por {{param}}, tire\n\
         # passos de sondagem e marque com confirmar: true o que envia, paga ou publica.\n{}",
        yaml.trim_end()
    ))
}

// ---------------------------------------------------------------- passos

#[derive(Debug, Clone)]
struct Passo {
    acao: String,
    valor: Value,
    confirmar: bool,
    opcional: bool,
}

const MODIFICADORES: &[&str] = &["confirmar", "opcional", "obs"];

fn passo(v: &Value, i: usize) -> Result<Passo, String> {
    let m = v
        .as_object()
        .ok_or_else(|| format!("passo {i}: esperado um mapa como {{click: \"Salvar\"}}"))?;
    let acoes: Vec<&String> = m
        .keys()
        .filter(|k| !MODIFICADORES.contains(&k.as_str()))
        .collect();
    let [acao] = acoes.as_slice() else {
        return Err(format!("passo {i}: uma ação por passo ({})", ACOES.join(", ")));
    };
    if !ACOES.contains(&acao.as_str()) {
        return Err(format!(
            "passo {i}: ação desconhecida {acao:?} ({})",
            ACOES.join(", ")
        ));
    }
    let mut valor = m[*acao].clone();
    let mut flag = |k: &str| {
        let fora = m.get(k).and_then(Value::as_bool).unwrap_or(false);
        let dentro = valor
            .as_object_mut()
            .and_then(|o| o.remove(k))
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        fora || dentro
    };
    let confirmar = flag("confirmar");
    let opcional = flag("opcional");
    if let Some(o) = valor.as_object_mut() {
        o.remove("obs");
    }
    Ok(Passo {
        acao: if acao.as_str() == "esperar" {
            "wait".into()
        } else {
            acao.to_string()
        },
        valor,
        confirmar,
        opcional,
    })
}

/// Troca `{nome}` pelos parâmetros conhecidos (o resto fica como está: JS com chaves).
fn substituir(v: &Value, params: &HashMap<String, String>) -> Value {
    match v {
        Value::String(s) => Value::String(trocar(s, params)),
        Value::Array(a) => Value::Array(a.iter().map(|x| substituir(x, params)).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| (k.clone(), substituir(x, params)))
                .collect(),
        ),
        x => x.clone(),
    }
}

fn trocar(s: &str, params: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut resto = s;
    while let Some(i) = resto.find('{') {
        out.push_str(&resto[..i]);
        let depois = &resto[i + 1..];
        match depois.find('}') {
            Some(j) if params.contains_key(&depois[..j]) => {
                out.push_str(&params[&depois[..j]]);
                resto = &depois[j + 1..];
            }
            _ => {
                out.push('{');
                resto = depois;
            }
        }
    }
    out.push_str(resto);
    out
}

/// Nomes `{x}` usados nos passos que parecem parâmetro (só letras, números e `_`).
fn usados(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            let mut resto = s.as_str();
            while let Some(i) = resto.find('{') {
                let depois = &resto[i + 1..];
                if let Some(j) = depois.find('}') {
                    let n = &depois[..j];
                    if !n.is_empty()
                        && n.chars().all(|c| c.is_alphanumeric() || c == '_')
                        && !out.iter().any(|x| x == n)
                    {
                        out.push(n.to_string());
                    }
                }
                resto = depois;
            }
        }
        Value::Array(a) => a.iter().for_each(|x| usados(x, out)),
        Value::Object(m) => m.values().for_each(|x| usados(x, out)),
        _ => {}
    }
}

/// Parâmetros do fluxo: declarados (lista de nomes, ou mapa nome → padrão), padrões e `-p k=v`.
fn parametros(fluxo: &Value, dados: &[String]) -> Result<HashMap<String, String>, String> {
    let mut declarados: Vec<String> = Vec::new();
    let mut valores: HashMap<String, String> = HashMap::new();
    match &fluxo["params"] {
        Value::Array(a) => declarados.extend(a.iter().filter_map(|x| x.as_str().map(str::to_string))),
        Value::Object(m) => {
            for (k, v) in m {
                declarados.push(k.clone());
                match v {
                    Value::Null => {}
                    Value::String(s) => {
                        valores.insert(k.clone(), s.clone());
                    }
                    x => {
                        valores.insert(k.clone(), x.to_string());
                    }
                }
            }
        }
        Value::Null => {}
        _ => return Err("params: use uma lista de nomes ou um mapa nome: padrão".into()),
    }
    for p in dados {
        let (k, v) = p
            .split_once('=')
            .ok_or_else(|| format!("-p {p:?}: use nome=valor"))?;
        valores.insert(k.to_string(), v.to_string());
    }
    let mut faltam: Vec<String> = declarados
        .iter()
        .filter(|k| !valores.contains_key(*k))
        .cloned()
        .collect();
    let mut nos_passos = Vec::new();
    // JavaScript tem chaves de sobra (`{a}` em desestruturação); fica fora da busca.
    for p in fluxo["passos"].as_array().into_iter().flatten() {
        if p.get("eval").is_none() {
            usados(p, &mut nos_passos);
        }
    }
    for n in nos_passos {
        if !declarados.contains(&n)
            && !valores.contains_key(&n)
            && n.chars().all(|c| c.is_ascii_lowercase() || c == '_')
        {
            // `{nome}` não declarado: provavelmente parâmetro esquecido em `params`.
            faltam.push(n);
        }
    }
    if !faltam.is_empty() {
        faltam.dedup();
        return Err(format!(
            "faltam parâmetros: {} (use -p {}=...)",
            faltam.join(", "),
            faltam[0]
        ));
    }
    Ok(valores)
}

fn alvo_de(v: &Value) -> Option<Value> {
    match v {
        Value::String(_) => Some(v.clone()),
        Value::Object(m) => m.get("alvo").or_else(|| m.get("nome")).cloned(),
        _ => None,
    }
}

fn campo(v: &Value, k: &str) -> Value {
    v.get(k).cloned().unwrap_or(Value::Null)
}

fn absoluto(s: &str) -> String {
    std::path::absolute(s).map_or_else(|_| s.to_string(), |p| p.display().to_string())
}

/// URL do passo `open`: relativa ao site do atlas.
fn url_do_site(site: &str, u: &str) -> String {
    if u.starts_with('/') {
        format!("https://{site}{u}")
    } else {
        u.to_string()
    }
}

/// O comando do daemon, os argumentos e o tempo máximo de um passo.
fn pedido(p: &Passo, site: &str) -> Result<(String, Value, Duration), String> {
    let v = &p.valor;
    let precisa_alvo = || alvo_de(v).ok_or_else(|| format!("{}: falta o alvo", p.acao));
    let sem_diff = false;
    let (cmd, args, t) = match p.acao.as_str() {
        "open" => {
            let (url, nova) = match v {
                Value::String(s) => (s.clone(), false),
                _ => (
                    campo(v, "url").as_str().ok_or("open: falta url")?.to_string(),
                    campo(v, "nova").as_bool().unwrap_or(false),
                ),
            };
            (
                "open",
                json!({ "url": url_do_site(site, &url), "nova": nova, "sem_map": true }),
                LONGO,
            )
        }
        "click" => (
            "click",
            json!({ "alvo": precisa_alvo()?, "js": campo(v, "js").as_bool().unwrap_or(false), "sem_diff": sem_diff }),
            LONGO,
        ),
        "fill" => (
            "fill",
            json!({ "alvo": precisa_alvo()?, "valor": texto(&campo(v, "valor")),
                    "teclas": campo(v, "teclas").as_bool().unwrap_or(false), "sem_diff": sem_diff }),
            LONGO,
        ),
        "pick" => (
            "pick",
            json!({ "alvo": precisa_alvo()?, "opcao": texto(&campo(v, "opcao")), "sem_diff": sem_diff }),
            LONGO,
        ),
        "check" => (
            "check",
            json!({ "alvo": precisa_alvo()?, "desmarcar": campo(v, "nao").as_bool().unwrap_or(false), "sem_diff": sem_diff }),
            LONGO,
        ),
        "upload" => {
            let lista: Vec<String> = match (campo(v, "arquivos"), campo(v, "arquivo")) {
                (Value::Array(a), _) => a.iter().map(texto).collect(),
                (Value::String(s), _) | (_, Value::String(s)) => vec![s],
                _ => return Err("upload: falta arquivo".into()),
            };
            let lista: Vec<String> = lista.iter().map(|s| absoluto(s)).collect();
            (
                "upload",
                json!({ "alvo": precisa_alvo()?, "arquivos": lista, "sem_diff": sem_diff }),
                LONGO,
            )
        }
        "key" => {
            let (teclas, em) = match v {
                Value::String(s) => (vec![s.clone()], Value::Null),
                Value::Array(a) => (a.iter().map(texto).collect(), Value::Null),
                _ => (
                    match campo(v, "teclas") {
                        Value::Array(a) => a.iter().map(texto).collect(),
                        Value::String(s) => vec![s],
                        _ => return Err("key: falta teclas".into()),
                    },
                    campo(v, "em"),
                ),
            };
            (
                "key",
                json!({ "alvo": em, "teclas": teclas, "sem_diff": sem_diff }),
                LONGO,
            )
        }
        "login" => {
            // `login: {}`, `login: Entrar` (botão) ou `{enviar, checar}`.
            let enviar = match v {
                Value::String(s) => Value::String(s.clone()),
                _ => campo(v, "enviar"),
            };
            (
                "login",
                json!({ "enviar": enviar, "checar": campo(v, "checar"), "sem_diff": false }),
                LONGO,
            )
        }
        "scroll" => {
            // `scroll: 3`, `scroll: fim` ou `{vezes, fim, dentro, subir}`.
            let vezes_de = |x: &Value| -> Result<Option<u64>, String> {
                match x {
                    Value::Null => Ok(None),
                    Value::Number(n) => n
                        .as_u64()
                        .map(Some)
                        .ok_or_else(|| format!("scroll: vezes inválido: {n}")),
                    Value::String(s) => s
                        .trim()
                        .parse::<u64>()
                        .map(Some)
                        .map_err(|_| format!("scroll: vezes inválido: {s:?}")),
                    outro => Err(format!("scroll: vezes inválido: {outro}")),
                }
            };
            let (vezes, fim, dentro, subir) = match v {
                Value::String(s) if s.trim() == "fim" => (None, true, Value::Null, false),
                Value::Null => (None, false, Value::Null, false),
                Value::Object(_) => (
                    vezes_de(&campo(v, "vezes"))?,
                    campo(v, "fim").as_bool().unwrap_or(false),
                    match campo(v, "dentro") {
                        Value::Null => alvo_de(v).unwrap_or(Value::Null),
                        x => x,
                    },
                    campo(v, "subir").as_bool().unwrap_or(false),
                ),
                x => (vezes_de(x)?, false, Value::Null, false),
            };
            (
                "scroll",
                json!({ "alvo": dentro, "vezes": vezes, "fim": fim, "subir": subir, "sem_diff": sem_diff }),
                ESPERA_SCROLL,
            )
        }
        "wait" => {
            let (tipo, valor) = [
                ("text", "text"),
                ("texto", "text"),
                ("sel", "sel"),
                ("url", "url"),
                ("net", "net"),
            ]
            .iter()
            .find_map(|(k, t)| v.get(*k).map(|x| (*t, texto(x))))
            .ok_or("wait: use texto, sel, url ou net")?;
            let max = campo(v, "max").as_u64().unwrap_or(15);
            (
                "wait",
                json!({ "tipo": tipo, "valor": valor, "sumir": campo(v, "sumir").as_bool().unwrap_or(false), "max": max }),
                Duration::from_secs(max + 30),
            )
        }
        "download" => {
            let max = campo(v, "max").as_u64().unwrap_or(30);
            let pasta = campo(v, "pasta").as_str().map(absoluto);
            let saida = campo(v, "saida").as_str().map(absoluto);
            (
                "download",
                json!({ "alvo": alvo_de(v), "url": campo(v, "url"), "pasta": pasta, "saida": saida, "max": max,
                        "js": campo(v, "js").as_bool().unwrap_or(false), "sem_diff": sem_diff }),
                Duration::from_secs(max + 60),
            )
        }
        "eval" => ("eval", json!({ "js": texto(v) }), LONGO),
        "text" => {
            let sel = match v {
                Value::String(s) => Some(s.clone()),
                _ => campo(v, "sel").as_str().map(str::to_string),
            };
            (
                "text",
                json!({ "sel": sel, "max": campo(v, "max").as_u64().unwrap_or(8000) }),
                LONGO,
            )
        }
        "tab" => ("tab", json!({ "alvo": texto(v) }), Duration::from_secs(30)),
        "pdf" => {
            let (saida, paisagem) = match v {
                Value::String(s) => (s.clone(), false),
                _ => (
                    texto(&campo(v, "saida")),
                    campo(v, "paisagem").as_bool().unwrap_or(false),
                ),
            };
            (
                "pdf",
                json!({ "saida": absoluto(&saida), "paisagem": paisagem }),
                LONGO,
            )
        }
        outro => return Err(format!("ação desconhecida {outro}")),
    };
    Ok((cmd.to_string(), args, t))
}

fn texto(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        x => x.to_string(),
    }
}

fn descrever(p: &Passo) -> String {
    let v = match &p.valor {
        Value::String(s) => format!("{s:?}"),
        x => x.to_string(),
    };
    let v: String = if v.chars().count() > 120 {
        format!("{}…", v.chars().take(119).collect::<String>())
    } else {
        v
    };
    format!("{} {v}", p.acao)
}

fn chamar(est: &Estado, cmd: &str, args: Value, t: Duration) -> Result<String, String> {
    let r = enviar(
        est,
        &Pedido {
            cmd: cmd.into(),
            args,
        },
        t,
    )?;
    if r.ok { Ok(r.saida) } else { Err(r.erro) }
}

fn recortar(s: &str, n: usize) -> String {
    let linhas: Vec<&str> = s.lines().collect();
    if linhas.len() <= n {
        return s.trim_end().to_string();
    }
    format!("{}\n… (+{} linhas)", linhas[..n].join("\n"), linhas.len() - n)
}

fn recuar(s: &str) -> String {
    s.lines().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n")
}

// ---------------------------------------------------------------- nd run

pub struct OpRun {
    pub site: String,
    pub fluxo: String,
    pub params: Vec<String>,
    pub desde: usize,
    pub ate: Option<usize>,
    pub confirmado: bool,
    pub seco: bool,
    pub tudo: bool,
}

pub fn run(op: &OpRun) -> Result<String, String> {
    let arq = achar(&op.site)?;
    let atlas = ler(&arq)?;
    let site = atlas["site"].as_str().map_or_else(|| nome(&arq), str::to_string);
    let fluxos = atlas["fluxos"]
        .as_object()
        .ok_or_else(|| format!("{} não tem fluxos", arq.display()))?;
    let fluxo = fluxos.get(&op.fluxo).ok_or_else(|| {
        format!(
            "fluxo {:?} não existe em {}; fluxos: {}",
            op.fluxo,
            nome(&arq),
            fluxos.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    })?;
    let params = parametros(fluxo, &op.params)?;
    let brutos = fluxo["passos"].as_array().ok_or("o fluxo não tem passos")?;
    let passos: Vec<Passo> = brutos
        .iter()
        .enumerate()
        .map(|(i, v)| passo(&substituir(v, &params), i + 1))
        .collect::<Result<_, _>>()?;
    let mut pedidos: Vec<(String, Value, Duration)> = passos
        .iter()
        .map(|p| pedido(p, &site))
        .collect::<Result<_, _>>()?;
    // Passo `login` sem `checar` usa o `login.checar` do atlas.
    for (cmd, args, _) in pedidos.iter_mut() {
        if cmd == "login" && args["checar"].is_null() {
            args["checar"] = atlas["login"]["checar"].clone();
        }
    }
    let total = passos.len();
    let ate = op.ate.unwrap_or(total).min(total);
    let desde = op.desde.max(1);
    if desde > ate {
        return Err(format!("--desde {desde} passa do último passo ({ate})"));
    }

    if op.seco {
        let linhas: Vec<String> = passos
            .iter()
            .enumerate()
            .filter(|(i, _)| *i + 1 >= desde && *i < ate)
            .map(|(i, p)| {
                let mut l = format!("[{}/{total}] {}", i + 1, descrever(p));
                if p.confirmar {
                    l.push_str(" (confirmar)");
                }
                if p.opcional {
                    l.push_str(" (opcional)");
                }
                l
            })
            .collect();
        return Ok(linhas.join("\n"));
    }

    let est = estado::ler().ok_or("o daemon não está rodando; use nd start")?;
    let checar_login = fluxo["login"].as_bool().unwrap_or(false);
    let login = &atlas["login"];
    let mut login_ok = !checar_login || login["checar"].as_str().is_none();
    let retomar = |n: usize| {
        let mut c = format!("nd run {} {}", op.site, op.fluxo);
        for p in &op.params {
            c.push_str(&format!(" -p {}", aspas(p)));
        }
        c.push_str(&format!(" --desde {n}"));
        c
    };
    let inicio = Instant::now();
    let mut out: Vec<String> = Vec::new();
    for i in (desde - 1)..ate {
        let p = &passos[i];
        let n = i + 1;
        if p.confirmar && !op.confirmado {
            out.push(format!(
                "[{n}/{total}] {} pede confirmação; o fluxo parou antes dele.\n\
                 Depois do ok do usuário: {} --confirmado",
                descrever(p),
                retomar(n)
            ));
            return Ok(out.join("\n"));
        }
        let (cmd, args, t) = &pedidos[i];
        out.push(format!("[{n}/{total}] {}", descrever(p)));
        match chamar(&est, cmd, args.clone(), *t) {
            Ok(s) => {
                let s = if op.tudo || cmd == "text" || cmd == "eval" {
                    s.trim_end().to_string()
                } else {
                    recortar(&s, LINHAS_PASSO)
                };
                if !s.is_empty() {
                    out.push(recuar(&s));
                }
            }
            Err(e) if p.opcional => out.push(format!("  (opcional) {}", e.lines().next().unwrap_or(""))),
            Err(e) => {
                println!("{}", out.join("\n"));
                return Err(falha(&est, n, total, p, &e, &retomar(n)));
            }
        }
        if !login_ok && cmd == "open" {
            let chk = login["checar"].as_str().unwrap_or("true");
            let v = chamar(&est, "eval", json!({ "js": chk }), Duration::from_secs(30)).unwrap_or_default();
            if v.trim() != "true" {
                println!("{}", out.join("\n"));
                let url = login["url"]
                    .as_str()
                    .map(|u| format!(" ({u})"))
                    .unwrap_or_default();
                return Err(format!(
                    "não há login em {site}{url}: login.checar deu {}. Entre na conta no Edge do nd e rode {}",
                    v.trim(),
                    retomar(n + 1)
                ));
            }
            login_ok = true;
        }
    }
    let fim = if ate < total {
        format!("parado no passo {ate} de {total} (--ate)")
    } else {
        format!("fluxo {} concluído", op.fluxo)
    };
    out.push(format!("{fim} em {:.1} s", inicio.elapsed().as_secs_f64()));
    Ok(out.join("\n"))
}

fn aspas(s: &str) -> String {
    if s.contains([' ', '"', '\'']) {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// Relato de passo que falhou: erro, mapa e rede recente, e como retomar.
fn falha(est: &Estado, n: usize, total: usize, p: &Passo, erro: &str, retomar: &str) -> String {
    let mut r = vec![
        format!("passo {n}/{total} falhou: {}", descrever(p)),
        erro.to_string(),
    ];
    if let Ok(m) = chamar(est, "map", json!({}), LONGO) {
        r.push(format!("--- nd map\n{}", recortar(&m, 80)));
    }
    if let Ok(net) = chamar(est, "net", json!({ "novas": true, "ultimas": 10 }), LONGO) {
        r.push(format!("--- nd net --novas\n{}", net.trim_end()));
    }
    r.push(format!("retomar: {retomar}"));
    r.join("\n")
}

#[cfg(test)]
mod testes {
    use super::*;

    fn y(s: &str) -> Value {
        serde_json::to_value(serde_yaml_ng::from_str::<serde_yaml_ng::Value>(s).unwrap()).unwrap()
    }

    #[test]
    fn passos_e_modificadores() {
        let p = passo(&y("click: { nome: Enviar, confirmar: true }"), 1).unwrap();
        assert_eq!(p.acao, "click");
        assert!(p.confirmar);
        assert_eq!(alvo_de(&p.valor), Some(json!("Enviar")));
        let p = passo(&y("{esperar: {texto: Obrigado}, opcional: true}"), 2).unwrap();
        assert_eq!(p.acao, "wait");
        assert!(p.opcional);
        assert!(passo(&y("{click: a, fill: b}"), 3).is_err());
        assert!(passo(&y("{voar: a}"), 4).unwrap_err().contains("desconhecida"));
    }

    #[test]
    fn parametros_e_troca() {
        let f = y(
            "params: {url: null, nome: Arthur}\npassos:\n  - open: '{url}'\n  - eval: '(() => { const {a} = x; })()'",
        );
        assert!(parametros(&f, &[]).unwrap_err().contains("url"));
        let p = parametros(&f, &["url=https://a.com/x".into()]).unwrap();
        assert_eq!(p["nome"], "Arthur");
        assert_eq!(
            trocar("{url}?q={nome}&{outro}", &p),
            "https://a.com/x?q=Arthur&{outro}"
        );
        // `{nome}` sem declarar também é parâmetro; JS com chaves e espaços não.
        let f = y("passos:\n  - fill: {alvo: Cidade, valor: '{cidade}'}\n  - eval: 'x => { return 1 }'");
        assert!(parametros(&f, &[]).unwrap_err().contains("cidade"));
    }

    #[test]
    fn pedidos() {
        let p = passo(&y("open: /marketplace"), 1).unwrap();
        let (c, a, _) = pedido(&p, "www.facebook.com").unwrap();
        assert_eq!(c, "open");
        assert_eq!(a["url"], "https://www.facebook.com/marketplace");
        let (c, a, _) = pedido(&passo(&y("key: {teclas: Enter, em: Busca}"), 1).unwrap(), "x").unwrap();
        assert_eq!(
            (c.as_str(), &a["alvo"], &a["teclas"]),
            ("key", &json!("Busca"), &json!(["Enter"]))
        );
        let (c, a, _) = pedido(&passo(&y("wait: {texto: ok, max: 5}"), 1).unwrap(), "x").unwrap();
        assert_eq!(
            (c.as_str(), &a["tipo"], &a["max"]),
            ("wait", &json!("text"), &json!(5))
        );
        let (_, a, _) = pedido(&passo(&y("check: {nome: Termos, nao: true}"), 1).unwrap(), "x").unwrap();
        assert_eq!(a["desmarcar"], true);
    }

    #[test]
    fn pedidos_de_scroll() {
        let s = |t: &str| pedido(&passo(&y(t), 1).unwrap(), "x");
        let (c, a, _) = s("scroll: 3").unwrap();
        assert_eq!(
            (c.as_str(), &a["vezes"], &a["fim"]),
            ("scroll", &json!(3), &json!(false))
        );
        let (_, a, _) = s("scroll: fim").unwrap();
        assert_eq!(
            (&a["vezes"], &a["fim"], &a["alvo"]),
            (&json!(null), &json!(true), &json!(null))
        );
        // Valor vindo de parâmetro chega como texto.
        let (_, a, _) = s("scroll: '5'").unwrap();
        assert_eq!(a["vezes"], 5);
        let (_, a, _) = s("scroll: {vezes: 2, dentro: Resultados, subir: true}").unwrap();
        assert_eq!(
            (&a["vezes"], &a["alvo"], &a["subir"], &a["fim"]),
            (&json!(2), &json!("Resultados"), &json!(true), &json!(false))
        );
        let (_, a, _) = s("scroll: {fim: true, vezes: 50}").unwrap();
        assert_eq!((&a["vezes"], &a["fim"]), (&json!(50), &json!(true)));
        assert!(s("scroll: muito").unwrap_err().contains("vezes"));
    }
}
