//! `nd daemon`: segura o Edge e a conexão CDP e atende os comandos do cliente por HTTP em
//! 127.0.0.1, um de cada vez.

pub mod comandos;
pub mod rede;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use nodriver::{Browser, Config, Error, RECONNECTED_EVENT, Tab};
use serde_json::json;
use tokio::sync::Notify;

use crate::cliente::{Pedido, Resposta};
use crate::desktop::{DesktopOculto, criar_processo};
use crate::estado::{self, Estado};

/// Tempo máximo de um comando no daemon.
const LIMITE_COMANDO: Duration = Duration::from_secs(180);

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
    /// Abas com `Network` ligado e eventos sendo acompanhados.
    preparadas: Mutex<HashSet<String>>,
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
        preparadas: Mutex::new(HashSet::new()),
        fila: tokio::sync::Mutex::new(()),
        parar: Notify::new(),
        _desktop: desktop,
    });

    acompanhar_alvos(&d).await;
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
    let r = match tokio::time::timeout(LIMITE_COMANDO, comandos::executar(&d, &p)).await {
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
            erro: format!("{} passou de {} s", p.cmd, LIMITE_COMANDO.as_secs()),
        },
    };
    (StatusCode::OK, Json(r))
}

/// Liga `Network` (e o foco emulado, no desktop oculto) numa aba e acompanha a rede dela.
pub async fn preparar_aba(d: &Arc<Daemon>, alvo: &str) -> nodriver::Result<Tab> {
    let tab = d.browser.tab(alvo).await?;
    let nova = d.preparadas.lock().unwrap().insert(alvo.to_string());
    if nova {
        let mut eventos = tab.subscribe_raw(Some("Network."));
        let fraco = Arc::downgrade(d);
        let alvo_ = alvo.to_string();
        tokio::spawn(async move {
            while let Some(ev) = eventos.next().await {
                let Some(d) = fraco.upgrade() else { break };
                d.rede.evento(&alvo_, &ev);
            }
        });
        ligar_dominios(d, &tab).await;
    }
    Ok(tab)
}

/// Domínios que o daemon precisa em cada aba. Uma reconexão cria sessões novas, onde eles
/// voltam desligados.
async fn ligar_dominios(d: &Daemon, tab: &Tab) {
    if let Err(e) = tab.call("Network.enable", json!({})).await {
        tracing::warn!("Network.enable em {}: {e}", tab.target_id());
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
                    if let Err(e) = preparar_aba(&d, id).await {
                        tracing::debug!("aba {id} não preparada: {e}");
                    }
                }
                ("Target.targetDestroyed", Some(id)) => {
                    d.preparadas.lock().unwrap().remove(id);
                    d.rede.remover(id);
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
            let alvos: Vec<String> = d.preparadas.lock().unwrap().iter().cloned().collect();
            for alvo in alvos {
                if let Ok(tab) = d.browser.tab(&alvo).await {
                    ligar_dominios(&d, &tab).await;
                }
            }
        }
    });
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
