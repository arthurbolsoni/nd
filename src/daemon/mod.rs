//! `nd daemon`: segura o Edge e a conexão CDP e atende os comandos do cliente por HTTP em
//! 127.0.0.1, um de cada vez.

pub mod acoes;
pub mod baixar;
pub mod comandos;
pub mod net;
pub mod pagina;
pub mod rede;

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use nodriver::{Browser, Config, Error, RECONNECTED_EVENT, Tab};
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::cliente::{Pedido, Resposta};
use crate::desktop::{DesktopOculto, criar_processo};
use crate::estado::{self, Estado};

/// Tempo máximo de um comando no daemon.
const LIMITE_COMANDO: Duration = Duration::from_secs(180);
/// `nd scroll --fim` espera a página assentar a cada rolagem (até 5 s cada).
const LIMITE_SCROLL: Duration = Duration::from_secs(540);

#[derive(Debug, Clone)]
pub struct Opcoes {
    pub oculto: bool,
    /// Pasta do perfil, ou `temp` para um perfil descartável.
    pub perfil: String,
    pub lang: String,
    pub headless: bool,
}

pub struct Daemon {
    pub browser: Browser,
    pub opcoes: Opcoes,
    token: String,
    /// Target id da aba em que os comandos atuam.
    pub atual: Mutex<Option<String>>,
    pub rede: rede::Rede,
    pub paginas: pagina::Paginas,
    pub downloads: baixar::Downloads,
    /// Passos feitos nesta sessão, no formato do atlas (`nd atlas --rascunho`).
    pub historico: Mutex<Vec<serde_json::Value>>,
    /// Abas criadas e quando (para a saída das ações).
    criadas: Mutex<Vec<(String, Instant)>>,
    /// Abas com `Network` ligado e eventos sendo acompanhados.
    preparadas: Mutex<HashSet<String>>,
    /// Sessões de iframe de outro site e de worker com a rede acompanhada.
    filhos: Mutex<HashSet<String>>,
    fila: tokio::sync::Mutex<()>,
    pub parar: Notify,
    _desktop: Option<DesktopOculto>,
}

pub async fn rodar(op: Opcoes) -> Result<(), String> {
    let token = gerar_token()?;
    let mut cfg = Config::new()
        .lang(&op.lang)
        .arg("--window-size=1400,950")
        .headless(op.headless);
    if op.perfil == "temp" {
        // Perfil novo entra sozinho na conta Microsoft do Windows e liga a sync.
        cfg = cfg.arg("--disable-sync");
    } else {
        cfg = cfg.user_data_dir(&op.perfil);
    }

    let (browser, desktop) = if op.oculto {
        let desktop = DesktopOculto::criar().map_err(|e| format!("criando desktop oculto: {e}"))?;
        let nome = desktop.nome().to_string();
        let b = Browser::launch_with(cfg, move |exe, args| criar_processo(&nome, exe, args).map(|_| ()))
            .await
            .map_err(|e| erro_lancamento(e, &op.perfil))?;
        (b, Some(desktop))
    } else {
        (
            Browser::launch(cfg)
                .await
                .map_err(|e| erro_lancamento(e, &op.perfil))?,
            None,
        )
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let porta = listener.local_addr().map_err(|e| e.to_string())?.port();
    let d = Arc::new(Daemon {
        browser,
        opcoes: op.clone(),
        token: token.clone(),
        atual: Mutex::new(None),
        rede: rede::Rede::default(),
        paginas: pagina::Paginas::default(),
        downloads: baixar::Downloads::default(),
        criadas: Mutex::new(Vec::new()),
        historico: Mutex::new(Vec::new()),
        preparadas: Mutex::new(HashSet::new()),
        filhos: Mutex::new(HashSet::new()),
        fila: tokio::sync::Mutex::new(()),
        parar: Notify::new(),
        _desktop: desktop,
    });

    acompanhar_alvos(&d).await;
    baixar::ligar(&d).await;
    for tab in d.browser.tabs().await.map_err(|e| e.to_string())? {
        preparar_aba(&d, tab.target_id())
            .await
            .map_err(|e| e.to_string())?;
        d.atual
            .lock()
            .unwrap()
            .get_or_insert_with(|| tab.target_id().to_string());
    }
    vigiar_navegador(&d);

    let meu_pid = std::process::id();
    estado::gravar(&Estado {
        pid: meu_pid,
        porta,
        token,
        perfil: op.perfil.clone(),
        oculto: op.oculto,
        edge_pid: d.browser.pid(),
        iniciado: estado::agora(),
    })
    .map_err(|e| format!("gravando {}: {e}", estado::arquivo().display()))?;
    tracing::info!(
        "pronto na porta {porta} (perfil {}, oculto {})",
        op.perfil,
        op.oculto
    );

    let app = axum::Router::new()
        .route("/cmd", post(tratar))
        .with_state(d.clone());
    let d2 = d.clone();
    let servidor =
        axum::serve(listener, app).with_graceful_shutdown(async move { d2.parar.notified().await });
    let r = servidor.await.map_err(|e| e.to_string());

    tracing::info!("encerrando");
    let _ = d.browser.close().await;
    estado::apagar_se_for(meu_pid);
    r
}

fn erro_lancamento(e: Error, perfil: &str) -> String {
    match e {
        Error::ProfileInUse(_) => {
            format!("o perfil {perfil} já está aberto em outro Edge; feche esse Edge ou use --perfil temp")
        }
        outro => format!("abrindo o Edge: {outro}"),
    }
}

fn gerar_token() -> Result<String, String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| e.to_string())?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

