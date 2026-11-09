//! Captcha de grade: o `nd step` resolve sozinho quando a página mostra um captcha, e `nd captcha`
//! resolve a aba atual ou um arquivo. Daemon, Edge e página são de verdade; o Decider é simulado:
//! responde "sim" para peça de cor vermelha.

mod comum;

use std::io::Cursor;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use comum::{Nd, servir};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn vermelha(url: &str) -> bool {
    let b64 = url.split_once("base64,").unwrap().1;
    let img = image::load_from_memory(&STANDARD.decode(b64).unwrap()).unwrap().to_rgb8();
    let p = img.get_pixel(img.width() / 2, img.height() / 2);
    p[0] > 150 && p[1] < 100
}

fn resposta(req: &Value) -> Value {
    if req["questions"].get("acao").is_some() {
        return json!({ "answers": {
            "acao": { "choice": "concluir", "confidence": 0.9 },
            "alvo": { "choice": "e1", "confidence": 0.9 },
            "valor": { "choice": "nenhum" } } });
    }
    let estado = req["state"].as_array().unwrap();
    let url = estado.last().unwrap()["image_url"]["url"].as_str().unwrap();
    let sim = vermelha(url);
    json!({ "answers": { "r": {
        "choice": if sim { "sim" } else { "nao" },
        "confidence": 0.9,
        "probabilities": { "sim": if sim { 0.95 } else { 0.05 } } } },
        "usage": { "input_tokens": 10, "output_tokens": 1, "cost": 0.00001 } })
}

async fn decider_simulado() -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let porta = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut p = [0u8; 65536];
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
                let corpo = resposta(&req).to_string();
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

#[test]
fn arquivo_sintetico() {
    // Mesma regra de cor sem Edge: a grade 2x2 do fixture, desenhada em memória.
    let mut img = image::RgbImage::from_pixel(240, 320, image::Rgb([255, 255, 255]));
    for y in 10..90 {
        for x in 10..230 {
            img.put_pixel(x, y, image::Rgb([70, 130, 220]));
        }
    }
    for (i, (x0, y0)) in [(10, 110), (114, 110), (10, 214), (114, 214)].into_iter().enumerate() {
        for y in y0..y0 + 100 {
            for x in x0..x0 + 100 {
                let c = if i == 1 { [210, 40, 40] } else { [40, 150, 60] };
                img.put_pixel(x, y, image::Rgb(c));
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("c.png");
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png).unwrap();
    std::fs::write(&f, buf).unwrap();
    let nd = Nd::new();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let url = rt.block_on(decider_simulado());
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_nd"))
        .args(["captcha", f.to_str().unwrap()])
        .env("ND_DIR", nd.dir.path())
        .env("ND_DECIDER_URL", url)
        .env("OPENROUTER_API_KEY", "chave-de-teste")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{out}\n{}", String::from_utf8_lossy(&o.stderr));
    assert!(out.contains("instrução: lida do cabeçalho da imagem (grade 2x2)\n. X\n. ."), "{out}");
    assert!(out.contains("4 chamadas"), "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn step_resolve_o_captcha_sozinho() {
    let url = decider_simulado().await;
    unsafe {
        std::env::set_var("ND_DECIDER_URL", &url);
        std::env::set_var("OPENROUTER_API_KEY", "chave-de-teste");
    }
    let base = servir().await;
    let nd = Nd::new();
    nd.ok(&["start", "--perfil", "temp", "--headless"]);
    nd.ok(&["open", &format!("{base}/captcha.html"), "--sem-map"]);

    let s = nd.ok(&["step", "Passar pelo captcha"]);
    assert!(s.contains("1. captcha: 2 peças marcadas para"), "{s}");
    assert!(s.contains("X .\n. X"), "{s}");
    assert!(s.contains("concluído"), "{s}");
    assert_eq!(nd.ok(&["text", "--sel", "#sel"]).trim(), "0,3");

    // Sem arquivo, `nd captcha` usa a aba atual; --seco não clica.
    let s = nd.ok(&["captcha", "--seco"]);
    assert!(s.contains("X .\n. X"), "{s}");
}
