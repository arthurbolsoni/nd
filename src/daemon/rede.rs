//! Rede a partir dos eventos `Network.*`: requisições em andamento por aba (para a espera de rede
//! ociosa) e as últimas chamadas XHR/fetch/documento de todas as abas, com corpo enviado e corpo
//! da resposta em texto (para `nd net`, `nd api` e `nd wait net`).

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
    "ads",
    "ad",
    "adserver",
    "analytics",
    "metrics",
    "telemetry",
    "tracking",
    "tracker",
    "pixel",
    "stats",
    "beacon",
    "collect",
    "collector",
    "events",
    "event",
    "log",
    "logs",
    "rum",
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
    let segundo_nivel = [
        "com", "net", "org", "gov", "edu", "co", "ac", "gob", "mil", "art", "blog", "app",
    ];
    let k = if n >= 3 && r[n - 1].len() == 2 && segundo_nivel.contains(&r[n - 2]) {
        3
    } else {
        2
    };
    r[n.saturating_sub(k)..].join(".")
}

/// ...e pelo último trecho do caminho (`/collect`, `/event`).
const RUIDO_CAMINHO: &[&str] = &[
    "collect",
    "event",
    "events",
    "track",
    "beacon",
    "gen_204",
    "ping",
    "pixel",
    // log do Facebook (banzai)
    "bz",
    "bnzai",
    "logging_client_events",
];

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

/// Carregamento de código que entra na espera mas não é dado para `nd net`: módulos do
/// Facebook (`/ajax/bootloader-endpoint`).
fn fora_do_buffer(url: &str) -> bool {
    url.contains("/ajax/bootloader-endpoint")
}

/// Requisição aberta há mais que isto deixa de segurar a espera (streaming, long-poll).
const VELHA: Duration = Duration::from_secs(5);
/// Chamadas guardadas (todas as abas).
const MAX_CHAMADAS: usize = 500;
/// Corpo guardado por chamada (enviado e resposta), em bytes.
pub const MAX_CORPO: usize = 200 * 1024;
/// Soma dos corpos guardados; acima disso, os das chamadas mais antigas saem.
const MAX_CORPOS_TOTAL: usize = 40 * 1024 * 1024;
/// Tipos de recurso guardados como chamada.
const GUARDA: &[&str] = &["XHR", "Fetch", "Document"];

struct Pedido {
    tipo: String,
    inicio: Instant,
    url: String,
    longo: bool,
}

/// Alvo filho de onde veio a chamada: iframe de outro site (processo próprio) ou worker.
#[derive(Debug, Clone)]
pub struct Sub {
    pub alvo: String,
    /// `iframe`, `worker`, `shared_worker`, `service_worker`.
    pub tipo: String,
}

/// Uma chamada de rede guardada.
#[derive(Debug, Clone)]
pub struct Chamada {
    /// Número para `nd net --corpo N` e `nd api N`.
    pub n: u64,
    pub aba: String,
    pub id: String,
    pub tipo: String,
    /// Documento do frame principal da aba (início de uma página).
    pub principal: bool,
    pub metodo: String,
    pub url: String,
    /// 0 enquanto não há resposta.
    pub status: i64,
    pub mime: String,
    pub cabecalhos: Vec<(String, String)>,
    pub enviado: Option<String>,
    pub resposta: Option<String>,
    /// A resposta passou de `MAX_CORPO` e foi cortada.
    pub cortada: bool,
    /// A resposta é binária (não guardada).
    pub binaria: bool,
    /// Navegação que o navegador transformou em download.
    pub download: bool,
    /// Feita por um iframe de outro site ou por um worker (None = a página da aba).
    pub sub: Option<Sub>,
    /// Bytes recebidos (`encodedDataLength`).
    pub tamanho: u64,
    pub inicio: Instant,
    pub fim: Option<Instant>,
    pub erro: Option<String>,
}

impl Chamada {
    pub fn em_andamento(&self) -> bool {
        self.fim.is_none()
    }

    fn bytes(&self) -> usize {
        self.enviado.as_ref().map_or(0, String::len) + self.resposta.as_ref().map_or(0, String::len)
    }
}

