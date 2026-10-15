//! Cliente HTTP síncrono do daemon (sem tokio, para responder rápido).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::estado::Estado;

#[derive(Debug, Serialize, Deserialize)]
pub struct Pedido {
    pub cmd: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Resposta {
    pub ok: bool,
    #[serde(default)]
    pub saida: String,
    #[serde(default)]
    pub erro: String,
}

/// Manda um comando ao daemon e devolve a resposta. `Err` = daemon inacessível.
pub fn enviar(estado: &Estado, pedido: &Pedido, timeout: Duration) -> Result<Resposta, String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], estado.porta));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .map_err(|e| format!("daemon não responde em {addr}: {e}"))?;
    s.set_read_timeout(Some(timeout)).ok();
    let corpo = serde_json::to_vec(pedido).map_err(|e| e.to_string())?;
    let cabecalho = format!(
        "POST /cmd HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        estado.porta,
        estado.token,
        corpo.len()
    );
    s.write_all(cabecalho.as_bytes()).map_err(|e| e.to_string())?;
    s.write_all(&corpo).map_err(|e| e.to_string())?;

    let mut buf = Vec::new();
    let mut pedaco = [0u8; 8192];
    let (inicio, tamanho) = loop {
        let n = s
            .read(&mut pedaco)
            .map_err(|e| format!("lendo resposta do daemon: {e}"))?;
        if n == 0 {
            return Err("daemon fechou a conexão sem responder".into());
        }
        buf.extend_from_slice(&pedaco[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let cab = String::from_utf8_lossy(&buf[..pos]).to_ascii_lowercase();
            let tam = cab
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok());
            break (pos + 4, tam);
        }
    };
    match tamanho {
        Some(t) => {
            while buf.len() < inicio + t {
                let n = s.read(&mut pedaco).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&pedaco[..n]);
            }
        }
        None => {
            s.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        }
    }
    let fim = tamanho.map_or(buf.len(), |t| (inicio + t).min(buf.len()));
    serde_json::from_slice(&buf[inicio..fim]).map_err(|e| format!("resposta inválida do daemon: {e}"))
}
