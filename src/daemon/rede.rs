//! Requisições em andamento por aba, a partir dos eventos `Network.*`.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nodriver::RawEvent;

/// Tipos de recurso de vida longa, que nunca terminam e não contam para "rede ociosa".
const LONGOS: &[&str] = &["EventSource", "WebSocket"];

struct Atividade {
    em_voo: HashSet<String>,
    ultima: Instant,
}

#[derive(Default)]
pub struct Rede {
    abas: Mutex<HashMap<String, Atividade>>,
}

impl Rede {
    pub fn evento(&self, alvo: &str, ev: &RawEvent) {
        let id = ev.params.get("requestId").and_then(|v| v.as_str());
        let mut abas = self.abas.lock().unwrap();
        let a = abas.entry(alvo.to_string()).or_insert_with(|| Atividade {
            em_voo: HashSet::new(),
            ultima: Instant::now(),
        });
        match (ev.method.as_str(), id) {
            ("Network.requestWillBeSent", Some(id)) => {
                let tipo = ev.params.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if !LONGOS.contains(&tipo) {
                    a.em_voo.insert(id.to_string());
                }
                a.ultima = Instant::now();
            }
            ("Network.loadingFinished" | "Network.loadingFailed", Some(id)) => {
                a.em_voo.remove(id);
                a.ultima = Instant::now();
            }
            _ => {}
        }
    }

    /// (requisições em andamento, tempo desde a última atividade)
    pub fn situacao(&self, alvo: &str) -> (usize, Duration) {
        let abas = self.abas.lock().unwrap();
        match abas.get(alvo) {
            Some(a) => (a.em_voo.len(), a.ultima.elapsed()),
            None => (0, Duration::MAX),
        }
    }

    /// Esquece as requisições de uma aba (ex.: ao navegar, as da página anterior não voltam).
    pub fn limpar(&self, alvo: &str) {
        if let Some(a) = self.abas.lock().unwrap().get_mut(alvo) {
            a.em_voo.clear();
            a.ultima = Instant::now();
        }
    }

    pub fn remover(&self, alvo: &str) {
        self.abas.lock().unwrap().remove(alvo);
    }
}
