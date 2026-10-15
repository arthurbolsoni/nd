//! Requisições por aba, a partir dos eventos `Network.*`: as que estão em andamento (para a espera
//! de rede ociosa) e as últimas concluídas (para `nd wait net`).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nodriver::RawEvent;

/// Tipos de recurso que contam para "rede ociosa": os que mudam a página. Imagem, fonte,
/// mídia, beacon e conexões longas (EventSource, WebSocket) ficam de fora.
const CONTAM: &[&str] = &["Document", "Stylesheet", "Script", "XHR", "Fetch"];
/// Anúncio, métricas e rastreamento: não contam para rede ociosa (vêm em cadeia por segundos e
/// alguns nunca terminam, como `fetchLater`). Por trecho do host...
const RUIDO_HOST: &[&str] = &[
    "doubleclick.net",
    "googlesyndication.com",
    "google-analytics.com",
    "googletagmanager.com",
    "googletagservices.com",
    "googleadservices.com",
    "adservice.google.",
    "fundingchoicesmessages.google.com",
    "adtrafficquality.google",
    "connect.facebook.net",
    "hotjar.",
    "clarity.ms",
    "scorecardresearch.com",
    "criteo.",
    "taboola.com",
    "outbrain.com",
    "amazon-adsystem.com",
    "rubiconproject.com",
    "pubmatic.com",
    "permutive.",
    "prebid",
    "newrelic.com",
    "nr-data.net",
    "sentry.io",
    "datadoghq.",
    "segment.io",
    "segment.com",
    "mixpanel.com",
    "amplitude.com",
    "fullstory.com",
    "quantserve.com",
    "chartbeat.",
    "bat.bing.com",
    "doubleverify.com",
    "adnxs.com",
    "smartadserver.com",
    "openx.net",
    "liadm.com",
    "crwdcntrl.net",
    "id5-sync.com",
    "ads-twitter.com",
    "creativecdn.com",
    "mrf.io",
    "adsrvr.org",
    "casalemedia.com",
    "teads.tv",
    "moatads.com",
    "adform.net",
    "bidswitch.net",
    "media.net",
    "lijit.com",
    "33across.com",
    "sharethrough.com",
    "yieldmo.com",
    "tiktok.com/i18n/pixel",
    "analytics.tiktok.com",
];
/// ...e por rótulo inteiro do host (`analytics.site.com`, `ads.site.com`).
const RUIDO_ROTULO: &[&str] = &[
    "ads", "ad", "adserver", "analytics", "metrics", "telemetry", "tracking", "tracker", "pixel",
    "stats", "beacon", "collect", "collector", "events", "event", "log", "logs", "rum",
];

