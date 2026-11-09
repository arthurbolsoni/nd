//! `nd step` com um Decider simulado (servidor HTTP local que responde como o OpenRouter):
//! o daemon, o Edge e a página são de verdade; só a decisão é combinada.

mod comum;

use comum::{Nd, servir};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Servidor que, no passo 0 (histórico vazio), manda preencher o campo "Nome" com o dado `nome`
/// e, depois, concluir. Acha a ref do campo pela descrição que o `nd step` manda em `alvo`.
async fn decider_simulado() -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let porta = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut p = [0u8; 8192];
                let (ini, tam) = loop {
                    let n = s.read(&mut p).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&p[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let cab = String::from_utf8_lossy(&buf[..pos]).to_ascii_lowercase();
                        let t = cab
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        break (pos + 4, t);
                    }
                };
                while buf.len() < ini + tam {
                    let n = s.read(&mut p).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&p[..n]);
                }
                let req: Value = serde_json::from_slice(&buf[ini..ini + tam]).unwrap();
                let passo0 = req["state"]["historico"].as_array().map_or(true, |h| h.is_empty());
                let nome = req["questions"]["alvo"]["criteria"]
                    .as_object()
                    .and_then(|m| m.iter().find(|(_, d)| d.as_str().unwrap_or("").contains("\"Nome\"")))
                    .map(|(r, _)| r.clone())
                    .unwrap_or_default();
                let corpo = json!({
                    "answers": {
                        "acao": { "choice": if passo0 { "preencher" } else { "concluir" }, "confidence": 0.9 },
                        "alvo": { "choice": nome, "confidence": 0.8 },
                        "valor": { "choice": "nome" },
                    },
                    "usage": { "input_tokens": 100, "output_tokens": 3, "cost": 0.0001 },
                })
                .to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{corpo}",
                    corpo.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
            });
        }
    });
    format!("http://127.0.0.1:{porta}/decisions")
}

#[tokio::test(flavor = "multi_thread")]
async fn step_com_decider_simulado() {
    let url = decider_simulado().await;
    // Processo de teste único para este arquivo: o `nd step` lê o ambiente do filho.
    unsafe {
        std::env::set_var("ND_DECIDER_URL", &url);
        std::env::set_var("OPENROUTER_API_KEY", "chave-de-teste");
    }
    let base = servir().await;
    let nd = Nd::new();
    nd.ok(&["start", "--perfil", "temp", "--headless"]);
    nd.ok(&["open", &format!("{base}/form.html"), "--sem-map"]);

    let s = nd.ok(&["step", "Preencher o nome", "-d", "nome=Arthur"]);
    assert!(s.contains("não é LLM"), "{s}");
    assert!(s.contains("1. preencher"), "{s}");
    assert!(s.contains("2. concluir"), "{s}");
    assert!(s.contains("2 chamadas, 200 tokens de entrada, 6 de saída, US$ 0.0002"), "{s}");
    assert_eq!(nd.ok(&["text", "--sel", "#eco"]).trim(), "Arthur");

    // Limite de passos: devolve com código 2 e o mapa atual para quem chamou assumir.
    let r = nd.run(&["step", "Preencher o nome", "-d", "nome=Outro", "--max", "1"]);
    assert_eq!(r.status.code(), Some(2));
    let out = String::from_utf8_lossy(&r.stdout);
    assert!(out.contains("--- mapa atual ---"), "{out}");
    assert!(String::from_utf8_lossy(&r.stderr).contains("limite de 1 passos"));

    // Dado sem '=' é erro de uso.
    let r = nd.run(&["step", "x", "-d", "semigual"]);
    assert_eq!(r.status.code(), Some(1));
}