async fn tratar(
    State(d): State<Arc<Daemon>>,
    headers: HeaderMap,
    Json(p): Json<Pedido>,
) -> (StatusCode, Json<Resposta>) {
    let esperado = format!("Bearer {}", d.token);
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(esperado.as_str()) {
        let r = Resposta {
            ok: false,
            saida: String::new(),
            erro: "token inválido".into(),
        };
        return (StatusCode::UNAUTHORIZED, Json(r));
    }
    let _vez = d.fila.lock().await;
    let limite = if p.cmd == "scroll" {
        LIMITE_SCROLL
    } else {
        LIMITE_COMANDO
    };
    let r = match tokio::time::timeout(limite, comandos::executar(&d, &p)).await {
        Ok(Ok(saida)) => Resposta {
            ok: true,
            saida,
            erro: String::new(),
        },
        Ok(Err(erro)) => Resposta {
            ok: false,
            saida: String::new(),
            erro,
        },
        Err(_) => Resposta {
            ok: false,
            saida: String::new(),
            erro: format!("{} passou de {} s", p.cmd, limite.as_secs()),
        },
    };
    (StatusCode::OK, Json(r))
}

/// Buffers de corpo explícitos: o navegador guarda as respostas para `Network.getResponseBody`.
fn params_rede() -> Value {
    json!({ "maxTotalBufferSize": 100_000_000, "maxResourceBufferSize": 10_000_000 })
}

/// Auto-attach com pausa: iframe de outro site (processo próprio) e worker esperam, antes de
/// rodar, o daemon ligar a rede neles.
fn params_auto_attach() -> Value {
    json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true })
}

