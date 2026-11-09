//! `nd step`: o Decider (pplx-decider, modelo de decisão da Perplexity, via OpenRouter) dirige o
//! navegador. Não é uma LLM: a cada passo recebe o objetivo, os dados, o histórico, a saída da
//! última ação e o mapa da página, e responde três perguntas de múltipla escolha (ação, alvo e
//! valor) com probabilidades. O `nd` executa o comando escolhido e repete até o Decider dizer
//! `concluir`. Não escreve texto livre: só digita o que veio em `-d chave=valor` ou um número
//! visível na página.
//!
//! Quando não dá para seguir sozinho (passo sensível, ação repetida, confiança baixa, limite de
//! passos), o `nd step` para com código 2 e imprime o mapa atual para o LLM que chamou assumir.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::captcha;
use crate::cliente::{Pedido, enviar};
use crate::estado::{self, Estado};

const URL_PADRAO: &str = "https://openrouter.ai/api/alpha/decisions";
const MODELO_PADRAO: &str = "perplexity/pplx-decider-v1.2";
const LONGO: Duration = Duration::from_secs(200);

/// Palavras que marcam um clique irreversível (enviar, pagar, apagar...). Sem `--confirmado`,
/// o `nd step` para antes de clicar neles.
const SENSIVEIS: &[&str] = &[
    "enviar", "envie", "send", "submit", "pagar", "pague", "pay", "comprar", "buy", "purchase", "apagar",
    "excluir", "delete", "publicar", "publish", "finalizar", "place order", "confirmar", "confirm",
    "autorizar", "authorize",
];

const ACOES: &[(&str, &str)] = &[
    ("clicar", "clicar no elemento alvo"),
    ("preencher", "digitar o valor no campo alvo"),
    ("escolher", "escolher a opção (valor) num select ou lista do elemento alvo"),
    ("marcar", "marcar o checkbox, radio ou switch alvo"),
    ("desmarcar", "desmarcar o checkbox ou switch alvo"),
    ("rolar", "rolar a página para baixo"),
    ("concluir", "a meta já foi cumprida; não há mais nada a fazer"),
];

const CONFERENCIA: &str = "Antes de escolher a próxima ação, confira a saida_da_ultima_acao contra o objetivo: os nomes, \
números e identificadores que o objetivo cita (por exemplo o número do ticket aberto, o nome completo do destinatário) \
coincidem com o que a saída mostra? Se algo contradiz o objetivo, a próxima ação deve corrigir isso (voltar, escolher \
outro item) em vez de seguir em frente.";

pub struct Opcoes {
    pub objetivo: String,
    pub dados: Vec<(String, String)>,
    pub max: usize,
    /// Abaixo disto o `nd step` devolve o controle (0 = nunca).
    pub confianca: f64,
    /// Deixa clicar em botões sensíveis (enviar, pagar, apagar...).
    pub confirmado: bool,
}

