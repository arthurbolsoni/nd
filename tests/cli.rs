//! Testes do `nd` de ponta a ponta: daemon de verdade, Edge headless com perfil temporário,
//! estado num diretório próprio (`ND_DIR`) e páginas locais.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct Nd {
    dir: tempfile::TempDir,
}

impl Nd {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_nd"))
            .args(args)
            .env("ND_DIR", self.dir.path())
            .output()
            .unwrap()
    }

    /// Roda e exige sucesso; devolve o stdout.
    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        let out = String::from_utf8_lossy(&o.stdout).to_string();
        let err = String::from_utf8_lossy(&o.stderr).to_string();
        assert!(
            o.status.success(),
            "nd {args:?} falhou\nstdout: {out}\nstderr: {err}\nlog: {}",
            self.log()
        );
        out
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("daemon.log")).unwrap_or_default()
    }

    fn estado(&self) -> PathBuf {
        self.dir.path().join("daemon.json")
    }
}

impl Drop for Nd {
    fn drop(&mut self) {
        if self.estado().exists() {
            let _ = self.run(&["stop"]);
        }
    }
}

/// Serve `tests/fixtures`; `/api/lenta` responde depois de 1,5 s.
async fn servir() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let porta = listener.local_addr().unwrap().port();
    let raiz = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                break;
            };
            let raiz = raiz.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let caminho = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (status, tipo, corpo): (&str, &str, Vec<u8>) = if caminho == "/api/lenta" {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    ("200 OK", "application/json", br#"{"valor":42}"#.to_vec())
                } else {
                    match std::fs::read(raiz.join(caminho.trim_start_matches('/'))) {
                        Ok(b) => ("200 OK", "text/html; charset=utf-8", b),
                        Err(_) => ("404 Not Found", "text/plain", b"404".to_vec()),
                    }
                };
                let cab = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {tipo}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    corpo.len()
                );
                let _ = s.write_all(cab.as_bytes()).await;
                let _ = s.write_all(&corpo).await;
            });
        }
    });
    format!("http://127.0.0.1:{porta}")
}

#[tokio::test(flavor = "multi_thread")]
async fn fluxo_completo() {
    let base = servir().await;
    let nd = Nd::new();

    // Command::output lê o stdout até o fim: se o daemon herdasse o pipe, isto travaria.
    let inicio = Instant::now();
    let status = nd.ok(&["start", "--perfil", "temp", "--headless"]);
    assert!(inicio.elapsed() < Duration::from_secs(60));
    assert!(status.contains("perfil temp"), "{status}");
    assert!(nd.estado().exists());

    let segundo = nd.ok(&["start", "--perfil", "temp", "--headless"]);
    assert!(segundo.contains("já está rodando"), "{segundo}");

    // open espera a requisição lenta e a mudança do DOM depois dela.
    let saida = nd.ok(&["open", &format!("{base}/lento.html")]);
    assert!(saida.starts_with("Lento\n"), "{saida}");
    assert!(saida.contains("estável em"), "{saida}");
    assert!(!saida.contains("aviso"), "{saida}");
    let texto = nd.ok(&["text", "--sel", "#dados"]);
    assert_eq!(texto.trim(), "dados: 42");

    assert_eq!(nd.ok(&["eval", "1 + 2"]).trim(), "3");
    assert_eq!(nd.ok(&["eval", "'texto'"]).trim(), "texto");
    let js = nd.dir.path().join("x.js");
    std::fs::write(&js, "({a: [1, 2]})").unwrap();
    let obj = nd.ok(&["eval", "-f", js.to_str().unwrap()]);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&obj).unwrap(),
        serde_json::json!({"a": [1, 2]})
    );

    let erro = nd.run(&["eval", "throw new Error('falhou aqui')"]);
    assert!(!erro.status.success());
    assert!(String::from_utf8_lossy(&erro.stderr).contains("falhou aqui"));

    nd.ok(&["open", &format!("{base}/outra.html"), "--nova"]);
    let abas = nd.ok(&["tabs"]);
    assert_eq!(abas.lines().count(), 2, "{abas}");
    assert!(
        abas.lines().nth(1).unwrap().starts_with("* [2] Outra página"),
        "{abas}"
    );

    // Aba aberta pela página (target=_blank) aparece e pode ser escolhida pelo título.
    nd.ok(&["eval", "document.getElementById('nova').click()"]);
    let fim = Instant::now() + Duration::from_secs(10);
    while nd.ok(&["tabs"]).lines().count() < 3 && Instant::now() < fim {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(nd.ok(&["tabs"]).lines().count(), 3);

    let escolhida = nd.ok(&["tab", "outra"]);
    assert!(escolhida.contains("Outra página"), "{escolhida}");
    assert_eq!(nd.ok(&["eval", "document.title"]).trim(), "Outra página");
    nd.ok(&["tab", "1"]);
    assert_eq!(nd.ok(&["eval", "document.title"]).trim(), "Lento");
    let r = nd.run(&["tab", "9"]);
    assert!(!r.status.success());

    let t = nd.ok(&["text", "--max", "5"]);
    assert!(t.contains("use --max"), "{t}");

    nd.ok(&["stop"]);
    assert!(!nd.estado().exists());
    let r = nd.run(&["status"]);
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("não está rodando"));
}

#[tokio::test(flavor = "multi_thread")]
async fn perfil_em_uso_falha_com_mensagem() {
    let perfil = tempfile::tempdir().unwrap();
    let p = perfil.path().to_str().unwrap();
    let a = Nd::new();
    a.ok(&["start", "--perfil", p, "--headless"]);
    let b = Nd::new();
    let r = b.run(&["start", "--perfil", p, "--headless"]);
    assert!(!r.status.success());
    let err = String::from_utf8_lossy(&r.stderr);
    assert!(err.contains("já está aberto em outro Edge"), "{err}");
    a.ok(&["stop"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn daemon_some_quando_o_edge_morre() {
    let nd = Nd::new();
    nd.ok(&["start", "--perfil", "temp", "--headless"]);
    let estado: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(nd.estado()).unwrap()).unwrap();
    let edge = estado["edge_pid"].as_u64().unwrap();
    let daemon = estado["pid"].as_u64().unwrap();
    Command::new("taskkill")
        .args(["/T", "/F", "/PID", &edge.to_string()])
        .output()
        .unwrap();
    let fim = Instant::now() + Duration::from_secs(20);
    while vivo(daemon) && Instant::now() < fim {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        !vivo(daemon),
        "daemon continuou depois do Edge morrer\n{}",
        nd.log()
    );
    assert!(!nd.estado().exists());
}

fn vivo(pid: u64) -> bool {
    let o = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\""))
}