/// Corpo que o daemon precisa pedir ao navegador (`Network.getResponseBody`,
/// `Network.getRequestPostData`).
pub enum Buscar {
    Resposta(String),
    Enviado(String),
}

struct Atividade {
    /// Site do documento principal da aba (de `site_de`).
    site: Option<String>,
    pedidos: HashMap<String, Pedido>,
    ultima: Instant,
}

impl Atividade {
    fn nova() -> Self {
        Self {
            site: None,
            pedidos: HashMap::new(),
            ultima: Instant::now(),
        }
    }
}

#[derive(Default)]
struct Chamadas {
    lista: VecDeque<Chamada>,
    prox: u64,
    bytes: usize,
}

impl Chamadas {
    fn achar(&mut self, aba: &str, id: &str) -> Option<&mut Chamada> {
        self.lista.iter_mut().rev().find(|c| c.aba == aba && c.id == id)
    }

    fn incluir(&mut self, mut c: Chamada) {
        if self.lista.len() >= MAX_CHAMADAS
            && let Some(velha) = self.lista.pop_front()
        {
            self.bytes -= velha.bytes();
        }
        self.prox += 1;
        c.n = self.prox;
        self.bytes += c.bytes();
        self.lista.push_back(c);
        self.aparar();
    }

    /// Tira os corpos das chamadas mais antigas até a soma caber em `MAX_CORPOS_TOTAL`.
    fn aparar(&mut self) {
        for c in self.lista.iter_mut() {
            if self.bytes <= MAX_CORPOS_TOTAL {
                break;
            }
            self.bytes -= c.bytes();
            c.enviado = None;
            c.resposta = None;
        }
    }
}