fn host_de(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or("")
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Domínio registrável aproximado: dois últimos rótulos, ou três quando o penúltimo é de segundo
/// nível de país (`site.com.br`, `site.co.uk`). IP e localhost ficam inteiros.
fn site_de(url: &str) -> String {
    let host = host_de(url);
    if host.parse::<std::net::IpAddr>().is_ok() || !host.contains('.') {
        return host;
    }
    let r: Vec<&str> = host.split('.').collect();
    let n = r.len();
    let segundo_nivel = ["com", "net", "org", "gov", "edu", "co", "ac", "gob", "mil", "art", "blog", "app"];
    let k = if n >= 3 && r[n - 1].len() == 2 && segundo_nivel.contains(&r[n - 2]) {
        3
    } else {
        2
    };
    r[n.saturating_sub(k)..].join(".")
}

/// ...e pelo último trecho do caminho (`/collect`, `/event`).
const RUIDO_CAMINHO: &[&str] = &["collect", "event", "events", "track", "beacon", "gen_204", "ping", "pixel"];

fn ruido(url: &str) -> bool {
    if url.starts_with("blob:") || url.starts_with("data:") {
        return true;
    }
    if url.contains("facebook.com/tr") {
        return true;
    }
    let resto = url.split("://").nth(1).unwrap_or("");
    let caminho = resto.split(['?', '#']).next().unwrap_or("");
    let ultimo = caminho.rsplit('/').next().unwrap_or("");
    if caminho.contains('/') && RUIDO_CAMINHO.contains(&ultimo) {
        return true;
    }
    let host = host_de(url);
    RUIDO_HOST.iter().any(|d| host.contains(d)) || host.split('.').any(|l| RUIDO_ROTULO.contains(&l))
}

/// Requisição aberta há mais que isto deixa de segurar a espera (streaming, long-poll).
const VELHA: Duration = Duration::from_secs(5);
/// Requisições concluídas guardadas por aba.
const MAX_CONCLUIDAS: usize = 500;

struct Pedido {
    tipo: String,
    inicio: Instant,
    metodo: String,
    url: String,
    status: i64,
    longo: bool,
}

pub struct Concluida {
    pub quando: Instant,
    pub metodo: String,
    pub url: String,
    /// Status HTTP; 0 quando a requisição falhou.
    pub status: i64,
}

struct Atividade {
    /// Site do documento principal da aba (de `site_de`).
    site: Option<String>,
    pedidos: HashMap<String, Pedido>,
    ultima: Instant,
    concluidas: VecDeque<Concluida>,
}

impl Atividade {
    fn nova() -> Self {
        Self {
            site: None,
            pedidos: HashMap::new(),
            ultima: Instant::now(),
            concluidas: VecDeque::new(),
        }
    }
}

#[derive(Default)]
pub struct Rede {
    abas: Mutex<HashMap<String, Atividade>>,
}

impl Rede {
    pub fn evento(&self, alvo: &str, ev: &RawEvent) {
        let p = &ev.params;
        let Some(id) = p.get("requestId").and_then(|v| v.as_str()) else {
            return;
        };
        let mut abas = self.abas.lock().unwrap();
        let a = abas.entry(alvo.to_string()).or_insert_with(Atividade::nova);
        match ev.method.as_str() {
            "Network.requestWillBeSent" => {
                let tipo = p["type"].as_str().unwrap_or("");
                // Documento de iframe: se o iframe é de outro site, ele termina na sessão do
                // target dele, que não é acompanhada aqui; não entra na conta de rede ociosa.
                let iframe = tipo == "Document" && p["frameId"].as_str() != Some(alvo);
                let url = p["request"]["url"].as_str().unwrap_or("");
                if tipo == "Document" && !iframe {
                    a.site = Some(site_de(url));
                }
                // XHR/fetch de terceiros (anúncio, métricas, widgets) não conta; script e CSS de
                // qualquer origem contam (é o código da página, às vezes num CDN de outro domínio).
                let terceiro = matches!(tipo, "XHR" | "Fetch")
                    && a.site.as_ref().is_some_and(|s| *s != site_de(url));
                let ruido = terceiro || ruido(url);
                a.pedidos.insert(
                    id.to_string(),
                    Pedido {
                        tipo: tipo.to_string(),
                        inicio: Instant::now(),
                        metodo: p["request"]["method"].as_str().unwrap_or("GET").to_string(),
                        url: p["request"]["url"].as_str().unwrap_or("").to_string(),
                        status: 0,
                        longo: iframe || ruido || !CONTAM.contains(&tipo),
                    },
                );
                a.ultima = Instant::now();
            }
            "Network.responseReceived" => {
                if let Some(x) = a.pedidos.get_mut(id) {
                    x.status = p["response"]["status"].as_i64().unwrap_or(0);
                    // Documento com resposta começada: o resto é o HTML chegando (às vezes em
                    // streaming por segundos); a espera passa a ser a do DOM parado.
                    if x.tipo == "Document" {
                        x.longo = true;
                    }
                }
            }
            "Network.loadingFinished" | "Network.loadingFailed" => {
                if let Some(x) = a.pedidos.remove(id) {
                    if !x.longo {
                        tracing::debug!(
                            "rede: {} {} {} em {} ms",
                            x.tipo,
                            x.metodo,
                            x.url.chars().take(120).collect::<String>(),
                            x.inicio.elapsed().as_millis()
                        );
                    }
                    if a.concluidas.len() >= MAX_CONCLUIDAS {
                        a.concluidas.pop_front();
                    }
                    let falhou = ev.method == "Network.loadingFailed";
                    a.concluidas.push_back(Concluida {
                        quando: Instant::now(),
                        metodo: x.metodo,
                        url: x.url,
                        status: if falhou { 0 } else { x.status },
                    });
                }
                a.ultima = Instant::now();
            }
            _ => {}
        }
    }

    /// (requisições em andamento, tempo desde a última atividade)
    pub fn situacao(&self, alvo: &str) -> (usize, Duration) {
        let abas = self.abas.lock().unwrap();
        match abas.get(alvo) {
            Some(a) => (
                a.pedidos
                    .values()
                    .filter(|p| !p.longo && p.inicio.elapsed() < VELHA)
                    .count(),
                a.ultima.elapsed(),
            ),
            None => (0, Duration::MAX),
        }
    }

    /// Requisições em andamento (as que contam para a rede ociosa): "tipo url", há quanto tempo.
    pub fn pendentes(&self, alvo: &str) -> Vec<(String, Duration)> {
        let abas = self.abas.lock().unwrap();
        abas.get(alvo)
            .map(|a| {
                a.pedidos
                    .values()
                    .filter(|p| !p.longo)
                    .map(|p| (format!("{} {}", p.tipo, p.url), p.inicio.elapsed()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Primeira requisição concluída depois de `desde` com `trecho` na URL: "GET 200 url".
    pub fn concluida_desde(&self, alvo: &str, desde: Instant, trecho: &str) -> Option<String> {
        let abas = self.abas.lock().unwrap();
        let c = abas
            .get(alvo)?
            .concluidas
            .iter()
            .find(|c| c.quando >= desde && c.url.contains(trecho))?;
        let status = if c.status == 0 {
            "falhou".to_string()
        } else {
            c.status.to_string()
        };
        Some(format!("{} {status} {}", c.metodo, c.url))
    }

    /// Esquece as requisições em andamento de uma aba (ao navegar, as da página anterior não
    /// voltam).
    pub fn limpar(&self, alvo: &str) {
        if let Some(a) = self.abas.lock().unwrap().get_mut(alvo) {
            a.pedidos.clear();
            a.ultima = Instant::now();
        }
    }

    pub fn remover(&self, alvo: &str) {
        self.abas.lock().unwrap().remove(alvo);
    }
}

#[cfg(test)]
mod testes {
    use super::{ruido, site_de};

    #[test]
    fn ruido_por_host() {
        assert!(ruido("https://p.clarity.ms/collect"));
        assert!(ruido("https://collector.github.com/github/collect"));
        assert!(ruido("https://ab.g.globo/event"));
        assert!(ruido("https://events.newsroom.bi/x"));
        assert!(ruido("blob:https://g1.globo.com/abc"));
        assert!(!ruido("https://api.github.com/repos"));
        assert!(!ruido("https://uploads.site.com/a"));
        assert!(!ruido("https://g1.globo.com/api/feed"));
    }

    #[test]
    fn site() {
        assert_eq!(site_de("https://g1.globo.com/a"), "globo.com");
        assert_eq!(site_de("https://www.mercadolivre.com.br/"), "mercadolivre.com.br");
        assert_eq!(site_de("https://a.b.co.uk:8080/x"), "b.co.uk");
        assert_eq!(site_de("http://127.0.0.1:5000/x"), "127.0.0.1");
        assert_eq!(site_de("http://localhost:5000/x"), "localhost");
        assert_eq!(site_de("https://www.facebook.com/"), "facebook.com");
    }
}
