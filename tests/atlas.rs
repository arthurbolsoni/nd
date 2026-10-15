//! Testes do atlas e do replay: lista, parâmetros, passo opcional, parada no passo com
//! `confirmar`, retomada com `--desde`, falha com mapa e rede, checagem de login e rascunho dos
//! passos da sessão.

mod comum;

use comum::{Nd, ref_de, servir};

fn tem(saida: &str, trecho: &str) {
    assert!(saida.contains(trecho), "{trecho:?} não está em:\n{saida}");
}

const ATLAS: &str = r#"site: 127.0.0.1
atualizado: 2026-10-15
login:
  checar: "!!document.querySelector('#nunca')"
  url: /entrar
armadilhas:
  - "Aviso de cookies cobre o botão Salvar"
fluxos:
  cadastro:
    params: [base, nome]
    passos:
      - open: "{base}/form.html"
      - click: Aceitar cookies
        opcional: true
      - fill: { alvo: Nome, valor: "{nome}" }
      - fill: { nome: E-mail, valor: a@b.com }
      - pick: { alvo: Estado, opcao: Rio de Janeiro }
      - check: Aceito os termos
      - pick: { alvo: Cidade, opcao: Santos }
      - pick: { alvo: Escolha a cor, opcao: Verde }
      - click: { alvo: Salvar, confirmar: true }
      - esperar: { texto: "enviado: {nome}" }
  quebrado:
    params: { base: null }
    passos:
      - open: "{base}/form.html"
      - click: Botão que não existe
  logado:
    login: true
    params: [base]
    passos:
      - open: "{base}/form.html"
      - click: Somar
"#;

#[tokio::test(flavor = "multi_thread")]
async fn atlas_e_replay() {
    let base = servir().await;
    let nd = Nd::new();
    let sites = nd.dir.path().join("sites");
    std::fs::create_dir_all(&sites).unwrap();
    std::fs::write(sites.join("127.0.0.1.yaml"), ATLAS).unwrap();
    let p_base = format!("base={base}");

    tem(
        &nd.ok(&["atlas"]),
        "127.0.0.1 (atualizado 2026-10-15) — fluxos: cadastro, quebrado, logado",
    );
    tem(&nd.ok(&["atlas", "127"]), "Aviso de cookies cobre o botão Salvar");
    let r = nd.run(&["atlas", "facebook"]);
    assert!(!r.status.success());
    tem(&String::from_utf8_lossy(&r.stderr), "sem atlas para facebook");

    let r = nd.run(&["run", "127.0.0.1", "cadastro", "-p", &p_base, "--seco"]);
    assert!(!r.status.success());
    tem(&String::from_utf8_lossy(&r.stderr), "faltam parâmetros: nome");
    let s = nd.ok(&[
        "run",
        "127",
        "cadastro",
        "-p",
        &p_base,
        "-p",
        "nome=Ana Lima",
        "--seco",
    ]);
    tem(&s, &format!("[1/10] open \"{base}/form.html\""));
    tem(&s, "[3/10] fill {\"alvo\":\"Nome\",\"valor\":\"Ana Lima\"}");
    tem(&s, "[9/10] click {\"alvo\":\"Salvar\"} (confirmar)");

    nd.ok(&["start", "--perfil", "temp", "--headless"]);
    // Para antes do passo com confirmar e diz como seguir.
    let s = nd.ok(&["run", "127", "cadastro", "-p", &p_base, "-p", "nome=Ana Lima"]);
    tem(&s, "[8/10] pick");
    tem(&s, "escolhido \"Verde\"");
    tem(&s, "[9/10] click {\"alvo\":\"Salvar\"} pede confirmação");
    tem(&s, "--desde 9 --confirmado");
    assert!(!s.contains("enviado:"), "{s}");
    let s = nd.ok(&[
        "run",
        "127",
        "cadastro",
        "-p",
        &p_base,
        "-p",
        "nome=Ana Lima",
        "--desde",
        "9",
        "--confirmado",
    ]);
    tem(&s, "[10/10] wait");
    tem(&s, "fluxo cadastro concluído");
    tem(
        &nd.ok(&["text", "--sel", "#resultado"]),
        "enviado: Ana Lima / a@b.com / RJ / true / Santos / Verde",
    );

    // --ate para no meio.
    let s = nd.ok(&[
        "run", "127", "cadastro", "-p", &p_base, "-p", "nome=X", "--ate", "3",
    ]);
    tem(&s, "parado no passo 3 de 10 (--ate)");

    // Falha: passo, erro, mapa, rede e como retomar.
    let r = nd.run(&["run", "127", "quebrado", "-p", &p_base]);
    assert!(!r.status.success());
    let err = String::from_utf8_lossy(&r.stderr);
    tem(&err, "passo 2/2 falhou: click \"Botão que não existe\"");
    tem(&err, "nenhum elemento com");
    tem(&err, "--- nd map");
    tem(&err, "button \"Somar\"");
    tem(&err, "retomar: nd run 127 quebrado -p");

    let r = nd.run(&["run", "127", "logado", "-p", &p_base]);
    assert!(!r.status.success());
    tem(
        &String::from_utf8_lossy(&r.stderr),
        "não há login em 127.0.0.1 (/entrar)",
    );

    // Rascunho: o passo feito por ref vira passo pelo nome.
    nd.ok(&["open", &format!("{base}/form.html"), "--sem-map"]);
    let mapa = nd.ok(&["map"]);
    nd.ok(&["fill", &ref_de(&mapa, "\"Nome\""), "Bia"]);
    nd.ok(&["key", "Tab"]);
    let s = nd.ok(&["atlas", "--rascunho"]);
    tem(&s, "- open: ");
    tem(&s, "- fill:\n    alvo: Nome\n    valor: Bia");
    tem(&s, "- key: Tab");

    nd.ok(&["stop"]);
}