/// Liga `Network` (e o foco emulado, no desktop oculto) numa aba e acompanha a rede dela e dos
/// iframes de outro site e workers dela.
pub async fn preparar_aba(d: &Arc<Daemon>, alvo: &str) -> nodriver::Result<Tab> {
    let tab = d.browser.tab(alvo).await?;
    let nova = d.preparadas.lock().unwrap().insert(alvo.to_string());
    if nova {
        let mut eventos = tab.subscribe_raw(Some("Network."));
        let fraco = Arc::downgrade(d);
        let aba = alvo.to_string();
        tokio::spawn(async move {
            while let Some(ev) = eventos.next().await {
                let Some(d) = fraco.upgrade() else { break };
                for b in d.rede.evento(&aba, None, &ev) {
                    tokio::spawn(buscar_corpo(d.clone(), aba.clone(), None, b));
                }
            }
        });
        let mut dialogos = tab.subscribe_raw(Some("Page.javascriptDialogOpening"));
        let fraco = Arc::downgrade(d);
        let aba = alvo.to_string();
        tokio::spawn(async move {
            while let Some(ev) = dialogos.next().await {
                let Some(d) = fraco.upgrade() else { break };
                aceitar_dialogo(&d, &aba, None, &ev.params).await;
            }
        });
        let mut filhos = tab.subscribe_raw(Some("Target.attachedToTarget"));
        let fraco = Arc::downgrade(d);
        let aba = alvo.to_string();
        tokio::spawn(async move {
            while let Some(ev) = filhos.next().await {
                let Some(d) = fraco.upgrade() else { break };
                tokio::spawn(adotar_filho(d, aba.clone(), ev.params.clone()));
            }
        });
        ligar_dominios(d, &tab).await;
    }
    Ok(tab)
}

/// Espera a sessão `sessao` acabar (`Target.detachedFromTarget`).
async fn fim_da_sessao(mut desligados: nodriver::RawEventStream, sessao: String) {
    while let Some(ev) = desligados.next().await {
        if ev.params["sessionId"].as_str() == Some(sessao.as_str()) {
            return;
        }
    }
}

/// Iframe de outro site ou worker que o navegador ligou (auto-attach) à aba `aba` ou a um iframe
/// dela: a biblioteca adota a sessão (o mapa e as ações usam a mesma), a rede dele entra no
/// buffer da aba, diálogos são aceitos, e o alvo, parado esperando, segue.
fn adotar_filho(d: Arc<Daemon>, aba: String, p: Value) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async move {
        let Some(sessao) = p["sessionId"].as_str().map(str::to_string) else {
            return;
        };
        let info = &p["targetInfo"];
        let alvo = info["targetId"].as_str().unwrap_or("").to_string();
        let tipo = info["type"].as_str().unwrap_or("").to_string();
        let conn = d.browser.connection().clone();
        conn.adopt(&alvo, &sessao);
        let curto = Duration::from_secs(5);
        let acompanha = matches!(
            tipo.as_str(),
            "iframe" | "worker" | "shared_worker" | "service_worker"
        );
        if acompanha && d.filhos.lock().unwrap().insert(sessao.clone()) {
            let sub = rede::Sub {
                alvo: alvo.clone(),
                tipo: tipo.clone(),
            };
            let mut eventos = conn.subscribe_raw(Some("Network."), Some(&sessao));
            let fim = fim_da_sessao(
                conn.subscribe_raw(Some("Target.detachedFromTarget"), None),
                sessao.clone(),
            );
            let fraco = Arc::downgrade(&d);
            let (aba_, sessao_) = (aba.clone(), sessao.clone());
            tokio::spawn(async move {
                tokio::pin!(fim);
                loop {
                    let ev = tokio::select! {
                        ev = eventos.next() => ev,
                        _ = &mut fim => None,
                    };
                    let Some(ev) = ev else { break };
                    let Some(d) = fraco.upgrade() else { break };
                    for b in d.rede.evento(&aba_, Some(&sub), &ev) {
                        tokio::spawn(buscar_corpo(d.clone(), aba_.clone(), Some(sessao_.clone()), b));
                    }
                }
                if let Some(d) = fraco.upgrade() {
                    d.filhos.lock().unwrap().remove(&sessao_);
                }
            });
            if tipo == "iframe" {
                // Diálogo e iframe de outro site aninhado chegam pela sessão do iframe.
                let mut eventos = conn.subscribe_raw(None, Some(&sessao));
                let fim = fim_da_sessao(
                    conn.subscribe_raw(Some("Target.detachedFromTarget"), None),
                    sessao.clone(),
                );
                let fraco = Arc::downgrade(&d);
                let (aba_, sessao_) = (aba.clone(), sessao.clone());
                tokio::spawn(async move {
                    tokio::pin!(fim);
                    loop {
                        let ev = tokio::select! {
                            ev = eventos.next() => ev,
                            _ = &mut fim => None,
                        };
                        let Some(ev) = ev else { break };
                        let Some(d) = fraco.upgrade() else { break };
                        match ev.method.as_str() {
                            "Page.javascriptDialogOpening" => {
                                aceitar_dialogo(&d, &aba_, Some(&sessao_), &ev.params).await
                            }
                            "Target.attachedToTarget" => {
                                tokio::spawn(adotar_filho(d.clone(), aba_.clone(), ev.params.clone()));
                            }
                            _ => {}
                        }
                    }
                });
            }
        }
        if acompanha {
            let r = conn
                .call_with_timeout("Network.enable", params_rede(), Some(&sessao), curto)
                .await;
            if let Err(e) = r {
                tracing::debug!("Network.enable em {tipo} {alvo}: {e}");
            }
        }
        if tipo == "iframe" {
            for (metodo, params) in [
                ("Page.enable", json!({})),
                ("Target.setAutoAttach", params_auto_attach()),
            ] {
                if let Err(e) = conn.call_with_timeout(metodo, params, Some(&sessao), curto).await {
                    tracing::debug!("{metodo} no iframe {alvo}: {e}");
                }
            }
        }
        // Sempre solta o alvo, mesmo se algo acima falhou: senão o iframe ou worker fica parado.
        if p["waitingForDebugger"] == json!(true) {
            let r = conn
                .call_with_timeout("Runtime.runIfWaitingForDebugger", json!({}), Some(&sessao), curto)
                .await;
            if let Err(e) = r {
                tracing::warn!("{tipo} {alvo} não foi solto: {e}");
            }
        }
    })
}