pub enum Fim {
    Concluido,
    Devolvido(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decisao {
    pub acao: String,
    pub alvo: String,
    pub valor: Option<String>,
    pub confianca: f64,
}

#[derive(Default)]
pub struct Uso {
    pub chamadas: u32,
    pub entrada: u64,
    pub saida: u64,
    pub custo: f64,
}

impl Uso {
    pub fn somar(&mut self, o: &Uso) {
        self.chamadas += o.chamadas;
        self.entrada += o.entrada;
        self.saida += o.saida;
        self.custo += o.custo;
    }
}

pub struct Cfg {
    chave: String,
    url: String,
    pub modelo: String,
}

pub fn config() -> Result<Cfg, String> {
    let chave = std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .or_else(|| chave_do_arquivo(&estado::dir().join(".env")))
        .ok_or("o Decider precisa de OPENROUTER_API_KEY (variável de ambiente ou linha em ~/.nd/.env)")?;
    Ok(Cfg {
        chave: chave.trim().to_string(),
        url: std::env::var("ND_DECIDER_URL").unwrap_or_else(|_| URL_PADRAO.into()),
        modelo: std::env::var("ND_DECIDER_MODEL").unwrap_or_else(|_| MODELO_PADRAO.into()),
    })
}

fn chave_do_arquivo(p: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(p).ok()?.lines().find_map(|l| {
        let v = l.trim().strip_prefix("OPENROUTER_API_KEY=")?.trim();
        (!v.is_empty()).then(|| v.to_string())
    })
}

/// Elementos do mapa: (ref, descrição) de cada linha `eN tipo "nome"`.
pub fn refs_do_mapa(mapa: &str) -> Vec<(String, String)> {
    mapa.lines()
        .filter_map(|l| {
            let t = l.trim_start();
            let (r, resto) = t.split_once(' ')?;
            let n = r.strip_prefix('e')?;
            if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Some((r.to_string(), resto.chars().take(140).collect()))
        })
        .collect()
}

/// Números de 4 a 8 dígitos no texto (o Decider pode digitar um deles, ex.: um token mostrado na página).
pub fn numeros_visiveis(texto: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut atual = String::new();
    for c in texto.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_digit() {
            atual.push(c);
        } else {
            if (4..=8).contains(&atual.len()) && !out.contains(&atual) {
                out.push(atual.clone());
            }
            atual.clear();
        }
    }
    out
}

pub fn corta(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn corpo(
    cfg: &Cfg,
    o: &Opcoes,
    hist: &[String],
    ultima: &str,
    mapa: &str,
    refs: &[(String, String)],
) -> Value {
    let mut dados = serde_json::Map::new();
    for (k, v) in &o.dados {
        dados.insert(k.clone(), json!(v));
    }
    let mut state = json!({
        "objetivo": o.objetivo,
        "dados": dados,
        "historico": hist.iter().rev().take(12).rev().collect::<Vec<_>>(),
        "saida_da_ultima_acao": ultima,
        "mapa_da_pagina": mapa,
    });
    let mut instr_acao = "Qual é a próxima ação única para avançar o objetivo, dado o histórico e o mapa? \
                          Não repita uma ação que já deu certo."
        .to_string();
    if !ultima.is_empty() {
        state["conferencia"] = json!(CONFERENCIA);
        instr_acao.push_str(" Siga a 'conferencia' do state.");
    }
    let acoes: serde_json::Map<String, Value> = ACOES.iter().map(|(k, d)| (k.to_string(), json!(d))).collect();
    let mut alvos: serde_json::Map<String, Value> = refs.iter().map(|(r, d)| (r.clone(), json!(d))).collect();
    if alvos.is_empty() {
        alvos.insert("e1".into(), json!("nenhum"));
    }
    let mut valores = serde_json::Map::new();
    for (k, v) in &o.dados {
        valores.insert(k.clone(), json!(format!("{k} = {v}")));
    }
    for n in numeros_visiveis(&format!("{mapa} {ultima}")) {
        valores.insert(format!("num:{n}"), json!(format!("número visível na página: {n}")));
    }
    valores.insert("nenhum".into(), json!("nenhum texto (ação sem valor)"));
    json!({
        "model": cfg.modelo,
        "state": state,
        "questions": {
            "acao": { "type": "choice", "instructions": instr_acao, "criteria": acoes },
            "alvo": { "type": "choice", "instructions": "Qual elemento do mapa recebe a próxima ação? (Ignore se a ação for rolar ou concluir.)", "criteria": alvos },
            "valor": { "type": "choice", "instructions": "Qual dado deve ser digitado ou escolhido na próxima ação? Use 'nenhum' se a ação não leva texto.", "criteria": valores },
        },
    })
}

/// Lê as respostas do Decider: a escolha de cada pergunta, a menor confiança entre ação e alvo e o uso.
pub fn ler_resposta(j: &Value, dados: &[(String, String)]) -> Result<Decisao, String> {
    let a = j.get("answers").ok_or_else(|| format!("resposta do Decider sem 'answers': {}", corta(&j.to_string(), 300)))?;
    let escolha = |k: &str| a[k]["choice"].as_str().map(str::to_string);
    let acao = escolha("acao").ok_or("resposta sem a pergunta 'acao'")?;
    let alvo = escolha("alvo").ok_or("resposta sem a pergunta 'alvo'")?;
    let conf = |k: &str| a[k]["confidence"].as_f64().unwrap_or(1.0);
    let valor = escolha("valor").filter(|v| v != "nenhum").map(|v| match v.strip_prefix("num:") {
        Some(n) => n.to_string(),
        None => dados.iter().find(|(k, _)| *k == v).map(|(_, x)| x.clone()).unwrap_or_default(),
    });
    Ok(Decisao { acao, alvo, valor, confianca: conf("acao").min(conf("alvo")) })
}

pub fn chamar(cfg: &Cfg, body: &Value, uso: &mut Uso) -> Result<Value, String> {
    let mut resp = ureq::post(&cfg.url)
        .header("Authorization", format!("Bearer {}", cfg.chave))
        .config()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .send_json(body)
        .map_err(|e| format!("Decider inacessível: {e}"))?;
    let status = resp.status().as_u16();
    let j: Value = resp.body_mut().read_json().map_err(|e| format!("resposta inválida do Decider: {e}"))?;
    if status >= 300 {
        return Err(format!("Decider respondeu HTTP {status}: {}", corta(&j.to_string(), 300)));
    }
    uso.chamadas += 1;
    uso.entrada += j["usage"]["input_tokens"].as_u64().unwrap_or(0);
    uso.saida += j["usage"]["output_tokens"].as_u64().unwrap_or(0);
    uso.custo += j["usage"]["cost"].as_f64().unwrap_or(0.0);
    Ok(j)
}

/// Comando do daemon para a decisão (ou `None` para `concluir`).
pub fn comando_da(d: &Decisao) -> Result<Option<(&'static str, Value)>, String> {
    let alvo = d.alvo.clone();
    let precisa_valor = |v: &Option<String>| v.clone().filter(|x| !x.is_empty()).ok_or("ação sem valor");
    Ok(match d.acao.as_str() {
        "clicar" => Some(("click", json!({ "alvo": alvo, "js": false, "sem_diff": false }))),
        "preencher" => Some(("fill", json!({ "alvo": alvo, "valor": precisa_valor(&d.valor)?, "teclas": false, "sem_diff": false }))),
        "escolher" => Some(("pick", json!({ "alvo": alvo, "opcao": precisa_valor(&d.valor)?, "sem_diff": false }))),
        "marcar" => Some(("check", json!({ "alvo": alvo, "desmarcar": false, "sem_diff": false }))),
        "desmarcar" => Some(("check", json!({ "alvo": alvo, "desmarcar": true, "sem_diff": false }))),
        "rolar" => Some(("scroll", json!({ "alvo": null, "subir": false, "vezes": 1, "fim": false, "sem_diff": false }))),
        "concluir" => None,
        outra => return Err(format!("ação desconhecida {outra:?}")),
    })
}

/// Clique ou marcação num elemento cujo nome indica envio, pagamento ou exclusão.
pub fn sensivel(d: &Decisao, refs: &[(String, String)]) -> Option<String> {
    if !matches!(d.acao.as_str(), "clicar" | "marcar") {
        return None;
    }
    let desc = refs.iter().find(|(r, _)| *r == d.alvo).map(|(_, x)| x.as_str())?;
    let baixo = desc.to_lowercase();
    SENSIVEIS.iter().any(|p| baixo.contains(p)).then(|| format!("{} {}", d.alvo, desc))
}

pub fn pedir(est: &Estado, cmd: &str, args: Value) -> Result<(bool, String), String> {
    let r = enviar(est, &Pedido::novo(cmd, args), LONGO)?;
    Ok(if r.ok { (true, r.saida) } else { (false, r.erro) })
}

fn resumo(uso: &Uso, inicio: Instant) -> String {
    format!(
        "decider: {} chamadas, {} tokens de entrada, {} de saída, US$ {:.4}, {:.1} s",
        uso.chamadas,
        uso.entrada,
        uso.saida,
        uso.custo,
        inicio.elapsed().as_secs_f64()
    )
}

pub fn rodar(o: Opcoes) -> Result<Fim, String> {
    let cfg = config()?;
    let est = estado::ler().ok_or("o daemon não está rodando; use nd start")?;
    println!("decider: {} decide cada passo (não é LLM); objetivo: {}", cfg.modelo, o.objetivo);
    let inicio = Instant::now();
    let mut uso = Uso::default();
    let mut hist: Vec<String> = Vec::new();
    let mut recentes: Vec<String> = Vec::new();
    let mut ultima = String::new();
    let (mut captchas, mut pular_captcha, mut sem_grade) = (0u32, false, 0u32);
    let devolve = |motivo: String, est: &Estado, uso: &Uso| -> Result<Fim, String> {
        println!("{}", resumo(uso, inicio));
        println!("--- mapa atual ---");
        if let Ok((_, mapa)) = pedir(est, "map", json!({ "tudo": false })) {
            println!("{mapa}");
        }
        Ok(Fim::Devolvido(motivo))
    };

    for passo in 1..=o.max {
        let (_, mapa) = pedir(&est, "map", json!({ "tudo": false }))?;
        // Captcha de grade: o Decider resolve sozinho, sem o chamador pedir. Depois de marcar as
        // peças o próximo passo é dele (clicar em Verificar); só então tenta outra rodada.
        if captcha::indicio(&mapa) && captchas < captcha::RODADAS_MAX && !std::mem::take(&mut pular_captcha) {
            match captcha::resolver_pagina(&est, &captcha::Params::default(), Some(&mapa), true) {
                Ok(Some((r, _))) => {
                    captchas += 1;
                    pular_captcha = true;
                    uso.somar(&r.uso);
                    let n = r.marcadas().len();
                    println!("{passo}. captcha: {n} peças marcadas para {:?}\n{}", r.tarefa.curto(), r.desenho());
                    println!("   {}", r.tempos.texto());
                    hist.push(format!("captcha de {:?} resolvido: {n} peças marcadas; falta clicar em verificar", r.tarefa.curto()));
                    ultima = format!("captcha resolvido: {n} peças marcadas; falta confirmar com o botão de verificar");
                    recentes.clear();
                    continue;
                }
                Ok(None) => {
                    // Desafio aberto que não é grade de fotos (clicar numa forma, arrastar): o
                    // Decider olha a imagem numerada e escolhe as células; se não der, quem chamou assume.
                    if let Some(i) = captcha::instrucao_do_mapa(&mapa) {
                        sem_grade += 1;
                        let jogou = captcha::capturar(&est).and_then(|cap| {
                            let mut u = Uso::default();
                            let r = captcha::decidir_celulas(&cap.img, &i, captcha::CELULA, &mut u);
                            uso.somar(&u);
                            let (jogada, texto) = r?;
                            println!("{passo}. captcha visual: {i:?} → {texto}");
                            captcha::jogar(&est, captcha::CELULA, &jogada)?;
                            Ok(texto)
                        });
                        match jogou {
                            Ok(texto) if sem_grade <= captcha::TENTATIVAS_VISUAIS => {
                                captchas += 1;
                                pular_captcha = true;
                                hist.push(format!("captcha visual {i:?}: {texto}; falta confirmar com o botão"));
                                ultima = format!("captcha visual: {texto}; falta confirmar com o botão de próximo ou verificar");
                                recentes.clear();
                                continue;
                            }
                            erro => {
                                let motivo = erro.err().unwrap_or_else(|| "muitas tentativas".into());
                                let numerada = captcha::gravar_numerada(&est, captcha::CELULA, &estado::dir().join("captcha-grade.png"))
                                    .map(|d| format!("; imagem numerada em {d}; olhe-a e use nd captcha --clicar N,N ou --arrastar A,B"))
                                    .unwrap_or_default();
                                return devolve(format!("captcha visual não resolvido ({i:?}): {motivo}{numerada}"), &est, &uso);
                            }
                        }
                    }
                }
                Err(e) => println!("{passo}. captcha: não resolvido ({e})"),
            }
        }
        let refs = refs_do_mapa(&mapa);
        let body = corpo(&cfg, &o, &hist, &ultima, &mapa, &refs);
        let d = ler_resposta(&chamar(&cfg, &body, &mut uso)?, &o.dados)?;
        let rotulo = format!(
            "{passo}. {} {}{} [{:.2}]",
            d.acao,
            d.alvo,
            d.valor.as_deref().map(|v| format!(" {v:?}")).unwrap_or_default(),
            d.confianca
        );
        let Some((cmd, args)) = (match comando_da(&d) {
            Ok(c) => c,
            Err(e) => {
                println!("{rotulo} → erro: {e}");
                hist.push(format!("{} {} -> ERRO: {e}", d.acao, d.alvo));
                ultima = format!("erro: {e}");
                continue;
            }
        }) else {
            println!("{rotulo} → concluído");
            println!("{}", resumo(&uso, inicio));
            return Ok(Fim::Concluido);
        };
        if o.confianca > 0.0 && d.confianca < o.confianca {
            println!("{rotulo} → não executado");
            return devolve(format!("confiança {:.2} abaixo de {:.2}", d.confianca, o.confianca), &est, &uso);
        }
        if !o.confirmado {
            if let Some(alvo) = sensivel(&d, &refs) {
                println!("{rotulo} → não executado");
                return devolve(
                    format!("passo sensível ({alvo}); rode de novo com --confirmado se o usuário já autorizou"),
                    &est,
                    &uso,
                );
            }
        }
        let chave = format!("{} {} {:?}", d.acao, d.alvo, d.valor);
        recentes.push(chave);
        if recentes.len() >= 3 && recentes[recentes.len() - 3..].iter().all(|x| *x == recentes[recentes.len() - 1]) {
            println!("{rotulo} → não executado");
            return devolve("repetiu a mesma ação 3 vezes".into(), &est, &uso);
        }
        let (ok, saida) = pedir(&est, cmd, args)?;
        let primeira = saida.lines().next().unwrap_or("").to_string();
        println!("{rotulo} → {}{}", if ok { "" } else { "ERRO: " }, corta(&primeira, 120));
        hist.push(format!("{} {} {} -> {}: {}", d.acao, d.alvo, d.valor.clone().unwrap_or_default(), if ok { "ok" } else { "ERRO" }, corta(&primeira, 200)));
        ultima = corta(&if ok { saida } else { format!("erro: {saida}") }, 800);
    }
    devolve(format!("limite de {} passos", o.max), &est, &uso)
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn refs_e_numeros() {
        let mapa = "# Título\nh1 Cadastro\ne2 input[text] \"Nome\"\n  e10 button \"Salvar\"\nstatus e3 nada";
        let r = refs_do_mapa(mapa);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], ("e2".into(), "input[text] \"Nome\"".into()));
        assert_eq!(r[1].0, "e10");
        assert_eq!(numeros_visiveis("token 198567 porta 8765, ano 2026 e 12 e 123456789"), ["198567", "8765", "2026"]);
    }

