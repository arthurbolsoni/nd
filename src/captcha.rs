//! Captcha de grade ("selecione todos os quadrados com semáforos") resolvido pelo Decider.
//!
//! A imagem é dividida em peças e cada peça vai ao Decider com a pergunta "contém o alvo?"
//! (sim/não com probabilidade). A grade é achada pelas linhas brancas entre as peças. A instrução vem,
//! em ordem, de `--alvo`, da frase que a página mostra (hCaptcha) ou do cabeçalho recortado da imagem,
//! que segue junto de cada peça (reCAPTCHA); não há lista de objetos.
//!
//! Dentro do `nd step` roda sozinho: quando o mapa da página indica um captcha, o `nd` captura a
//! janela, resolve e clica nas peças, sem o chamador pedir nada.

use std::io::Cursor;
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::{DynamicImage, ImageFormat, RgbImage, imageops};
use serde_json::{Value, json};

use crate::decisor::{Cfg, Uso, chamar, config, corta, pedir};
use crate::estado::Estado;

/// Elemento do captcha na página: o quadro do desafio do reCAPTCHA ou do hCaptcha.
pub const SELETOR: &str = "iframe[src*=\"recaptcha\"][src*=\"bframe\"], iframe[src*=\"hcaptcha\"][src*=\"challenge\"], \
                           iframe[title*=\"challenge\" i]";

/// Pausa entre cliques nas peças.
const PAUSA_CLIQUE: Duration = Duration::from_millis(100);
/// Tamanho mínimo, em pixels, de uma faixa de conteúdo (descarta bordas e linhas de 1 px).
const FAIXA_MIN: usize = 12;
/// Tentativas do Decider em desafios visuais por execução (cada desafio do hCaptcha tem 2 páginas).
pub const TENTATIVAS_VISUAIS: u32 = 4;
/// Rodadas de captcha resolvidas automaticamente por execução do `nd step`.
pub const RODADAS_MAX: u32 = 6;

type Ret = (u32, u32, u32, u32);

/// Grade de peças dentro da imagem: retângulo `(x, y, largura, altura)` de cada uma, linha a linha.
#[derive(Debug, PartialEq)]
pub struct Grade {
    pub linhas: usize,
    pub colunas: usize,
    pub pecas: Vec<Ret>,
    /// Faixa acima da grade com a instrução (e a imagem de exemplo), quando existe.
    pub cabecalho: Option<Ret>,
}

#[derive(Debug, Clone)]
pub struct Peca {
    pub linha: usize,
    pub coluna: usize,
    pub sim: bool,
    pub p_sim: f64,
    pub confianca: f64,
}

/// Tempo de cada parte (captura e cliques só existem quando a origem é a página).
#[derive(Default, Clone, Copy)]
pub struct Tempos {
    pub captura: Duration,
    pub pecas: Duration,
    pub cliques: Duration,
}

impl Tempos {
    pub fn texto(&self) -> String {
        let s = |d: Duration| format!("{:.2}", d.as_secs_f64());
        format!(
            "tempos (s): captura {}, peças {}, cliques {}",
            s(self.captura),
            s(self.pecas),
            s(self.cliques)
        )
    }
}

pub struct Resultado {
    pub tempos: Tempos,
    pub tarefa: Tarefa,
    pub grade: Grade,
    pub pecas: Vec<Peca>,
    pub uso: Uso,
}

impl Resultado {
    pub fn marcadas(&self) -> Vec<&Peca> {
        self.pecas.iter().filter(|p| p.sim).collect()
    }