/// Pede ao navegador um corpo de chamada e o guarda (a resposta fica disponível só por um tempo
/// depois do `loadingFinished`). `sessao` = a do iframe ou worker; `None` = a da aba.
async fn buscar_corpo(d: Arc<Daemon>, aba: String, sessao: Option<String>, b: rede::Buscar) {
    let conn = d.browser.connection().clone();
    let sessao = sessao.or_else(|| conn.session_of(&aba));
    match b {
        rede::Buscar::Resposta(id) => {
            match conn
                .call(
                    "Network.getResponseBody",
                    json!({ "requestId": id }),
                    sessao.as_deref(),
                )
                .await
            {
                Ok(v) => d.rede.com_resposta(
                    &aba,
                    &id,
                    v["body"].as_str().unwrap_or(""),
                    v["base64Encoded"].as_bool().unwrap_or(false),
                ),
                Err(e) => tracing::debug!("corpo de {id}: {e}"),
            }
        }
        rede::Buscar::Enviado(id) => {
            if let Ok(v) = conn
                .call(
                    "Network.getRequestPostData",
                    json!({ "requestId": id }),
                    sessao.as_deref(),
                )
                .await
            {
                d.rede
                    .com_enviado(&aba, &id, v["postData"].as_str().unwrap_or(""));
            }
        }
    }
}

/// Aceita `alert`/`confirm`/`prompt`/`beforeunload` (a ação que o abriu foi pedida) e guarda o
/// texto para a saída do comando. `sessao` = a do iframe que abriu; `None` = a da aba.
async fn aceitar_dialogo(d: &Daemon, aba: &str, sessao: Option<&str>, p: &Value) {
    let tipo = p["type"].as_str().unwrap_or("alert");
    let msg = p["message"].as_str().unwrap_or("");
    let conn = d.browser.connection();
    let sessao = sessao.map(str::to_string).or_else(|| conn.session_of(aba));
    let r = conn
        .call(
            "Page.handleJavaScriptDialog",
            json!({ "accept": true, "promptText": p["defaultPrompt"].as_str().unwrap_or("") }),
            sessao.as_deref(),
        )
        .await;
    if let Err(e) = r {
        tracing::warn!("diálogo {tipo} em {aba}: {e}");
    }
    d.paginas.dialogo(aba, format!("diálogo {tipo} {msg:?} aceito"));
}

