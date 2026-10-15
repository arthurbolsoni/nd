//! `nd`: automação de navegador para o Claude Code. `nd start` sobe um daemon que segura o Edge;
//! os outros comandos falam com ele e terminam na hora.

mod cliente;
mod daemon;
mod desktop;
mod estado;

use std::path::PathBuf;
use std::process::{ExitCode, Stdio};
use std::time::{Duration, Instant};

use clap::{Args, Parser, Subcommand};
use serde_json::json;

use crate::cliente::{Pedido, enviar};

#[derive(Parser)]
#[command(
    name = "nd",
    version,
    about = "Automação de navegador (Edge via CDP) para o Claude Code"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct Abertura {
    /// Abre o Edge num desktop Windows oculto (fora da tela, sem roubar o foco).
    #[arg(long)]
    oculto: bool,
    /// Pasta do perfil, ou `temp` para um perfil descartável.
    #[arg(long, default_value_t = estado::perfil_padrao())]
    perfil: String,
    /// Idioma da interface e do Accept-Language (ex.: pt-BR em site brasileiro).
    #[arg(long, default_value = "en-US")]
    lang: String,
    /// Edge sem janela (`--headless=new`).
    #[arg(long)]
    headless: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Sobe o daemon e o Edge.
    Start(Abertura),
    /// Fecha o Edge e encerra o daemon.
    Stop,
    /// Mostra o daemon, o perfil e a aba atual.
    Status,
    /// Navega a aba atual (ou uma nova) e espera rede ociosa + DOM parado.
    Open {
        url: String,
        /// Abre numa aba nova, que passa a ser a atual.
        #[arg(long)]
        nova: bool,
    },
    /// Lista as abas; `*` marca a atual.
    Tabs,
    /// Escolhe a aba atual pelo número de `nd tabs` ou por um trecho do título/URL.
    Tab { alvo: String },
    /// Avalia JavaScript na aba atual (promises são aguardadas).
    Eval {
        js: Option<String>,
        /// Lê o JavaScript de um arquivo.
        #[arg(short = 'f', long = "arquivo")]
        arquivo: Option<PathBuf>,
    },
    /// Texto visível (`innerText`) da página ou de um seletor CSS.
    Text {
        #[arg(long)]
        sel: Option<String>,
        /// Máximo de caracteres.
        #[arg(long, default_value_t = 8000)]
        max: usize,
    },
    /// Processo do daemon (usado pelo `nd start`).
    #[command(hide = true)]
    Daemon(Abertura),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let r = match cli.cmd {
        Cmd::Start(a) => start(a),
        Cmd::Daemon(a) => rodar_daemon(a),
        Cmd::Stop => parar(),
        Cmd::Status => comando("status", json!({}), Duration::from_secs(30)),
        Cmd::Open { url, nova } => comando(
            "open",
            json!({ "url": url, "nova": nova }),
            Duration::from_secs(200),
        ),
        Cmd::Tabs => comando("tabs", json!({}), Duration::from_secs(30)),
        Cmd::Tab { alvo } => comando("tab", json!({ "alvo": alvo }), Duration::from_secs(30)),
        Cmd::Eval { js, arquivo } => match (js, arquivo) {
            (_, Some(f)) => match std::fs::read_to_string(&f) {
                Ok(js) => comando("eval", json!({ "js": js }), Duration::from_secs(200)),
                Err(e) => Err(format!("{}: {e}", f.display())),
            },
            (Some(js), None) => comando("eval", json!({ "js": js }), Duration::from_secs(200)),
            (None, None) => Err("informe o JavaScript ou -f arquivo.js".into()),
        },
        Cmd::Text { sel, max } => comando("text", json!({ "sel": sel, "max": max }), Duration::from_secs(60)),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Manda um comando ao daemon e imprime a saída.
fn comando(cmd: &str, args: serde_json::Value, timeout: Duration) -> Result<(), String> {
    let est = estado::ler().ok_or("o daemon não está rodando; use nd start")?;
    let r = enviar(
        &est,
        &Pedido {
            cmd: cmd.into(),
            args,
        },
        timeout,
    )?;
    if r.ok {
        if !r.saida.is_empty() {
            println!("{}", r.saida);
        }
        Ok(())
    } else {
        Err(r.erro)
    }
}

fn parar() -> Result<(), String> {
    let Some(est) = estado::ler() else {
        println!("o daemon não está rodando");
        return Ok(());
    };
    match enviar(
        &est,
        &Pedido {
            cmd: "stop".into(),
            args: json!({}),
        },
        Duration::from_secs(10),
    ) {
        Ok(r) => println!("{}", r.saida),
        Err(e) => {
            println!("daemon não respondeu ({e}); estado apagado");
            let _ = std::fs::remove_file(estado::arquivo());
            return Ok(());
        }
    }
    // Espera o daemon fechar o Edge e apagar o estado.
    let inicio = Instant::now();
    while estado::ler().is_some_and(|e| e.pid == est.pid) && inicio.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

fn start(a: Abertura) -> Result<(), String> {
    if let Some(est) = estado::ler() {
        if enviar(
            &est,
            &Pedido {
                cmd: "status".into(),
                args: json!({}),
            },
            Duration::from_secs(10),
        )
        .is_ok()
        {
            println!("o daemon já está rodando (nd stop para fechar)");
            return comando("status", json!({}), Duration::from_secs(30));
        }
        let _ = std::fs::remove_file(estado::arquivo());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let log = std::fs::File::create(estado::log()).map_err(|e| e.to_string())?;
    let log2 = log.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("daemon").args(["--perfil", &a.perfil, "--lang", &a.lang]);
    if a.oculto {
        cmd.arg("--oculto");
    }
    if a.headless {
        cmd.arg("--headless");
    }
    cmd.stdin(Stdio::null()).stdout(log).stderr(log2);
    let mut filho = desacoplar(cmd)?;

    let inicio = Instant::now();
    loop {
        if let Some(est) = estado::ler()
            && enviar(
                &est,
                &Pedido {
                    cmd: "status".into(),
                    args: json!({}),
                },
                Duration::from_secs(10),
            )
            .is_ok()
        {
            return comando("status", json!({}), Duration::from_secs(30));
        }
        if let Ok(Some(status)) = filho.try_wait() {
            let log = std::fs::read_to_string(estado::log()).unwrap_or_default();
            let fim: Vec<&str> = log.lines().rev().take(15).collect();
            let fim: Vec<&str> = fim.into_iter().rev().collect();
            return Err(format!("o daemon saiu ({status}):\n{}", fim.join("\n")));
        }
        if inicio.elapsed() > Duration::from_secs(60) {
            return Err(format!(
                "o daemon não ficou pronto em 60 s; veja {}",
                estado::log().display()
            ));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// Inicia o daemon fora do grupo de processos e, se o job permitir, fora do job do terminal,
/// para ele seguir vivo depois que o comando que o iniciou terminar.
fn desacoplar(mut cmd: std::process::Command) -> Result<std::process::Child, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // O Windows cria o filho herdando todo handle herdável. Os handles padrão deste processo
        // costumam ser pipes do terminal (`nd start | ...`, o Bash do Claude Code); herdados,
        // o pipe nunca fecha e quem lê a saída fica preso até o daemon terminar.
        nao_herdar_handles_padrao();
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB);
        match cmd.spawn() {
            Ok(c) => return Ok(c),
            Err(e) => tracing::debug!("sem breakaway do job: {e}"),
        }
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    cmd.spawn().map_err(|e| format!("iniciando o daemon: {e}"))
}

#[cfg(windows)]
fn nao_herdar_handles_padrao() {
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation};
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        unsafe {
            let h = GetStdHandle(id);
            if !h.is_null() && h != INVALID_HANDLE_VALUE {
                SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

fn rodar_daemon(a: Abertura) -> Result<(), String> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("ND_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_ansi(false)
        .init();
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let op = daemon::Opcoes {
        oculto: a.oculto,
        perfil: a.perfil,
        lang: a.lang,
        headless: a.headless,
    };
    let r = rt.block_on(daemon::rodar(op));
    if let Err(e) = &r {
        tracing::error!("{e}");
    }
    r
}