/// Corta `s` em até `max` bytes, sem partir caractere. Devolve (texto, cortado).
pub fn cortar_bytes(s: &str, max: usize) -> (String, bool) {
    if s.len() <= max {
        return (s.to_string(), false);
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    (s[..i].to_string(), true)
}

fn texto(mime: &str, url: &str) -> bool {
    let m = mime.to_ascii_lowercase();
    m.contains("json")
        || m.starts_with("text/")
        || m.contains("xml")
        || m.contains("javascript")
        || m.contains("x-www-form-urlencoded")
        || url.split('?').next().unwrap_or("").ends_with(".json")
}

#[derive(Default)]
pub struct Rede {
    abas: Mutex<HashMap<String, Atividade>>,
    chamadas: Mutex<Chamadas>,
}

impl Rede {
    /// Atualiza o estado com um evento da aba `alvo` (vindo da sessão da aba, ou de um iframe ou
    /// worker dela em `sub`) e diz quais corpos buscar.
    pub fn evento(&self, alvo: &str, sub: Option<&Sub>, ev: &RawEvent) -> Vec<Buscar> {
        let p = &ev.params;
        let Some(id) = p.get("requestId").and_then(|v| v.as_str()) else {
            return Vec::new();
        };
        let mut buscar = Vec::new();
        let mut abas = self.abas.lock().unwrap();
        let a = abas.entry(alvo.to_string()).or_insert_with(Atividade::nova);
        match ev.method.as_str() {
            "Network.requestWillBeSent" => {
                let tipo = p["type"].as_str().unwrap_or("");
                // Documento de iframe: se o iframe é de outro site, ele termina na sessão do
                // target dele, que não é acompanhada aqui; não entra na conta de rede ociosa.
                let iframe = tipo == "Document" && p["frameId"].as_str() != Some(alvo);
                let req = &p["request"];
                let url = req["url"].as_str().unwrap_or("");
                if tipo == "Document" && !iframe {
                    a.site = Some(site_de(url));
                }
                // XHR/fetch de terceiros (anúncio, métricas, widgets) não conta; script e CSS de
                // qualquer origem contam (é o código da página, às vezes num CDN de outro domínio).
                let terceiro =
                    matches!(tipo, "XHR" | "Fetch") && a.site.as_ref().is_some_and(|s| *s != site_de(url));
                let eh_ruido = ruido(url);
                a.pedidos.insert(
                    id.to_string(),
                    Pedido {
                        tipo: tipo.to_string(),
                        inicio: Instant::now(),
                        url: url.to_string(),
                        // Iframe de outro site e worker não seguram a espera da aba (`nd wait
                        // net` espera por eles).
                        longo: iframe || terceiro || eh_ruido || sub.is_some() || !CONTAM.contains(&tipo),
                    },
                );
                a.ultima = Instant::now();
                drop(abas);

                let mut ch = self.chamadas.lock().unwrap();
                // Redirecionamento: mesmo requestId, URL nova.
                if let Some(c) = ch.achar(alvo, id) {
                    if p.get("redirectResponse").is_some() {
                        c.url = url.to_string();
                    } else if c.sub.is_none() {
                        // Documento do iframe: começa na sessão da aba e segue na do iframe.
                        c.sub = sub.cloned();
                    }
                    return buscar;
                }
                if !GUARDA.contains(&tipo) || eh_ruido || fora_do_buffer(url) {
                    return buscar;
                }
                let cabecalhos: Vec<(String, String)> = req["headers"]
                    .as_object()
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let enviado = req["postData"]
                    .as_str()
                    .map(|s| cortar_bytes(&mascarar_corpo(s), MAX_CORPO).0);
                if enviado.is_none() && req["hasPostData"].as_bool() == Some(true) {
                    buscar.push(Buscar::Enviado(id.to_string()));
                }
                ch.incluir(Chamada {
                    n: 0,
                    aba: alvo.to_string(),
                    id: id.to_string(),
                    tipo: tipo.to_string(),
                    principal: tipo == "Document" && !iframe,
                    metodo: req["method"].as_str().unwrap_or("GET").to_string(),
                    url: mascarar_url(url),
                    status: 0,
                    mime: String::new(),
                    cabecalhos,
                    enviado,
                    resposta: None,
                    cortada: false,
                    binaria: false,
                    download: false,
                    sub: sub.cloned(),
                    tamanho: 0,
                    inicio: Instant::now(),
                    fim: None,
                    erro: None,
                });
            }
            "Network.responseReceived" => {
                if let Some(x) = a.pedidos.get_mut(id) {
                    // Documento com resposta começada: o resto é o HTML chegando (às vezes em
                    // streaming por segundos); a espera passa a ser a do DOM parado.
                    if x.tipo == "Document" {
                        x.longo = true;
                    }
                }
                drop(abas);
                if let Some(c) = self.chamadas.lock().unwrap().achar(alvo, id) {
                    c.status = p["response"]["status"].as_i64().unwrap_or(0);
                    c.mime = p["response"]["mimeType"].as_str().unwrap_or("").to_string();
                }
            }
            "Network.loadingFinished" | "Network.loadingFailed" => {
                if let Some(x) = a.pedidos.remove(id)
                    && !x.longo
                {
                    tracing::debug!(
                        "rede: {} {} em {} ms",
                        x.tipo,
                        x.url.chars().take(120).collect::<String>(),
                        x.inicio.elapsed().as_millis()
                    );
                }
                a.ultima = Instant::now();
                drop(abas);
                if let Some(c) = self.chamadas.lock().unwrap().achar(alvo, id) {
                    c.fim = Some(Instant::now());
                    if ev.method == "Network.loadingFailed" && !c.download {
                        let e = p["errorText"].as_str().unwrap_or("falhou");
                        c.erro = Some(if p["canceled"].as_bool() == Some(true) {
                            format!("cancelada ({e})")
                        } else {
                            e.to_string()
                        });
                    } else {
                        c.tamanho = p["encodedDataLength"].as_f64().unwrap_or(0.0) as u64;
                        if c.tipo != "Document"
                            && texto(&c.mime, &c.url)
                            && c.tamanho <= 20 * MAX_CORPO as u64
                        {
                            buscar.push(Buscar::Resposta(id.to_string()));
                        } else if c.tipo != "Document" && !texto(&c.mime, &c.url) {
                            c.binaria = true;
                        }
                    }
                }
            }
            _ => {}
        }
        buscar
    }

    /// Guarda a resposta de `Network.getResponseBody`.
    pub fn com_resposta(&self, aba: &str, id: &str, corpo: &str, base64: bool) {
        let mut ch = self.chamadas.lock().unwrap();
        let Some(c) = ch.achar(aba, id) else { return };
        if base64 {
            c.binaria = true;
            return;
        }
        let (t, cortado) = cortar_bytes(corpo, MAX_CORPO);
        let antes = c.bytes();
        c.resposta = Some(t);
        c.cortada = cortado;
        let depois = c.bytes();
        ch.bytes = ch.bytes + depois - antes;
        ch.aparar();
    }

    /// Guarda o corpo enviado de `Network.getRequestPostData`.
    pub fn com_enviado(&self, aba: &str, id: &str, corpo: &str) {
        let mut ch = self.chamadas.lock().unwrap();
        let Some(c) = ch.achar(aba, id) else { return };
        let antes = c.bytes();
        c.enviado = Some(cortar_bytes(&mascarar_corpo(corpo), MAX_CORPO).0);
        let depois = c.bytes();
        ch.bytes = ch.bytes + depois - antes;
        ch.aparar();
    }

    /// Roda `f` com as chamadas guardadas (da mais antiga para a mais nova).
    /// Marca o documento de `url` (o mais recente) como download: o navegador cancela a navegação
    /// e baixa a resposta.
    pub fn virou_download(&self, url: &str) {
        let mut ch = self.chamadas.lock().unwrap();
        if let Some(c) = ch
            .lista
            .iter_mut()
            .rev()
            .find(|c| c.tipo == "Document" && c.url == url)
        {
            c.download = true;
            c.erro = None;
        }
    }

    pub fn com_chamadas<T>(&self, f: impl FnOnce(&VecDeque<Chamada>) -> T) -> T {
        f(&self.chamadas.lock().unwrap().lista)
    }

    pub fn chamada(&self, n: u64) -> Option<Chamada> {
        self.chamadas
            .lock()
            .unwrap()
            .lista
            .iter()
            .find(|c| c.n == n)
            .cloned()
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

    /// Primeira chamada da aba concluída depois de `desde` com `trecho` na URL: "#n GET 200 url".
    pub fn concluida_desde(&self, alvo: &str, desde: Instant, trecho: &str) -> Option<String> {
        self.com_chamadas(|l| {
            let c = l
                .iter()
                .find(|c| c.aba == alvo && c.fim.is_some_and(|f| f >= desde) && c.url.contains(trecho))?;
            let status = match &c.erro {
                Some(e) => format!("falhou ({e})"),
                None => c.status.to_string(),
            };
            Some(format!("#{} {} {status} {}", c.n, c.metodo, c.url))
        })
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

/// O que entra no lugar de uma senha no buffer.
pub const MASCARA: &str = "••••";

/// Campo que guarda senha (form, JSON, multipart, query): `pass`, `encpass`, `password`, `senha`,
/// `pwd`, `secret`.
fn campo_de_senha(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    ["pass", "senha", "pwd", "secret"].iter().any(|x| k.contains(x))
}

fn mascarar_json(v: &mut serde_json::Value) -> bool {
    use serde_json::Value;
    match v {
        Value::Object(m) => {
            let mut mudou = false;
            for (k, x) in m.iter_mut() {
                if campo_de_senha(k) && !x.is_null() {
                    *x = Value::String(MASCARA.into());
                    mudou = true;
                } else {
                    mudou |= mascarar_json(x);
                }
            }
            mudou
        }
        Value::Array(a) => a.iter_mut().fold(false, |m, x| mascarar_json(x) | m),
        _ => false,
    }
}

/// Corpo enviado com as senhas trocadas por `MASCARA`: a senha que o navegador preencheu não
/// aparece no `nd net --corpo` (nem é refeita pelo `nd api`).
pub fn mascarar_corpo(corpo: &str) -> String {
    let t = corpo.trim_start();
    if (t.starts_with('{') || t.starts_with('['))
        && let Ok(mut v) = serde_json::from_str::<serde_json::Value>(corpo)
    {
        return if mascarar_json(&mut v) {
            v.to_string()
        } else {
            corpo.to_string()
        };
    }
    if corpo.contains("form-data; name=") {
        return mascarar_multipart(corpo);
    }
    if !corpo.contains('=') || corpo.contains('\n') {
        return corpo.to_string();
    }
    let mut pares = super::net::ler_form(corpo);
    let mut mudou = false;
    for (k, v) in pares.iter_mut() {
        if campo_de_senha(k) && !v.is_empty() {
            *v = MASCARA.into();
            mudou = true;
        } else if let Ok(mut j) = serde_json::from_str::<serde_json::Value>(v)
            && mascarar_json(&mut j)
        {
            *v = j.to_string();
            mudou = true;
        }
    }
    if mudou {
        super::net::escrever_form(&pares)
    } else {
        corpo.to_string()
    }
}

fn mascarar_multipart(corpo: &str) -> String {
    let mut out = String::with_capacity(corpo.len());
    let mut resto = corpo;
    while let Some(i) = resto.find("form-data; name=\"") {
        let ini_nome = i + "form-data; name=\"".len();
        let Some(fim_nome) = resto[ini_nome..].find('"') else {
            break;
        };
        let nome = &resto[ini_nome..ini_nome + fim_nome];
        let Some(ini_valor) = resto[ini_nome..].find("\r\n\r\n").map(|x| ini_nome + x + 4) else {
            break;
        };
        let fim_valor = resto[ini_valor..]
            .find("\r\n--")
            .map_or(resto.len(), |x| ini_valor + x);
        out.push_str(&resto[..ini_valor]);
        out.push_str(if campo_de_senha(nome) {
            MASCARA
        } else {
            &resto[ini_valor..fim_valor]
        });
        resto = &resto[fim_valor..];
    }
    out.push_str(resto);
    out
}

/// URL com os parâmetros de senha da query trocados por `MASCARA`.
pub fn mascarar_url(url: &str) -> String {
    let Some((base, q)) = url.split_once('?') else {
        return url.to_string();
    };
    let (q, frag) = q.split_once('#').map_or((q, None), |(a, b)| (a, Some(b)));
    if !q
        .split('&')
        .any(|p| campo_de_senha(p.split('=').next().unwrap_or("")))
    {
        return url.to_string();
    }
    let partes: Vec<String> = q
        .split('&')
        .map(|p| match p.split_once('=') {
            Some((k, _)) if campo_de_senha(k) => format!("{k}={MASCARA}"),
            _ => p.to_string(),
        })
        .collect();
    let mut u = format!("{base}?{}", partes.join("&"));
    if let Some(f) = frag {
        u.push('#');
        u.push_str(f);
    }
    u
}

#[cfg(test)]
mod testes {
    use super::{cortar_bytes, mascarar_corpo, mascarar_url, ruido, site_de};

    #[test]
    fn senhas_mascaradas() {
        assert_eq!(
            mascarar_corpo("email=a%40b.com&pass=segredo&lsd=x"),
            "email=a%40b.com&pass=%E2%80%A2%E2%80%A2%E2%80%A2%E2%80%A2&lsd=x"
        );
        assert_eq!(
            mascarar_corpo(r#"{"user":{"login":"a","password":"segredo"},"n":1}"#),
            r#"{"user":{"login":"a","password":"••••"},"n":1}"#
        );
        assert_eq!(mascarar_corpo("a=1&b=2"), "a=1&b=2");
        assert_eq!(mascarar_corpo("texto qualquer"), "texto qualquer");
        let mp = "--x\r\nContent-Disposition: form-data; name=\"senha\"\r\n\r\nsegredo\r\n--x\r\nContent-Disposition: form-data; name=\"u\"\r\n\r\nana\r\n--x--";
        let m = mascarar_corpo(mp);
        assert!(
            !m.contains("segredo") && m.contains("ana") && m.contains("••••"),
            "{m}"
        );
        assert_eq!(
            mascarar_url("https://a.com/l?u=a&pwd=x#f"),
            "https://a.com/l?u=a&pwd=••••#f"
        );
        assert_eq!(mascarar_url("https://a.com/l?q=1"), "https://a.com/l?q=1");
    }

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

    #[test]
    fn corte_sem_partir_caractere() {
        assert_eq!(cortar_bytes("ação", 2), ("a".to_string(), true));
        assert_eq!(cortar_bytes("abc", 5), ("abc".to_string(), false));
    }
}
