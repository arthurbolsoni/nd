//! `nd scroll` na página `rolagem.html`: rolagem infinita da página (IntersectionObserver +
//! `/api/lista?pagina=N`), lista com rolagem própria (`--dentro`), `--fim` parando sozinho, limite,
//! subida, histórico (`nd atlas --rascunho`), o passo `scroll` no replay e o diff do que sai da
//! vista sem sair da página (lista com rolagem própria, carrossel).

mod comum;

use comum::{Nd, servir};

fn tem(saida: &str, trecho: &str) {
    assert!(saida.contains(trecho), "{trecho:?} não está em:\n{saida}");
}

const CONTAR: &str = "document.querySelectorAll('.anuncio').length";

#[tokio::test(flavor = "multi_thread")]
async fn rolagem_infinita() {
    let base = servir().await;
    let nd = Nd::new();
    nd.ok(&["start", "--perfil", "temp", "--headless"]);
    nd.ok(&["open", &format!("{base}/rolagem.html")]);
    assert_eq!(nd.ok(&["eval", CONTAR]).trim(), "10");

    // Carrossel por transform: os cartões que saem da caixa seguem na página, não "sumiram".
    let s = nd.ok(&["click", "Próximo"]);
    tem(
        &s,
        "fora da vista 3 (seguem na página): e11 \"Cartão 1\"; e12 \"Cartão 2\"; e13 \"Cartão 3\"",
    );
    assert!(!s.contains("sumiram"), "{s}");

    // Uma rolagem: chega ao sentinela, a página pede a próxima e os anúncios novos saem no diff.
    let s = nd.ok(&["scroll"]);
    tem(&s, "1 rolagem(ns) para baixo");
    tem(&s, "/api/lista?pagina=2");
    tem(&s, "+ ");
    tem(&s, "\"Anúncio 11\"");
    assert!(!s.contains("fim:"), "{s}");

    // --fim com limite curto para antes do fim e avisa.
    tem(
        &nd.ok(&["scroll", "--fim", "--vezes", "1"]),
        "parou no limite de 1 rolagens",
    );

    // --fim para sozinho quando a página não cresce mais.
    let s = nd.ok(&["scroll", "--fim"]);
    tem(&s, "fim: a página parou de crescer");
    tem(&s, "\"Anúncio 40\"");
    assert_eq!(nd.ok(&["eval", CONTAR]).trim(), "40");
    assert_eq!(
        nd.ok(&["eval", "document.getElementById('sentinela').textContent"])
            .trim(),
        "fim da lista"
    );

    // Já no fim: uma rolagem a mais não acha nada novo.
    tem(&nd.ok(&["scroll", "--fim"]), "fim: a página parou de crescer");

    // Subida até o topo.
    let s = nd.ok(&["scroll", "--subir", "--fim"]);
    tem(&s, "para cima");
    tem(&s, "a página está no topo");
    assert_eq!(nd.ok(&["eval", "window.scrollY"]).trim(), "0");

    // Dentro da lista com rolagem própria (perto do topo da página), pelo texto de um item dela;
    // a página fica parada. As opções que a rolagem leva para cima da área seguem no mapa.
    let s = nd.ok(&["scroll", "--dentro", "Opção 1"]);
    tem(&s, "área com rolagem:");
    tem(&s, "nada novo carregou");
    assert!(!s.contains("sumiram"), "{s}");
    // Até o fim: as 20 opções carregadas saem como elementos novos, mesmo as que passaram pela
    // área e ficaram acima dela.
    let s = nd.ok(&["scroll", "--dentro", "Opção 1", "--fim"]);
    let novas: Vec<&str> = s
        .lines()
        .filter(|l| l.starts_with("+ e") && l.contains("button \"Opção "))
        .collect();
    assert_eq!(novas.len(), 20, "{s}");
    assert!(novas[0].ends_with("button \"Opção 11\""), "{s}");
    assert!(novas[19].ends_with("button \"Opção 30\""), "{s}");
    tem(&s, "fim: a área parou de crescer");
    assert!(!s.contains("sumiram"), "{s}");
    assert!(!s.contains("texto novo"), "{s}");
    // Opção que ficou acima da área: o clique a traz para a vista.
    tem(&nd.ok(&["click", "Opção 2"]), "clique em e2 button \"Opção 2\"");
    let topo: f64 = nd
        .ok(&["eval", "document.getElementById('painel').scrollTop"])
        .trim()
        .parse()
        .unwrap();
    assert!(topo < 80.0, "{topo}");
    assert_eq!(
        nd.ok(&["eval", "document.getElementById('painel').children.length"])
            .trim(),
        "30"
    );
    assert_eq!(nd.ok(&["eval", "window.scrollY"]).trim(), "0");

    // Elemento fora de área com rolagem própria.
    let r = nd.run(&["scroll", "--dentro", "Anúncios"]);
    assert!(!r.status.success());
    tem(&String::from_utf8_lossy(&r.stderr), "não tem rolagem própria");

    let rascunho = nd.ok(&["atlas", "--rascunho"]);
    tem(&rascunho, "- scroll: 1");
    tem(&rascunho, "- scroll: fim");
    tem(&rascunho, "dentro: Opção 1");
    tem(&rascunho, "subir: true");
}

#[tokio::test(flavor = "multi_thread")]
async fn passo_scroll_no_replay() {
    let base = servir().await;
    let nd = Nd::new();
    let sites = nd.dir.path().join("sites");
    std::fs::create_dir_all(&sites).unwrap();
    std::fs::write(
        sites.join("127.0.0.1.yaml"),
        format!(
            "site: 127.0.0.1\nfluxos:\n  anuncios:\n    params: {{ base: null, rolagens: 2 }}\n    passos:\n      \
             - open: \"{{base}}/rolagem.html\"\n      - scroll: \"{{rolagens}}\"\n      - eval: \"{CONTAR}\"\n  \
             todos:\n    params: [base]\n    passos:\n      - open: \"{{base}}/rolagem.html\"\n      \
             - scroll: fim\n      - eval: \"{CONTAR}\"\n"
        ),
    )
    .unwrap();
    nd.ok(&["start", "--perfil", "temp", "--headless"]);
    let p = format!("base={base}");
    let s = nd.ok(&["run", "127.0.0.1", "anuncios", "-p", &p]);
    tem(&s, "[2/3] scroll \"2\"");
    tem(&s, "2 rolagem(ns) para baixo");
    tem(&s, "[3/3] eval");
    tem(&s, "\n  30\n");
    let s = nd.ok(&["run", "127.0.0.1", "todos", "-p", &p]);
    tem(&s, "fim: a página parou de crescer");
    tem(&s, "\n  40\n");
}