/// Domínios que o daemon precisa em cada aba. Uma reconexão cria sessões novas, onde eles
/// voltam desligados.
async fn ligar_dominios(d: &Daemon, tab: &Tab) {
    for (dominio, params) in [
        ("Network.enable", params_rede()),
        ("Page.enable", json!({})),
        ("Target.setAutoAttach", params_auto_attach()),
    ] {
        if let Err(e) = tab.call(dominio, params).await {
            tracing::warn!("{dominio} em {}: {e}", tab.target_id());
        }
    }
    if d.opcoes.oculto
        && let Err(e) = tab
            .call("Emulation.setFocusEmulationEnabled", json!({ "enabled": true }))
            .await
    {
        tracing::warn!("foco emulado em {}: {e}", tab.target_id());
    }
}

/// Abas novas (pop-up, `target=_blank`) são preparadas assim que aparecem; abas fechadas saem
/// do estado. Depois de uma reconexão, liga de novo a descoberta e os domínios.
async fn acompanhar_alvos(d: &Arc<Daemon>) {
    let conn = d.browser.connection().clone();
    let mut eventos = conn.subscribe_raw(Some("Target."), None);
    let mut reconexoes = conn.subscribe_raw(Some(RECONNECTED_EVENT), None);
    let _ = conn
        .call("Target.setDiscoverTargets", json!({ "discover": true }), None)
        .await;

    let fraco = Arc::downgrade(d);
    tokio::spawn(async move {
        while let Some(ev) = eventos.next().await {
            let Some(d) = fraco.upgrade() else { break };
            let info = &ev.params["targetInfo"];
            let alvo = info["targetId"]
                .as_str()
                .or_else(|| ev.params["targetId"].as_str());
            match (ev.method.as_str(), alvo) {
                ("Target.targetCreated", Some(id)) if info["type"] == "page" => {
                    d.criadas.lock().unwrap().push((id.to_string(), Instant::now()));
                    if let Err(e) = preparar_aba(&d, id).await {
                        tracing::debug!("aba {id} não preparada: {e}");
                    }
                }
                ("Target.targetDestroyed", Some(id)) => {
                    d.preparadas.lock().unwrap().remove(id);
                    d.criadas.lock().unwrap().retain(|(x, _)| x != id);
                    d.rede.remover(id);
                    d.paginas.remover(id);
                    let mut atual = d.atual.lock().unwrap();
                    if atual.as_deref() == Some(id) {
                        *atual = None;
                    }
                }
                _ => {}
            }
        }
    });

    let fraco = Arc::downgrade(d);
    tokio::spawn(async move {
        while reconexoes.next().await.is_some() {
            let Some(d) = fraco.upgrade() else { break };
            tracing::info!("conexão com o Edge refeita; religando domínios");
            let conn = d.browser.connection().clone();
            let _ = conn
                .call("Target.setDiscoverTargets", json!({ "discover": true }), None)
                .await;
            baixar::configurar(&conn).await;
            let alvos: Vec<String> = d.preparadas.lock().unwrap().iter().cloned().collect();
            for alvo in alvos {
                if let Ok(tab) = d.browser.tab(&alvo).await {
                    ligar_dominios(&d, &tab).await;
                }
            }
        }
    });
}

impl Daemon {
    /// Abas criadas desde `t0` que ainda estão abertas.
    pub fn criadas_desde(&self, t0: Instant) -> Vec<String> {
        self.criadas
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, t)| *t >= t0)
            .map(|(id, _)| id.clone())
            .collect()
    }
}

/// Encerra o daemon quando o Edge morre (a conexão não volta).
fn vigiar_navegador(d: &Arc<Daemon>) {
    let mut tudo = d.browser.connection().subscribe_raw(None, None);
    let fraco = Arc::downgrade(d);
    tokio::spawn(async move {
        while tudo.next().await.is_some() {}
        if let Some(d) = fraco.upgrade() {
            tracing::warn!("o Edge fechou; encerrando o daemon");
            d.parar.notify_one();
        }
    });
}
