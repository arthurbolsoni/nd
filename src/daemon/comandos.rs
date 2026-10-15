//! Comandos atendidos pelo daemon. Cada um devolve o texto que o cliente imprime.

use std::fmt::Display;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nodriver::{Error, Tab};
use serde::Deserialize;
use serde_json::{Value, json};

use super::acoes::{self, Acao, AcaoArgs};
use super::pagina::{self, OpFormato, OpMapa};
use super::{Daemon, preparar_aba};
use crate::cliente::Pedido;

/// Espera máxima por rede ociosa + DOM parado depois de uma navegação.
const ESPERA_MAX: Duration = Duration::from_secs(10);

type R = Result<String, String>;

fn e(x: impl Display) -> String {
    x.to_string()
}

fn args<T: for<'de> Deserialize<'de>>(p: &Pedido) -> Result<T, String> {
    let v = if p.args.is_null() {
        Value::Object(Default::default())
    } else {
        p.args.clone()
    };
    serde_json::from_value(v).map_err(|x| format!("argumentos de {}: {x}", p.cmd))
}

pub async fn executar(d: &Arc<Daemon>, p: &Pedido) -> R {
    match p.cmd.as_str() {
        "status" => status(d).await,
        "stop" => {
            d.parar.notify_one();
            Ok("encerrando o daemon e o Edge".into())
        }
        "open" => open(d, args(p)?).await,
        "tabs" => tabs(d).await,
        "tab" => tab(d, args(p)?).await,
        "eval" => eval(d, args(p)?).await,
        "text" => text(d, args(p)?).await,
        "map" => acoes::map(d, args(p)?).await,
        "click" => {
            let x: ClickArgs = args(p)?;
            acoes::agir(d, x.base, Acao::Click { js: x.js }).await
        }
        "fill" => {
            let x: FillArgs = args(p)?;
            acoes::agir(
                d,
                x.base,
                Acao::Fill {
                    valor: x.valor,
                    teclas: x.teclas,
                },
            )
            .await
        }
        "pick" => {
            let x: PickArgs = args(p)?;
            acoes::agir(d, x.base, Acao::Pick { opcao: x.opcao }).await
        }
        "check" => {
            let x: CheckArgs = args(p)?;
            acoes::agir(d, x.base, Acao::Check { marcar: !x.desmarcar }).await
        }
        "upload" => {
            let x: UploadArgs = args(p)?;
            acoes::agir(d, x.base, Acao::Upload { arquivos: x.arquivos }).await
        }
        "key" => {
            let x: KeyArgs = args(p)?;
            acoes::agir(d, x.base, Acao::Key { teclas: x.teclas }).await
        }
        "wait" => acoes::wait(d, args(p)?).await,
        outro => Err(format!("comando desconhecido: {outro}")),
    }
}

/// Aba em que os comandos atuam; se a anterior fechou, a primeira aba aberta.
pub async fn aba_atual(d: &Arc<Daemon>) -> Result<Tab, String> {
    let atual = d.atual.lock().unwrap().clone();
    let tabs = d.browser.tabs().await.map_err(e)?;
    if let Some(id) = atual
        && let Some(t) = tabs.iter().find(|t| t.target_id() == id)
    {
        return Ok(t.clone());
    }
    let t = tabs.into_iter().next().ok_or("nenhuma aba aberta")?;
    *d.atual.lock().unwrap() = Some(t.target_id().to_string());
    preparar_aba(d, t.target_id()).await.map_err(e)?;
    Ok(t)
}

async fn status(d: &Arc<Daemon>) -> R {
    let mut s = String::new();
    let op = &d.opcoes;
    s.push_str(&format!("daemon pid {}\n", std::process::id()));
    s.push_str(&format!(
        "edge pid {}\n",
        d.browser.pid().map_or("?".into(), |p| p.to_string())
    ));
    s.push_str(&format!("perfil {}\n", op.perfil));
    s.push_str(&format!(
        "modo {}{}\n",
        if op.oculto { "desktop oculto" } else { "visível" },
        if op.headless { ", headless" } else { "" }
    ));
    s.push_str(&format!("idioma {}\n", op.lang));
    let abas = paginas(d).await?;
    s.push_str(&format!("abas {}\n", abas.len()));
    if let Ok(t) = aba_atual(d).await
        && let Some((i, info)) = abas.iter().enumerate().find(|(_, a)| a.0 == t.target_id())
    {
        s.push_str(&format!("atual [{}] {} — {}", i + 1, info.1, info.2));
    }
    Ok(s.trim_end().to_string())
}

