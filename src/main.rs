//! `nd`: automação de navegador para o Claude Code. `nd start` sobe um daemon que segura o Edge;
//! os outros comandos falam com ele e terminam na hora.

mod atlas;
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

#[derive(Args, Clone)]
struct OpAcao {
    /// Não lê a página depois da ação (sem o diff).
    #[arg(long)]
    sem_diff: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Sobe o daemon e o Edge.
    Start(Abertura),
    /// Fecha o Edge e encerra o daemon.
    Stop,
    /// Mostra o daemon, o perfil e a aba atual.
    Status,
    /// Navega a aba atual (ou uma nova), espera rede ociosa + DOM parado e mostra o mapa.
    Open {
        url: String,
        /// Abre numa aba nova, que passa a ser a atual.
        #[arg(long)]
        nova: bool,
        /// Só título, URL e espera, sem o mapa.
        #[arg(long)]
        sem_map: bool,
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
    /// Mapa da página: elementos interativos com refs (e12), grupos, títulos e avisos.
    Map {
        /// Página toda, sem recorte pela área visível nem pelo diálogo modal.
        #[arg(long)]
        tudo: bool,
        /// Só o que está dentro da ref (form, diálogo, iframe).
        #[arg(long)]
        dentro: Option<String>,
        /// Máximo de linhas (padrão 300; 2000 com --tudo).
        #[arg(long)]
        max: Option<usize>,
        /// Mostra o destino de todos os links.
        #[arg(long)]
        href: bool,
    },
    /// Clica (mouse via CDP) num elemento, por ref ou texto, e mostra o que mudou.
    Click {
        alvo: String,
        /// Usa el.click() em vez do mouse (elemento coberto ou sem caixa).
        #[arg(long)]
        js: bool,
        #[command(flatten)]
        acao: OpAcao,
    },
    /// Preenche um campo de texto (substitui o conteúdo).
    Fill {
        alvo: String,
        valor: String,
        /// Digita tecla por tecla em vez de inserir o texto de uma vez.
        #[arg(long)]
        teclas: bool,
        #[command(flatten)]
        acao: OpAcao,
    },
    /// Escolhe uma opção: select nativo, combobox, autocomplete ou menu.
    Pick {
        alvo: String,
        opcao: String,
        #[command(flatten)]
        acao: OpAcao,
    },
    /// Marca (ou desmarca) checkbox, radio ou switch.
    Check {
        alvo: String,
        /// Desmarca.
        #[arg(long)]
        nao: bool,
        #[command(flatten)]
        acao: OpAcao,
    },
    /// Envia arquivo(s) a um input de arquivo ou ao botão que abre o seletor.
    Upload {
        alvo: String,
        #[arg(required = true)]
        arquivos: Vec<PathBuf>,
        #[command(flatten)]
        acao: OpAcao,
    },
    /// Pressiona teclas (Enter, Tab, Escape, Control+a, ...) no foco atual ou no elemento --em.
    Key {
        #[arg(required = true)]
        teclas: Vec<String>,
        /// Foca este elemento (ref ou texto) antes.
        #[arg(long)]
        em: Option<String>,
        #[command(flatten)]
        acao: OpAcao,
    },
    /// Login com a senha salva no Edge: o navegador preenche, o nd só envia (a senha não passa
    /// pelo nd). Sem senha salva, pede o login manual na janela.
    Login {
        /// Botão de envio (ref ou texto), se não for o do formulário.
        #[arg(long)]
        enviar: Option<String>,
        /// JavaScript que dá true quando logado (o login.checar do atlas).
        #[arg(long)]
        checar: Option<String>,
        /// Login em duas etapas: usuário (CPF, e-mail) digitado na primeira tela.
        #[arg(long)]
        usuario: Option<String>,
        #[command(flatten)]
        acao: OpAcao,
    },
    /// Rola a página (gesto de rolagem, como a roda do mouse) e mostra o que carregou.
    Scroll {
        /// Rola para cima.
        #[arg(long)]
        subir: bool,
        /// Quantas rolagens de ~1 tela (padrão 1; com --fim, o limite, padrão 30).
        #[arg(long)]
        vezes: Option<u32>,
        /// Rola até a página (ou a área) parar de crescer.
        #[arg(long)]
        fim: bool,
        /// Rola a área com rolagem própria que contém este elemento (ref ou texto).
        #[arg(long)]
        dentro: Option<String>,
        #[command(flatten)]
        acao: OpAcao,
    },
    /// Espera texto, seletor, URL ou requisição (desde a última ação).
    Wait {
        #[arg(value_parser = ["text", "sel", "url", "net"])]
        tipo: String,
        valor: String,
        /// Espera sumir em vez de aparecer (text, sel).
        #[arg(long)]
        sumir: bool,
        /// Segundos.
        #[arg(long, default_value_t = 15)]
        max: u64,
    },
    /// Clica e grava o arquivo gerado (download, PDF na mesma aba ou em aba nova, blob); com --url,
    /// baixa a URL com a sessão da aba; sem nada, lista os arquivos da sessão.
    Download {
        /// Elemento que gera o arquivo (ref ou texto).
        alvo: Option<String>,
        /// Baixa esta URL com os cookies da aba atual.
        #[arg(long, conflicts_with = "alvo")]
        url: Option<String>,
        /// Pasta onde gravar (padrão ~/.nd/downloads).
        #[arg(long)]
        pasta: Option<PathBuf>,
        /// Arquivo de saída (com --url).
        #[arg(long, requires = "url")]
        saida: Option<PathBuf>,
        /// Segundos de espera pelo arquivo.
        #[arg(long, default_value_t = 30)]
        max: u64,
        /// Usa el.click() em vez do mouse.
        #[arg(long)]
        js: bool,
        #[command(flatten)]
        acao: OpAcao,
    },
    /// Imprime a aba atual em PDF.
    Pdf {
        saida: PathBuf,
        #[arg(long)]
        paisagem: bool,
    },
    /// Chamadas XHR/fetch/documento capturadas: uma linha cada, com a forma do JSON.
    Net {
        /// Trecho da URL, método ou nome da operação (GraphQL).
        filtro: Option<String>,
        /// De todas as abas (padrão: a atual).
        #[arg(long)]
        todas: bool,
        /// Quantas mostrar (as mais recentes).
        #[arg(long, default_value_t = 30)]
        ultimas: usize,
        /// Só as feitas desde a última ação.
        #[arg(long)]
        novas: bool,
        /// Inclui as das páginas anteriores da aba (padrão: só a página atual).
        #[arg(long)]
        antigas: bool,
        /// Detalhe de uma chamada: corpo enviado e resposta.
        #[arg(long)]
        corpo: Option<u64>,
        /// Parte da resposta JSON: data.itens, data.itens.0, data.itens[].nome
        #[arg(long)]
        caminho: Option<String>,
        /// Máximo de caracteres da resposta.
        #[arg(long, default_value_t = 4000)]
        max: usize,
        /// Grava a resposta (ou a parte do --caminho) neste arquivo.
        #[arg(long)]
        saida: Option<PathBuf>,
    },
    /// Refaz uma chamada capturada por fetch na página (cookies, origem e cabeçalhos do site).
    Api {
        /// Número da chamada em `nd net`.
        n: u64,
        /// Funde este JSON no corpo (num form, nos campos; campo com JSON dentro é fundido também).
        #[arg(long)]
        json: Option<String>,
        /// Muda um valor do corpo: variables.count=20 (repetível).
        #[arg(long)]
        set: Vec<String>,
        /// Muda um parâmetro da URL: pagina=2 (repetível).
        #[arg(long)]
        query: Vec<String>,
        #[arg(long)]
        metodo: Option<String>,
        /// Corpo novo, inteiro.
        #[arg(long)]
        corpo: Option<String>,
        #[arg(long)]
        caminho: Option<String>,
        #[arg(long, default_value_t = 4000)]
        max: usize,
        /// Grava a resposta (bytes, ou a parte do --caminho) neste arquivo.
        #[arg(long)]
        saida: Option<PathBuf>,
    },
    /// Atlas do site (~/nodriver/sites/<site>.yaml); sem site, lista os atlas.
    Atlas {
        site: Option<String>,
        /// Passos feitos nesta sessão, em YAML, para virar fluxo.
        #[arg(long, conflicts_with = "site")]
        rascunho: bool,
    },
    /// Roda um fluxo do atlas, passo a passo, sem LLM.
    Run {
        site: String,
        fluxo: String,
        /// Parâmetro do fluxo: -p nome=valor (repetível).
        #[arg(short = 'p', long = "param")]
        params: Vec<String>,
        /// Começa neste passo (1 = primeiro).
        #[arg(long, default_value_t = 1)]
        desde: usize,
        /// Para depois deste passo.
        #[arg(long)]
        ate: Option<usize>,
        /// Executa também os passos marcados com confirmar (o usuário já deu o ok).
        #[arg(long)]
        confirmado: bool,
        /// Só mostra os passos com os parâmetros trocados, sem executar.
        #[arg(long)]
        seco: bool,
        /// Saída inteira de cada passo.
        #[arg(long)]
        tudo: bool,
    },
    /// Comando CDP cru na aba atual (depuração): nd cdp Network.getResponseBody '{"requestId":"1.2"}'
    #[command(hide = true)]
    Cdp { metodo: String, params: Option<String> },
    /// Processo do daemon (usado pelo `nd start`).
    #[command(hide = true)]
    Daemon(Abertura),
}

/// Tempo máximo de espera do cliente por comandos que navegam ou agem.
const LONGO: Duration = Duration::from_secs(200);
/// `nd scroll --fim` espera a página assentar a cada rolagem.
const ESPERA_SCROLL: Duration = Duration::from_secs(600);

fn main() -> ExitCode {
    let cli = Cli::parse();
    let r = match cli.cmd {
        Cmd::Start(a) => start(a),
        Cmd::Daemon(a) => rodar_daemon(a),
        Cmd::Stop => parar(),
        Cmd::Status => comando("status", json!({}), Duration::from_secs(30)),
        Cmd::Open { url, nova, sem_map } => comando(
            "open",
            json!({ "url": url, "nova": nova, "sem_map": sem_map }),
            LONGO,
        ),
        Cmd::Map {
            tudo,
            dentro,
            max,
            href,
        } => comando(
            "map",
            json!({ "tudo": tudo, "dentro": dentro, "max": max, "href": href }),
            LONGO,
        ),
        Cmd::Click { alvo, js, acao } => comando(
            "click",
            json!({ "alvo": alvo, "js": js, "sem_diff": acao.sem_diff }),
            LONGO,
        ),
        Cmd::Fill {
            alvo,
            valor,
            teclas,
            acao,
        } => comando(
            "fill",
            json!({ "alvo": alvo, "valor": valor, "teclas": teclas, "sem_diff": acao.sem_diff }),
            LONGO,
        ),
        Cmd::Pick { alvo, opcao, acao } => comando(
            "pick",
            json!({ "alvo": alvo, "opcao": opcao, "sem_diff": acao.sem_diff }),
            LONGO,
        ),
        Cmd::Check { alvo, nao, acao } => comando(
            "check",
            json!({ "alvo": alvo, "desmarcar": nao, "sem_diff": acao.sem_diff }),
            LONGO,
        ),
        Cmd::Upload { alvo, arquivos, acao } => match arquivos
            .iter()
            .map(|f| std::path::absolute(f).map(|p| p.display().to_string()))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(arquivos) => comando(
                "upload",
                json!({ "alvo": alvo, "arquivos": arquivos, "sem_diff": acao.sem_diff }),
                LONGO,
            ),
            Err(e) => Err(e.to_string()),
        },
        Cmd::Scroll {
            subir,
            vezes,
            fim,
            dentro,
            acao,
        } => comando(
            "scroll",
            json!({ "alvo": dentro, "subir": subir, "vezes": vezes, "fim": fim, "sem_diff": acao.sem_diff }),
            ESPERA_SCROLL,
        ),
        Cmd::Login {
            enviar,
            checar,
            usuario,
            acao,
        } => comando(
            "login",
            json!({ "enviar": enviar, "checar": checar, "usuario": usuario, "sem_diff": acao.sem_diff }),
            LONGO,
        ),
        Cmd::Key { teclas, em, acao } => comando(
            "key",
            json!({ "alvo": em, "teclas": teclas, "sem_diff": acao.sem_diff }),
            LONGO,
        ),
        Cmd::Net {
            filtro,
            todas,
            ultimas,
            novas,
            antigas,
            corpo,
            caminho,
            max,
            saida,
        } => match absoluto(saida) {
            Ok(saida) => comando(
                "net",
                json!({ "filtro": filtro, "todas": todas, "ultimas": ultimas, "novas": novas, "antigas": antigas,
                        "corpo": corpo, "caminho": caminho, "max": max, "saida": saida }),
                LONGO,
            ),
            Err(e) => Err(e),
        },
        Cmd::Api {
            n,
            json: j,
            set,
            query,
            metodo,
            corpo,
            caminho,
            max,
            saida,
        } => match absoluto(saida) {
            Ok(saida) => comando(
                "api",
                json!({ "n": n, "json": j, "set": set, "query": query, "metodo": metodo,
                        "corpo": corpo, "caminho": caminho, "max": max, "saida": saida }),
                LONGO,
            ),
            Err(e) => Err(e),
        },
        Cmd::Download {
            alvo,
            url,
            pasta,
            saida,
            max,
            js,
            acao,
        } => match (absoluto(pasta), absoluto(saida)) {
            (Ok(pasta), Ok(saida)) => comando(
                "download",
                json!({ "alvo": alvo, "url": url, "pasta": pasta, "saida": saida, "max": max,
                        "js": js, "sem_diff": acao.sem_diff }),
                Duration::from_secs(max + 60),
            ),
            (Err(e), _) | (_, Err(e)) => Err(e),
        },
        Cmd::Pdf { saida, paisagem } => match absoluto(Some(saida)) {
            Ok(saida) => comando("pdf", json!({ "saida": saida, "paisagem": paisagem }), LONGO),
            Err(e) => Err(e),
        },
        Cmd::Atlas { site, rascunho } => atlas::atlas(site.as_deref(), rascunho).map(|s| println!("{s}")),
        Cmd::Run {
            site,
            fluxo,
            params,
            desde,
            ate,
            confirmado,
            seco,
            tudo,
        } => atlas::run(&atlas::OpRun {
            site,
            fluxo,
            params,
            desde,
            ate,
            confirmado,
            seco,
            tudo,
        })
        .map(|s| println!("{s}")),
        Cmd::Cdp { metodo, params } => match params
            .as_deref()
            .map(serde_json::from_str::<serde_json::Value>)
            .transpose()
        {
            Ok(p) => comando("cdp", json!({ "metodo": metodo, "params": p }), LONGO),
            Err(e) => Err(format!("params: {e}")),
        },
        Cmd::Wait {
            tipo,
            valor,
            sumir,
            max,
        } => comando(
            "wait",
            json!({ "tipo": tipo, "valor": valor, "sumir": sumir, "max": max }),
            Duration::from_secs(max + 30),
        ),
        Cmd::Tabs => comando("tabs", json!({}), Duration::from_secs(30)),
        Cmd::Tab { alvo } => comando("tab", json!({ "alvo": alvo }), Duration::from_secs(30)),
        Cmd::Eval { js, arquivo } => match (js, arquivo) {
            (_, Some(f)) => match std::fs::read_to_string(&f) {
                Ok(js) => comando("eval", json!({ "js": js }), LONGO),
                Err(e) => Err(format!("{}: {e}", f.display())),
            },
            (Some(js), None) => comando("eval", json!({ "js": js }), LONGO),
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

/// Caminho absoluto (o daemon roda noutro diretório).
fn absoluto(p: Option<PathBuf>) -> Result<Option<String>, String> {
    p.map(|p| std::path::absolute(&p).map(|a| a.display().to_string()))
        .transpose()
        .map_err(|e| e.to_string())
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
