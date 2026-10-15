//! Apoio aos testes de ponta a ponta: daemon de verdade, Edge headless com perfil temporário,
//! estado num diretório próprio (`ND_DIR`) e as páginas de `tests/fixtures`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub struct Nd {
    pub dir: tempfile::TempDir,
}

impl Nd {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    pub fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_nd"))
            .args(args)
            .env("ND_DIR", self.dir.path())
            .output()
            .unwrap()
    }

    /// Roda e exige sucesso; devolve o stdout.
    pub fn ok(&self, args: &[&str]) -> String {
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

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("daemon.log")).unwrap_or_default()
    }

    pub fn estado(&self) -> PathBuf {
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

/// Serve `tests/fixtures` em 127.0.0.1 (a página com iframe de outro site o abre por
/// `localhost`); `/api/lenta` responde depois de 1,5 s.
pub async fn servir() -> String {
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

/// Ref da primeira linha do mapa que contém `trecho` ("e12").
pub fn ref_de(mapa: &str, trecho: &str) -> String {
    let l = mapa
        .lines()
        .find(|l| l.contains(trecho))
        .unwrap_or_else(|| panic!("{trecho:?} não está no mapa:\n{mapa}"));
    l.split_whitespace().next().unwrap().to_string()
}

pub fn vivo(pid: u64) -> bool {
    let o = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\""))
}