/// (target id, título, url) das abas, na ordem do navegador.
async fn paginas(d: &Daemon) -> Result<Vec<(String, String, String)>, String> {
    Ok(d.browser
        .targets()
        .await
        .map_err(e)?
        .into_iter()
        .filter(|t| t.r#type == "page")
        .map(|t| (t.target_id, t.title, t.url))
        .collect())
}

#[derive(Deserialize)]
struct ClickArgs {
    #[serde(flatten)]
    base: AcaoArgs,
    #[serde(default)]
    js: bool,
}

#[derive(Deserialize)]
struct FillArgs {
    #[serde(flatten)]
    base: AcaoArgs,
    valor: String,
    #[serde(default)]
    teclas: bool,
}

#[derive(Deserialize)]
struct PickArgs {
    #[serde(flatten)]
    base: AcaoArgs,
    opcao: String,
}

#[derive(Deserialize)]
struct CheckArgs {
    #[serde(flatten)]
    base: AcaoArgs,
    #[serde(default)]
    desmarcar: bool,
}

#[derive(Deserialize)]
struct UploadArgs {
    #[serde(flatten)]
    base: AcaoArgs,
    arquivos: Vec<String>,
}

#[derive(Deserialize)]
struct KeyArgs {
    #[serde(flatten)]
    base: AcaoArgs,
    teclas: Vec<String>,
}

#[derive(Deserialize)]
struct OpenArgs {
    url: String,
    #[serde(default)]
    nova: bool,
    /// Não imprime o mapa da página depois de carregar.
    #[serde(default)]
    sem_map: bool,
}

async fn open(d: &Arc<Daemon>, a: OpenArgs) -> R {
    let url = normalizar_url(&a.url);
    let tab = if a.nova {
        let t = d.browser.new_tab("about:blank").await.map_err(e)?;
        preparar_aba(d, t.target_id()).await.map_err(e)?;
        *d.atual.lock().unwrap() = Some(t.target_id().to_string());
        let _ = t.activate().await;
        t
    } else {
        aba_atual(d).await?
    };
    d.rede.limpar(tab.target_id());
    let t0 = Instant::now();
    let mut avisos = Vec::new();
    if let Some(av) = navegar(&tab, &url).await? {
        avisos.push(av);
    }
    let espera = pagina::estabilizar(d, &tab, t0, ESPERA_MAX).await;
    avisos.extend(espera.avisos());
    let mut extra = vec![espera.resumo()];
    extra.extend(avisos.into_iter().map(|av| format!("aviso: {av}")));
    if a.sem_map {
        let titulo = tab.title().await.unwrap_or_default();
        let atual = tab.url().await.unwrap_or_default();
        return Ok(format!("{titulo}\n{atual}\n{}", extra.join("\n")));
    }
    // Mapa logo depois de abrir: o cabeçalho dele já traz título e URL.
    let inst = pagina::instantaneo(d, &tab, &OpMapa::default()).await?;
    let mapa = pagina::formatar(&inst, &OpFormato::default());
    let (cab, resto) = mapa.split_once('\n').unwrap_or((&mapa, ""));
    Ok(format!("{cab}\n{}\n{resto}", extra.join("\n"))
        .trim_end()
        .to_string())
}

/// Navega e espera o documento novo entrar (commit) e o DOMContentLoaded (até 3 s); o resto, a
/// estabilização espera. Devolve um aviso se o commit não veio.
async fn navegar(tab: &Tab, url: &str) -> Result<Option<String>, String> {
    // Aba recém-criada pode ainda não ter o Page ligado (o preparo dela corre em paralelo).
    tab.call("Page.enable", json!({})).await.map_err(e)?;
    let mut navegou = tab.subscribe_raw(Some("Page.frameNavigated"));
    let mut dcl = tab.subscribe_raw(Some("Page.domContentEventFired"));
    let r = tab
        .call("Page.navigate", json!({ "url": url }))
        .await
        .map_err(e)?;
    if let Some(erro) = r["errorText"].as_str() {
        return Err(format!("{url}: {erro}"));
    }
    let Some(loader) = r["loaderId"].as_str() else {
        // Navegação no mesmo documento (âncora).
        return Ok(None);
    };
    let espera = async {
        while let Some(ev) = navegou.next().await {
            let f = &ev.params["frame"];
            if f["parentId"].is_null() && f["loaderId"].as_str() == Some(loader) {
                return true;
            }
        }
        false
    };
    match tokio::time::timeout(Duration::from_secs(30), espera).await {
        Ok(true) => {}
        Ok(false) => return Err(e(Error::Disconnected)),
        Err(_) => return Ok(Some("o documento novo não entrou em 30 s".into())),
    }
    // Dá ao parser até 3 s para o DOMContentLoaded; página em streaming (Facebook) passa daqui
    // com o HTML ainda chegando.
    let _ = tokio::time::timeout(Duration::from_secs(3), dcl.next()).await;
    Ok(None)
}

fn normalizar_url(url: &str) -> String {
    let u = url.trim();
    if u.contains("://") || u.starts_with("about:") || u.starts_with("data:") || u.starts_with("file:") {
        u.to_string()
    } else {
        format!("https://{u}")
    }
}

async fn tabs(d: &Arc<Daemon>) -> R {
    let atual = aba_atual(d).await.ok().map(|t| t.target_id().to_string());
    let abas = paginas(d).await?;
    let linhas: Vec<String> = abas
        .iter()
        .enumerate()
        .map(|(i, (id, titulo, url))| {
            let marca = if atual.as_deref() == Some(id.as_str()) {
                "*"
            } else {
                " "
            };
            format!("{marca} [{}] {} — {}", i + 1, cortar(titulo, 70), url)
        })
        .collect();
    Ok(linhas.join("\n"))
}

#[derive(Deserialize)]
struct TabArgs {
    alvo: String,
}

async fn tab(d: &Arc<Daemon>, a: TabArgs) -> R {
    let abas = paginas(d).await?;
    let escolhida = match a.alvo.parse::<usize>() {
        Ok(n) if n >= 1 && n <= abas.len() => Some(n - 1),
        Ok(n) => return Err(format!("não há aba {n}; abertas: {}", abas.len())),
        Err(_) => {
            let busca = a.alvo.to_lowercase();
            abas.iter()
                .position(|(_, t, u)| t.to_lowercase().contains(&busca) || u.to_lowercase().contains(&busca))
        }
    };
    let i = escolhida.ok_or_else(|| format!("nenhuma aba com {:?} no título ou na URL", a.alvo))?;
    let (id, titulo, url) = &abas[i];
    let t = preparar_aba(d, id).await.map_err(e)?;
    let _ = t.activate().await;
    *d.atual.lock().unwrap() = Some(id.clone());
    Ok(format!("* [{}] {} — {}", i + 1, cortar(titulo, 70), url))
}

#[derive(Deserialize)]
struct EvalArgs {
    js: String,
}

async fn eval(d: &Arc<Daemon>, a: EvalArgs) -> R {
    let tab = aba_atual(d).await?;
    let v = tab.evaluate(&a.js).await.map_err(e)?;
    Ok(match v {
        Value::String(s) => s,
        Value::Null => "null".into(),
        outro => serde_json::to_string_pretty(&outro).unwrap_or_default(),
    })
}

#[derive(Deserialize)]
struct TextArgs {
    sel: Option<String>,
    #[serde(default = "max_texto")]
    max: usize,
}

fn max_texto() -> usize {
    8000
}

async fn text(d: &Arc<Daemon>, a: TextArgs) -> R {
    let tab = aba_atual(d).await?;
    let sel = serde_json::to_string(&a.sel).unwrap();
    let expr = format!(
        "((sel) => {{ const el = sel ? document.querySelector(sel) : document.body; \
         return el ? (el.innerText ?? el.textContent ?? '') : null; }})({sel})"
    );
    let v = tab.evaluate(&expr).await.map_err(e)?;
    let Some(texto) = v.as_str() else {
        return Err(format!("nenhum elemento casa com {}", a.sel.unwrap_or_default()));
    };
    let limpo = limpar_texto(texto);
    let total = limpo.chars().count();
    if total <= a.max {
        return Ok(limpo);
    }
    let corte: String = limpo.chars().take(a.max).collect();
    Ok(format!("{corte}\n… (+{} caracteres; use --max)", total - a.max))
}

/// Tira espaços no fim das linhas e junta linhas em branco seguidas numa só.
fn limpar_texto(t: &str) -> String {
    let mut out = Vec::new();
    let mut branco = false;
    for l in t.lines() {
        let l = l.trim_end();
        if l.trim().is_empty() {
            if !branco && !out.is_empty() {
                out.push(String::new());
            }
            branco = true;
        } else {
            out.push(l.to_string());
            branco = false;
        }
    }
    out.join("\n").trim().to_string()
}

fn cortar(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn url() {
        assert_eq!(normalizar_url("example.com"), "https://example.com");
        assert_eq!(normalizar_url("http://a.b/c"), "http://a.b/c");
        assert_eq!(normalizar_url("about:blank"), "about:blank");
    }

    #[test]
    fn texto() {
        assert_eq!(limpar_texto("a  \n\n\n\nb\n\n"), "a\n\nb");
        assert_eq!(cortar("abcdef", 4), "abc…");
    }
}
