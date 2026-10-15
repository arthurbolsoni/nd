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

/// Lê uma requisição HTTP inteira: (método, caminho com query, cabeçalhos em minúsculas, corpo).
async fn ler_requisicao(s: &mut tokio::net::TcpStream) -> (String, String, Vec<(String, String)>, Vec<u8>) {
    let mut buf = Vec::new();
    let mut pedaco = [0u8; 8192];
    let fim_cab = loop {
        let n = s.read(&mut pedaco).await.unwrap_or(0);
        if n == 0 {
            break buf.len();
        }
        buf.extend_from_slice(&pedaco[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let cab = String::from_utf8_lossy(&buf[..fim_cab.min(buf.len())]).to_string();
    let mut linhas = cab.lines();
    let primeira = linhas.next().unwrap_or("");
    let mut partes = primeira.split_whitespace();
    let metodo = partes.next().unwrap_or("GET").to_string();
    let caminho = partes.next().unwrap_or("/").to_string();
    let cabecalhos: Vec<(String, String)> = linhas
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let tamanho: usize = cabecalhos
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut corpo = buf[fim_cab.min(buf.len())..].to_vec();
    while corpo.len() < tamanho {
        let n = s.read(&mut pedaco).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        corpo.extend_from_slice(&pedaco[..n]);
    }
    (metodo, caminho, cabecalhos, corpo)
}

/// Serve `tests/fixtures` em 127.0.0.1 (a página com iframe de outro site o abre por
/// `localhost`). APIs: `/api/lenta` (1,5 s), `/api/lista?pagina=N`, `/api/eco` (devolve método,
/// query, corpo, `x-token` e cookie), `/api/stream` (dois JSON em sequência), `/arquivo.bin`.
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
                let (metodo, caminho, cab, corpo_req) = ler_requisicao(&mut s).await;
                let (rota, query) = caminho.split_once('?').unwrap_or((&caminho, ""));
                let json = "application/json";
                let (status, tipo, corpo): (&str, &str, Vec<u8>) = match rota {
                    "/api/lenta" => {
                        tokio::time::sleep(Duration::from_millis(1500)).await;
                        ("200 OK", json, br#"{"valor":42}"#.to_vec())
                    }
                    "/api/lista" => {
                        let pagina: i64 = query
                            .split('&')
                            .find_map(|p| p.strip_prefix("pagina="))
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(1);
                        let v = serde_json::json!({
                            "itens": [{"id": 1, "nome": "a"}, {"id": 2, "nome": "b"}, {"id": 3, "nome": "c"}],
                            "pagina": pagina,
                            "total": 9,
                        });
                        ("200 OK", json, v.to_string().into_bytes())
                    }
                    "/api/eco" => {
                        let h = |n: &str| cab.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
                        let v = serde_json::json!({
                            "metodo": metodo,
                            "query": query,
                            "corpo": String::from_utf8_lossy(&corpo_req),
                            "token": h("x-token"),
                            "cookie": h("cookie"),
                        });
                        ("200 OK", json, v.to_string().into_bytes())
                    }
                    "/api/stream" => ("200 OK", json, b"{\"a\":1}\n{\"b\":[1,2]}\n".to_vec()),
                    "/arquivo.bin" => ("200 OK", "application/octet-stream", (0u8..=255).collect()),
                    _ => match std::fs::read(raiz.join(rota.trim_start_matches('/'))) {
                        Ok(b) => ("200 OK", "text/html; charset=utf-8", b),
                        Err(_) => ("404 Not Found", "text/plain", b"404".to_vec()),
                    },
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