    #[test]
    fn acao_para_comando() {
        let d = |a: &str, v: Option<&str>| Decisao { acao: a.into(), alvo: "e4".into(), valor: v.map(Into::into), confianca: 1.0 };
        let (cmd, args) = comando_da(&d("preencher", Some("x"))).unwrap().unwrap();
        assert_eq!((cmd, args["valor"].as_str()), ("fill", Some("x")));
        assert!(comando_da(&d("preencher", None)).is_err());
        assert!(comando_da(&d("concluir", None)).unwrap().is_none());
        assert_eq!(comando_da(&d("desmarcar", None)).unwrap().unwrap().1["desmarcar"], json!(true));
    }

    #[test]
    fn passo_sensivel() {
        let refs = vec![("e1".into(), "button \"Enviar resposta\"".into()), ("e2".into(), "button \"Avançar\"".into())];
        let d = |a: &str, alvo: &str| Decisao { acao: a.into(), alvo: alvo.into(), valor: None, confianca: 1.0 };
        assert!(sensivel(&d("clicar", "e1"), &refs).is_some());
        assert!(sensivel(&d("clicar", "e2"), &refs).is_none());
        assert!(sensivel(&d("preencher", "e1"), &refs).is_none());
    }

    #[test]
    fn le_respostas() {
        let j = json!({"answers": {
            "acao": {"choice": "preencher", "confidence": 0.9},
            "alvo": {"choice": "e5", "confidence": 0.7},
            "valor": {"choice": "nome"}}});
        let d = ler_resposta(&j, &[("nome".into(), "Arthur".into())]).unwrap();
        assert_eq!((d.acao.as_str(), d.alvo.as_str(), d.valor.as_deref()), ("preencher", "e5", Some("Arthur")));
        assert!((d.confianca - 0.7).abs() < 1e-9);
        let j = json!({"answers": {"acao": {"choice": "clicar"}, "alvo": {"choice": "e1"}, "valor": {"choice": "num:4821"}}});
        assert_eq!(ler_resposta(&j, &[]).unwrap().valor.as_deref(), Some("4821"));
        assert!(ler_resposta(&json!({"error": "x"}), &[]).is_err());
    }
}