    /// Grade marcada (`X` contém o alvo, `.` não), uma linha por linha da grade.
    pub fn desenho(&self) -> String {
        (0..self.grade.linhas)
            .map(|l| {
                (0..self.grade.colunas)
                    .map(|c| if self.pecas[l * self.grade.colunas + c].sim { "X" } else { "." })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Default)]
pub struct Params {
    pub alvo: Option<String>,
    /// Frase de instrução do desafio ("Select all drinks that can be hot or cold"), lida do mapa.
    pub instrucao: Option<String>,
    pub linhas: Option<usize>,
    pub colunas: Option<usize>,
    /// `(x, y, largura, altura)` da grade dentro da imagem.
    pub recorte: Option<Ret>,
}

// ---------------------------------------------------------------- grade

fn branco(p: &image::Rgb<u8>) -> bool {
    p.0.iter().all(|&c| c >= 235)
}

/// Faixas `[ini, fim)` onde mais da metade dos valores tem conteúdo (não é branco).
fn faixas(fracoes: impl Iterator<Item = f64>) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut ini = None;
    let mut n = 0;
    for (i, f) in fracoes.enumerate() {
        n = i + 1;
        match (f > 0.5, ini) {
            (true, None) => ini = Some(i),
            (false, Some(a)) => {
                if i - a >= FAIXA_MIN {
                    out.push((a, i));
                }
                ini = None;
            }
            _ => {}
        }
    }
    if let Some(a) = ini
        && n - a >= FAIXA_MIN
    {
        out.push((a, n));
    }
    out
}

fn colunas_da_faixa(img: &RgbImage, y0: usize, y1: usize) -> Vec<(usize, usize)> {
    faixas((0..img.width() as usize).map(|x| {
        let n = (y0..y1).filter(|&y| !branco(img.get_pixel(x as u32, y as u32))).count();
        n as f64 / (y1 - y0) as f64
    }))
}

/// Acha a grade pelas linhas brancas: cada linha de peças é uma faixa de conteúdo com 2 ou mais
/// colunas; o cabeçalho é a faixa de coluna única logo acima. `None` quando não há grade com
/// separadores (foto única sem linhas; aí a grade vem de `--linhas/--colunas`).
pub fn detectar(img: &RgbImage) -> Option<Grade> {
    // Captura da janela inteira: a grade só ocupa um canto. Corta na caixa do que não é branco.
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if !branco(p) {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    if (x0, y0, x1, y1) != (0, 0, img.width(), img.height()) {
        let corte = imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image();
        let mut g = detectar_na_caixa(&corte)?;
        for r in g.pecas.iter_mut().chain(g.cabecalho.as_mut()) {
            (r.0, r.1) = (r.0 + x0, r.1 + y0);
        }
        return Some(g);
    }
    detectar_na_caixa(img)
}

fn detectar_na_caixa(img: &RgbImage) -> Option<Grade> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let linhas = faixas((0..h).map(|y| {
        let n = (0..w).filter(|&x| !branco(img.get_pixel(x as u32, y as u32))).count();
        n as f64 / w as f64
    }));
    let cols: Vec<_> = linhas.iter().map(|&(a, b)| colunas_da_faixa(img, a, b)).collect();
    // Maior sequência de faixas com 2 ou mais colunas e a mesma altura (o cabeçalho é mais baixo
    // ou mais alto que uma linha de peças, mesmo quando uma linha branca o parte em duas colunas).
    let altura = |l: usize| linhas[l].1 - linhas[l].0;
    let mut melhor: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < linhas.len() {
        if cols[i].len() < 2 {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < linhas.len() && cols[j].len() >= 2 && altura(j).abs_diff(altura(j - 1)) <= 4 {
            j += 1;
        }
        if j - i >= 2 && melhor.is_none_or(|(a, b)| j - i > b - a) {
            melhor = Some((i, j));
        }
        i = j;
    }
    let (a, b) = melhor?;
    // Colunas pelo perfil de todas as linhas juntas: a linha branca vertical some em uma linha de
    // peças com céu claro, mas no conjunto continua sendo a coluna mais branca.
    let total: usize = (a..b).map(|l| linhas[l].1 - linhas[l].0).sum();
    let colunas = faixas((0..w).map(|x| {
        let n: usize = (a..b)
            .map(|l| (linhas[l].0..linhas[l].1).filter(|&y| !branco(img.get_pixel(x as u32, y as u32))).count())
            .sum();
        n as f64 / total as f64
    }));
    if colunas.len() < 2 {
        return None;
    }
    let pecas = (a..b)
        .flat_map(|l| {
            let (y0, y1) = linhas[l];
            colunas.iter().map(move |&(x0, x1)| (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
        })
        .collect();
    let cabecalho = a
        .checked_sub(1)
        .map(|k| (0, linhas[k].0 as u32, w as u32, (linhas[k].1 - linhas[k].0) as u32));
    Some(Grade { linhas: b - a, colunas: colunas.len(), pecas, cabecalho })
}

/// Grade por divisão igual do retângulo (padrão: a imagem toda).
pub fn dividir(img: &RgbImage, linhas: usize, colunas: usize, recorte: Option<Ret>) -> Grade {
    let (x0, y0, w, h) = recorte.unwrap_or((0, 0, img.width(), img.height()));
    let (lin, col) = (linhas as u32, colunas as u32);
    let pecas = (0..lin)
        .flat_map(|r| {
            (0..col).map(move |c| {
                let (x, xf) = (x0 + c * w / col, x0 + (c + 1) * w / col);
                let (y, yf) = (y0 + r * h / lin, y0 + (r + 1) * h / lin);
                (x, y, xf - x, yf - y)
            })
        })
        .collect();
    Grade { linhas, colunas, pecas, cabecalho: None }
}

fn grade_de(img: &RgbImage, p: &Params) -> Result<Grade, String> {
    if let (Some(l), Some(c)) = (p.linhas, p.colunas) {
        return Ok(dividir(img, l, c, p.recorte));
    }
    if p.linhas.is_some() || p.colunas.is_some() {
        return Err("informe --linhas e --colunas juntos".into());
    }
    detectar(img).ok_or_else(|| "não achei a grade pelas linhas brancas; informe --linhas e --colunas (e --recorte)".into())
}

// ---------------------------------------------------------------- Decider

fn recorte_png(img: &RgbImage, (x, y, w, h): Ret) -> Result<String, String> {
    let (x, y) = (x.min(img.width().saturating_sub(1)), y.min(img.height().saturating_sub(1)));
    let (w, h) = (w.min(img.width() - x).max(1), h.min(img.height() - y).max(1));
    let parte = imageops::crop_imm(img, x, y, w, h).to_image();
    let mut buf = Vec::new();
    DynamicImage::ImageRgb8(parte).write_to(&mut Cursor::new(&mut buf), ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(buf)))
}

/// O que se pergunta de cada peça: um objeto nomeado ou a frase de instrução do desafio.
#[derive(Debug, Clone, PartialEq)]
pub enum Tarefa {
    Objeto(String),
    Instrucao(String),
    /// A instrução só existe como imagem (o cabeçalho recortado, em data URL); vai junto de cada peça.
    Cabecalho(String),
}

impl Tarefa {
    pub fn rotulo(&self) -> String {
        match self {
            Tarefa::Objeto(o) => format!("alvo: {o}"),
            Tarefa::Instrucao(i) => format!("instrução: {i}"),
            Tarefa::Cabecalho(_) => "instrução: lida do cabeçalho da imagem".into(),
        }
    }

    /// Texto do alvo para o resumo de `nd step`.
    pub fn curto(&self) -> &str {
        match self {
            Tarefa::Objeto(t) | Tarefa::Instrucao(t) => t,
            Tarefa::Cabecalho(_) => "instrução do cabeçalho",
        }
    }
}

fn corpo_peca(cfg: &Cfg, url: &str, t: &Tarefa) -> Value {
    if let Tarefa::Cabecalho(cab) = t {
        return json!({
            "model": cfg.modelo,
            "state": [
                { "type": "text", "text": "Captcha de grade. A primeira imagem é o cabeçalho com a instrução do desafio; a segunda é uma peça da grade." },
                { "type": "image_url", "image_url": { "url": cab } },
                { "type": "image_url", "image_url": { "url": url } },
            ],
            "questions": { "r": {
                "type": "choice",
                "instructions": "A peça (segunda imagem) atende à instrução escrita no cabeçalho (primeira imagem), mesmo que só em parte?",
                "criteria": { "sim": "atende à instrução", "nao": "não atende à instrução" },
            } },
        });
    }
    let (contexto, pergunta, sim, nao) = match t {
        Tarefa::Objeto(a) => (
            format!("Peça de um captcha de grade. Alvo: {a}."),
            format!("Esta peça contém {a} (mesmo que só em parte)?"),
            format!("contém {a}"),
            format!("não contém {a}"),
        ),
        Tarefa::Cabecalho(_) => unreachable!(),
        Tarefa::Instrucao(i) => (
            format!("Peça de um captcha de grade. Instrução do desafio: \"{i}\"."),
            format!("Esta peça atende à instrução \"{i}\"?"),
            "atende à instrução".to_string(),
            "não atende à instrução".to_string(),
        ),
    };
    json!({
        "model": cfg.modelo,
        "state": [
            { "type": "text", "text": contexto },
            { "type": "image_url", "image_url": { "url": url } },
        ],
        "questions": { "r": {
            "type": "choice",
            "instructions": pergunta,
            "criteria": { "sim": sim, "nao": nao },
        } },
    })
}

/// `(sim, p_sim, confiança)` da resposta de uma peça.
pub fn ler_peca(j: &Value) -> Result<(bool, f64, f64), String> {
    let a = j.get("answers").and_then(|a| a.get("r")).ok_or_else(|| {
        format!("resposta do Decider sem 'answers.r': {}", corta(&j.to_string(), 300))
    })?;
    let sim = a["choice"].as_str().ok_or("resposta sem 'choice'")? == "sim";
    let p_sim = a["probabilities"]["sim"].as_f64().unwrap_or(if sim { 1.0 } else { 0.0 });
    Ok((sim, p_sim, a["confidence"].as_f64().unwrap_or(1.0)))
}

/// Divide, pergunta ao Decider por peça (todas ao mesmo tempo) e devolve a grade marcada.
pub fn resolver(img: &RgbImage, p: &Params) -> Result<Resultado, String> {
    let cfg = config()?;
    let grade = grade_de(img, p)?;
    let mut uso = Uso::default();
    let tarefa = match (&p.alvo, &p.instrucao) {
        (Some(a), _) => Tarefa::Objeto(a.clone()),
        (None, Some(i)) => Tarefa::Instrucao(i.clone()),
        (None, None) => {
            let area = grade.cabecalho.ok_or("sem cabeçalho nem instrução na página; informe --alvo")?;
            Tarefa::Cabecalho(recorte_png(img, area)?)
        }
    };
    let t1 = Instant::now();
    let urls: Vec<String> = grade.pecas.iter().map(|&r| recorte_png(img, r)).collect::<Result<_, _>>()?;
    let respostas: Vec<Result<((bool, f64, f64), Uso), String>> = thread::scope(|s| {
        let hs: Vec<_> = urls
            .iter()
            .map(|u| {
                let (cfg, tarefa) = (&cfg, &tarefa);
                s.spawn(move || {
                    let mut uso = Uso::default();
                    let j = chamar(cfg, &corpo_peca(cfg, u, tarefa), &mut uso)?;
                    Ok((ler_peca(&j)?, uso))
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("thread do Decider caiu".into()))).collect()
    });
    let mut pecas = Vec::new();
    for (i, r) in respostas.into_iter().enumerate() {
        let ((sim, p_sim, confianca), u) = r.map_err(|e| format!("peça {}: {e}", i + 1))?;
        uso.somar(&u);
        pecas.push(Peca { linha: i / grade.colunas, coluna: i % grade.colunas, sim, p_sim, confianca });
    }
    Ok(Resultado { tempos: Tempos { pecas: t1.elapsed(), ..Tempos::default() }, tarefa, grade, pecas, uso })
}

// ---------------------------------------------------------------- página

/// Captura da janela (ou do quadro do desafio): imagem e origem em pixels da janela.
pub struct Captura {
    pub img: RgbImage,
    pub x: f64,
    pub y: f64,
}

pub fn capturar(est: &Estado) -> Result<Captura, String> {
    let (ok, saida) = pedir(est, "shot", json!({ "sel": SELETOR }))?;
    if !ok {
        return Err(saida);
    }
    let j: Value = serde_json::from_str(&saida).map_err(|e| format!("captura inválida: {e}"))?;
    let png = STANDARD.decode(j["png"].as_str().ok_or("captura sem imagem")?).map_err(|e| e.to_string())?;
    // ND_CAPTCHA_SALVAR=arquivo.png grava a captura (depuração da detecção da grade).
    if let Some(f) = std::env::var_os("ND_CAPTCHA_SALVAR") {
        let _ = std::fs::write(f, &png);
    }
    let img = image::load_from_memory(&png).map_err(|e| e.to_string())?.to_rgb8();
    Ok(Captura { img, x: j["x"].as_f64().unwrap_or(0.0), y: j["y"].as_f64().unwrap_or(0.0) })
}

/// Clica no centro das peças marcadas.
pub fn clicar(est: &Estado, cap: &Captura, r: &Resultado) -> Result<(), String> {
    for p in r.marcadas() {
        let (x, y, w, h) = r.grade.pecas[p.linha * r.grade.colunas + p.coluna];
        let (cx, cy) = (cap.x + x as f64 + w as f64 / 2.0, cap.y + y as f64 + h as f64 / 2.0);
        let (ok, e) = pedir(est, "clicarxy", json!({ "x": cx, "y": cy }))?;
        if !ok {
            return Err(e);
        }
        thread::sleep(PAUSA_CLIQUE);
    }
    Ok(())
}

/// O mapa da página sugere um captcha de grade (vale só para decidir se vale capturar a tela).
pub fn indicio(mapa: &str) -> bool {
    let m = mapa.to_lowercase();
    ["captcha", "select all images", "select all squares", "selecione todas as imagens", "selecione todos os quadrados"]
        .iter()
        .any(|k| m.contains(k))
}

/// Frase de instrução do desafio no mapa da página ("Select all drinks that can be hot or cold").
/// O hCaptcha escreve o prompt em texto, num título ou num elemento clicável.
pub fn instrucao_do_mapa(mapa: &str) -> Option<String> {
    const INICIOS: &[&str] = &[
        "select all", "select every", "select each", "click on all", "click all", "click each", "choose all",
        "pick all", "tap all", "click the", "click on the", "select the", "drag", "selecione todas", "selecione todos", "clique em todas", "clique em todos",
    ];
    mapa.lines().find_map(|l| {
        let (_, resto) = l.trim_start().split_once(' ')?;
        let texto = match (resto.find('"'), resto.rfind('"')) {
            (Some(a), Some(b)) if b > a => &resto[a + 1..b],
            _ => resto,
        }
        .trim();
        let baixo = texto.to_lowercase();
        (texto.chars().count() <= 160 && INICIOS.iter().any(|i| baixo.starts_with(i))).then(|| texto.to_string())
    })
}

/// Captura, resolve e clica. `Ok(None)` = a página não mostra uma grade de captcha agora.
/// `mapa` = o mapa que o chamador já tem (o `nd step`); sem ele, lê da aba quando falta alvo.
pub fn resolver_pagina(
    est: &Estado,
    p: &Params,
    mapa: Option<&str>,
    clica: bool,
) -> Result<Option<(Resultado, Captura)>, String> {
    let t = Instant::now();
    let mut p = Params { alvo: p.alvo.clone(), instrucao: p.instrucao.clone(), ..*p };
    if p.alvo.is_none() && p.instrucao.is_none() {
        let mapa = match mapa {
            Some(m) => m.to_string(),
            None => pedir(est, "map", json!({ "tudo": false })).map(|(_, m)| m).unwrap_or_default(),
        };
        p.instrucao = instrucao_do_mapa(&mapa);
    }
    let p = &p;
    let cap = capturar(est)?;
    if p.linhas.is_none() && detectar(&cap.img).is_none() {
        return Ok(None);
    }
    let captura = t.elapsed();
    let mut r = resolver(&cap.img, p)?;
    r.tempos.captura = captura;
    if clica {
        let t = Instant::now();
        clicar(est, &cap, &r)?;
        r.tempos.cliques = t.elapsed();
    }
    Ok(Some((r, cap)))
}

// ---------------------------------------------------------------- grade numerada

/// Lado, em pixels, de cada célula da grade numerada.
pub const CELULA: u32 = 48;

/// Dígitos 3x5 (um bit por pixel, linha a linha) para rotular as células sem depender de fonte.
const DIGITOS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

fn colunas_da(largura: u32, celula: u32) -> u32 {
    largura.div_ceil(celula)
}

/// Centro, na janela, da célula `n` (1 = canto superior esquerdo, numeração por linha).
pub fn centro(cap: &Captura, celula: u32, n: u32) -> Result<(f64, f64), String> {
    let cols = colunas_da(cap.img.width(), celula);
    let linhas = cap.img.height().div_ceil(celula);
    if n == 0 || n > cols * linhas {
        return Err(format!("célula {n} fora da grade (1 a {})", cols * linhas));
    }
    let (r, c) = ((n - 1) / cols, (n - 1) % cols);
    let (x0, y0) = (c * celula, r * celula);
    let (w, h) = (celula.min(cap.img.width() - x0), celula.min(cap.img.height() - y0));
    Ok((cap.x + x0 as f64 + w as f64 / 2.0, cap.y + y0 as f64 + h as f64 / 2.0))
}

/// Cópia da captura com linhas de grade e o número de cada célula no canto.
pub fn numerar(img: &RgbImage, celula: u32) -> RgbImage {
    let mut o = img.clone();
    let (w, h) = (o.width(), o.height());
    let cols = colunas_da(w, celula);
    for y in 0..h {
        for x in 0..w {
            if x % celula == 0 || y % celula == 0 {
                let p = o.get_pixel_mut(x, y);
                *p = image::Rgb([p[0] / 2 + 100, p[1] / 2 + 100, p[2] / 2 + 100]);
            }
        }
    }
    let escala = 2u32;
    for r in 0..h.div_ceil(celula) {
        for c in 0..cols {
            let texto = (r * cols + c + 1).to_string();
            let (bx, by) = (c * celula + 2, r * celula + 2);
            let (bw, bh) = (texto.len() as u32 * 4 * escala + escala, 5 * escala + 2 * escala);
            for y in by..(by + bh).min(h) {
                for x in bx..(bx + bw).min(w) {
                    o.put_pixel(x, y, image::Rgb([255, 235, 59]));
                }
            }
            for (i, d) in texto.bytes().enumerate() {
                for (ly, linha) in DIGITOS[(d - b'0') as usize].iter().enumerate() {
                    for lx in 0..3u32 {
                        if linha >> (2 - lx) & 1 == 1 {
                            for dy in 0..escala {
                                for dx in 0..escala {
                                    let (x, y) = (
                                        bx + escala + (i as u32 * 4 + lx) * escala + dx,
                                        by + escala + ly as u32 * escala + dy,
                                    );
                                    if x < w && y < h {
                                        o.put_pixel(x, y, image::Rgb([0, 0, 0]));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    o
}

/// Captura o desafio, grava a imagem numerada em `destino` e devolve a descrição da grade.
pub fn gravar_numerada(est: &Estado, celula: u32, destino: &std::path::Path) -> Result<String, String> {
    let cap = capturar(est)?;
    numerar(&cap.img, celula).save(destino).map_err(|e| format!("{}: {e}", destino.display()))?;
    let (cols, linhas) = (colunas_da(cap.img.width(), celula), cap.img.height().div_ceil(celula));
    Ok(format!(
        "{} ({}x{} px, células de {celula} px numeradas de 1 a {}, {cols} por linha)",
        destino.display(),
        cap.img.width(),
        cap.img.height(),
        cols * linhas
    ))
}

/// Clica no centro das células `ns` do desafio atual.
pub fn clicar_celulas(est: &Estado, celula: u32, ns: &[u32]) -> Result<(), String> {
    let cap = capturar(est)?;
    for &n in ns {
        let (x, y) = centro(&cap, celula, n)?;
        let (ok, e) = pedir(est, "clicarxy", json!({ "x": x, "y": y }))?;
        if !ok {
            return Err(e);
        }
        thread::sleep(PAUSA_CLIQUE);
    }
    Ok(())
}

/// Arrasta do centro da célula `de` ao centro da célula `para`.
pub fn arrastar_celulas(est: &Estado, celula: u32, de: u32, para: u32) -> Result<(), String> {
    let cap = capturar(est)?;
    let (a, b) = (centro(&cap, celula, de)?, centro(&cap, celula, para)?);
    let (ok, e) = pedir(est, "arrastarxy", json!({ "de": a, "para": b }))?;
    if ok { Ok(()) } else { Err(e) }
}

// ---------------------------------------------------------------- desafios visuais pelo Decider

/// Retângulo do desafio dentro da captura: a faixa de conteúdo mais alta (o cabeçalho e o rodapé são
/// mais baixos), na largura que o conteúdo ocupa.
pub fn area_do_desafio(img: &RgbImage) -> Option<Ret> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let linhas = faixas((0..h).map(|y| {
        (0..w).filter(|&x| !branco(img.get_pixel(x as u32, y as u32))).count() as f64 / w as f64
    }));
    let &(y0, y1) = linhas.iter().max_by_key(|(a, b)| b - a)?;
    let cols = colunas_da_faixa(img, y0, y1);
    let (x0, x1) = (cols.first()?.0, cols.last()?.1);
    Some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Quantas peças a instrução manda escolher ("Click the THREE matching shapes" = 3).
pub fn quantidade(instrucao: &str) -> usize {
    let baixo = instrucao.to_lowercase();
    const NOMES: &[(&str, usize)] =
        &[("two", 2), ("three", 3), ("four", 4), ("five", 5), ("dois", 2), ("duas", 2), ("três", 3), ("quatro", 4)];
    baixo
        .split(|c: char| !c.is_alphanumeric())
        .find_map(|p| NOMES.iter().find(|(n, _)| *n == p).map(|(_, q)| *q).or_else(|| p.parse().ok().filter(|n| (2..=9).contains(n))))
        .unwrap_or(1)
}

/// As `k` células mais prováveis, sem duas coladas (a mesma forma ocupa várias células).
pub fn melhores_celulas(prob: &serde_json::Map<String, Value>, cols: u32, k: usize) -> Vec<u32> {
    let mut v: Vec<(u32, f64)> = prob.iter().filter_map(|(c, p)| Some((c.parse().ok()?, p.as_f64()?))).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    let pos = |n: u32| ((n - 1) / cols, (n - 1) % cols);
    let mut out: Vec<u32> = Vec::new();
    for (c, _) in v {
        let (r, q) = pos(c);
        if out.iter().all(|&o| {
            let (r2, q2) = pos(o);
            r.abs_diff(r2).max(q.abs_diff(q2)) >= 3
        }) {
            out.push(c);
            if out.len() == k {
                break;
            }
        }
    }
    out
}

fn criterios_celulas(img: &RgbImage, celula: u32, area: Ret) -> serde_json::Map<String, Value> {
    let cols = colunas_da(img.width(), celula);
    let (ax, ay, aw, ah) = area;
    let mut m = serde_json::Map::new();
    for r in 0..img.height().div_ceil(celula) {
        for c in 0..cols {
            let (cx, cy) = (c * celula + celula / 2, r * celula + celula / 2);
            if cx >= ax && cx < ax + aw && cy >= ay && cy < ay + ah {
                m.insert((r * cols + c + 1).to_string(), json!(format!("célula {}", r * cols + c + 1)));
            }
        }
    }
    m
}

pub enum Jogada {
    Clicar(Vec<u32>),
    Arrastar(u32, u32),
}

/// O Decider olha a imagem numerada e escolhe as células: clicar nas `k` melhores, ou, para
/// "Drag ...", a peça a arrastar e o lugar onde ela encaixa.
pub fn decidir_celulas(
    img: &RgbImage,
    instrucao: &str,
    celula: u32,
    uso: &mut Uso,
) -> Result<(Jogada, String), String> {
    let cfg = config()?;
    let area = area_do_desafio(img).ok_or("não achei a área do desafio na captura")?;
    let crit = criterios_celulas(img, celula, area);
    if crit.is_empty() {
        return Err("a área do desafio não cobre nenhuma célula".into());
    }
    let url = {
        let mut buf = Vec::new();
        DynamicImage::ImageRgb8(numerar(img, celula))
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        format!("data:image/png;base64,{}", STANDARD.encode(buf))
    };
    let arrastar = instrucao.to_lowercase().contains("drag");
    let perguntas = if arrastar {
        json!({
            "de": { "type": "choice", "criteria": crit,
                    "instructions": format!("Instrução: \"{instrucao}\". Qual célula numerada contém a peça que deve ser arrastada (a peça que encaixa no espaço vazio)?") },
            "para": { "type": "choice", "criteria": crit,
                      "instructions": format!("Instrução: \"{instrucao}\". Qual célula numerada contém o espaço vazio, ou o contorno, onde a peça deve ser solta?") },
        })
    } else {
        let k = quantidade(instrucao);
        json!({ "c": { "type": "choice", "criteria": crit,
            "instructions": format!("Instrução: \"{instrucao}\". Qual célula numerada contém {} que a instrução manda clicar?",
                if k == 1 { "o item".to_string() } else { format!("um dos {k} itens") }) } })
    };
    let body = json!({
        "model": cfg.modelo,
        "state": [
            { "type": "text", "text": format!("Desafio visual de captcha. Instrução: \"{instrucao}\". A imagem tem uma grade numerada: o número em amarelo fica no canto superior esquerdo de cada célula.") },
            { "type": "image_url", "image_url": { "url": url } },
        ],
        "questions": perguntas,
    });
    let j = chamar(&cfg, &body, uso)?;
    let a = j.get("answers").ok_or_else(|| format!("resposta sem 'answers': {}", corta(&j.to_string(), 300)))?;
    let escolha = |k: &str| a[k]["choice"].as_str().and_then(|c| c.parse::<u32>().ok());
    if arrastar {
        let (de, para) = (escolha("de").ok_or("resposta sem 'de'")?, escolha("para").ok_or("resposta sem 'para'")?);
        if de == para {
            return Err(format!("o Decider escolheu a mesma célula ({de}) para pegar e soltar"));
        }
        let conf = |k: &str| a[k]["confidence"].as_f64().unwrap_or(0.0);
        return Ok((
            Jogada::Arrastar(de, para),
            format!("arrastar da célula {de} para a {para} (confiança {:.2})", conf("de").min(conf("para"))),
        ));
    }
    let prob = a["c"]["probabilities"].as_object().cloned().unwrap_or_default();
    let mut cels = melhores_celulas(&prob, colunas_da(img.width(), celula), quantidade(instrucao));
    if cels.is_empty() {
        cels.extend(escolha("c"));
    }
    if cels.is_empty() {
        return Err("o Decider não escolheu célula".into());
    }
    let texto = format!("clicar nas células {cels:?} (confiança {:.2})", a["c"]["confidence"].as_f64().unwrap_or(0.0));
    Ok((Jogada::Clicar(cels), texto))
}

/// Executa a jogada no desafio atual (a captura tem de ter o mesmo tamanho da usada para decidir).
pub fn jogar(est: &Estado, celula: u32, j: &Jogada) -> Result<(), String> {
    match j {
        Jogada::Clicar(c) => clicar_celulas(est, celula, c),
        Jogada::Arrastar(a, b) => arrastar_celulas(est, celula, *a, *b),
    }
}

pub fn relatorio(r: &Resultado) -> String {
    let baixas: Vec<String> = r
        .pecas
        .iter()
        .filter(|p| p.confianca < 0.5)
        .map(|p| format!("[{},{}] p(sim)={:.2}", p.linha, p.coluna, p.p_sim))
        .collect();
    let mut s = format!("{} (grade {}x{})\n{}", r.tarefa.rotulo(), r.grade.linhas, r.grade.colunas, r.desenho());
    if !baixas.is_empty() {
        s.push_str(&format!("\nconfiança baixa: {}", baixas.join(", ")));
    }
    s
}

#[cfg(test)]
mod testes {
    use super::*;
    use image::Rgb;

    /// Captura sintética: cabeçalho azul, grade `l`×`c` de peças coloridas separadas por linhas
    /// brancas e rodapé branco com um botão.
    fn sintetica(l: u32, c: u32) -> RgbImage {
        let (tw, th, gap) = (80u32, 70u32, 3u32);
        let (w, h) = (c * (tw + gap) + 20, 110 + l * (th + gap) + 60);
        let mut img = RgbImage::from_pixel(w, h, Rgb([255, 255, 255]));
        for y in 8..100 {
            for x in 8..w - 8 {
                img.put_pixel(x, y, Rgb([70, 130, 220]));
            }
        }
        for r in 0..l {
            for k in 0..c {
                let (x0, y0) = (10 + k * (tw + gap), 110 + r * (th + gap));
                for y in y0..y0 + th {
                    for x in x0..x0 + tw {
                        img.put_pixel(x, y, Rgb([40 + (r * 30) as u8, 90, 60 + (k * 20) as u8]));
                    }
                }
            }
        }
        for y in h - 40..h - 15 {
            for x in w - 90..w - 12 {
                img.put_pixel(x, y, Rgb([70, 130, 220]));
            }
        }
        img
    }

    #[test]
    fn acha_a_grade_e_o_cabecalho() {
        let g = detectar(&sintetica(3, 3)).unwrap();
        assert_eq!((g.linhas, g.colunas, g.pecas.len()), (3, 3, 9));
        assert_eq!(g.pecas[0], (10, 110, 80, 70));
        assert_eq!(g.pecas[4], (10 + 83, 110 + 73, 80, 70));
        assert_eq!(g.cabecalho.map(|c| (c.1, c.3)), Some((8, 92)));
        let g = detectar(&sintetica(3, 4)).unwrap();
        assert_eq!((g.linhas, g.colunas), (3, 4));
    }

    #[test]
    fn sem_linhas_nao_ha_grade() {
        let img = RgbImage::from_pixel(200, 200, Rgb([90, 120, 60]));
        assert!(detectar(&img).is_none());
        assert!(detectar(&RgbImage::from_pixel(200, 200, Rgb([255, 255, 255]))).is_none());
    }

    #[test]
    fn divisao_igual_com_recorte() {
        let img = RgbImage::new(100, 100);
        let g = dividir(&img, 2, 2, Some((10, 20, 40, 60)));
        assert_eq!(g.pecas, vec![(10, 20, 20, 30), (30, 20, 20, 30), (10, 50, 20, 30), (30, 50, 20, 30)]);
    }

    #[test]
    fn le_resposta_da_peca() {
        let j = json!({"answers": {"r": {"choice": "sim", "confidence": 0.8, "probabilities": {"sim": 0.9, "nao": 0.1}}}});
        assert_eq!(ler_peca(&j).unwrap(), (true, 0.9, 0.8));
        assert!(ler_peca(&json!({"error": "x"})).is_err());
    }

    #[test]
    fn instrucao_no_mapa() {
        let mapa = "e11 iframe \"newassets.hcaptcha.com\"\n  h2 Select all drinks that can be hot or cold\n  e12 clicável \"Select all drinks that can be hot or cold\"\n  e13 button \"Challenge Image 1\"";
        assert_eq!(instrucao_do_mapa(mapa).as_deref(), Some("Select all drinks that can be hot or cold"));
        assert_eq!(instrucao_do_mapa("e1 clicável \"Selecione todos os quadrados com semáforos\"").as_deref(), Some("Selecione todos os quadrados com semáforos"));
        assert_eq!(instrucao_do_mapa("e1 button \"Entrar\"\nh1 Cadastro"), None);
    }

    #[test]
    fn grade_numerada() {
        let cap = Captura { img: RgbImage::from_pixel(100, 60, image::Rgb([0, 0, 255])), x: 10.0, y: 20.0 };
        // 100 px / 48 = 3 colunas; 60 / 48 = 2 linhas.
        assert_eq!(centro(&cap, 48, 1).unwrap(), (10.0 + 24.0, 20.0 + 24.0));
        assert_eq!(centro(&cap, 48, 3).unwrap(), (10.0 + 96.0 + 2.0, 20.0 + 24.0));
        assert_eq!(centro(&cap, 48, 4).unwrap(), (10.0 + 24.0, 20.0 + 48.0 + 6.0));
        assert!(centro(&cap, 48, 7).is_err());
        assert!(centro(&cap, 48, 0).is_err());
        let o = numerar(&cap.img, 48);
        assert_eq!(o.dimensions(), (100, 60));
        assert_eq!(o.get_pixel(3, 3), &image::Rgb([255, 235, 59]));
        assert_ne!(o.get_pixel(48, 30), cap.img.get_pixel(48, 30));
    }

    #[test]
    fn quantidade_e_melhores_celulas() {
        assert_eq!(quantidade("Click the THREE matching shapes"), 3);
        assert_eq!(quantidade("Click the incomplete tower"), 1);
        assert_eq!(quantidade("Clique em 2 formas"), 2);
        let prob = json!({"1": 0.3, "2": 0.25, "30": 0.2, "60": 0.1, "5": 0.05});
        // 11 colunas: 1 e 2 são vizinhas (fica só a 1); 30 e 60 estão longe de tudo.
        assert_eq!(melhores_celulas(prob.as_object().unwrap(), 11, 3), vec![1, 30, 60]);
    }

    #[test]
    fn area_do_desafio_pega_a_faixa_mais_alta() {
        let mut img = RgbImage::from_pixel(200, 300, image::Rgb([255, 255, 255]));
        for y in 10..60 {
            for x in 10..190 {
                img.put_pixel(x, y, image::Rgb([0, 128, 128]));
            }
        }
        for y in 80..250 {
            for x in 20..180 {
                img.put_pixel(x, y, image::Rgb([40, 40, 40]));
            }
        }
        assert_eq!(area_do_desafio(&img), Some((20, 80, 160, 170)));
    }

    #[test]
    fn indicio_no_mapa() {
        assert!(indicio("iframe google.com\n  h1 Verifique que não é um robô (reCAPTCHA)"));
        assert!(indicio("Select all images with cars"));
        assert!(!indicio("e1 button \"Entrar\""));
    }

    #[test]
    fn imagens_de_exemplo() {
        let ler = |n: &str| image::open(format!("{}/tests/fixtures/{n}", env!("CARGO_MANIFEST_DIR"))).unwrap().to_rgb8();
        let g = detectar(&ler("captcha-semaforos.webp")).unwrap();
        assert_eq!((g.linhas, g.colunas), (4, 4));
        let g = detectar(&ler("captcha-carros.jpeg")).unwrap();
        assert_eq!((g.linhas, g.colunas), (3, 3));
        assert!(g.cabecalho.is_some());
        // Captura de quadro real do reCAPTCHA: a miniatura do cabeçalho tem borda branca que o parte em duas colunas.
        let g = detectar(&ler("captcha-recaptcha.jpeg")).unwrap();
        assert_eq!((g.linhas, g.colunas, g.pecas.len()), (3, 3, 9));
        assert!(g.cabecalho.is_some_and(|c| c.1 <= 8));
    }
}
