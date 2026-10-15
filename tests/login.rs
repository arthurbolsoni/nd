//! Testes do `nd login` na página `login.html`: formulário já preenchido (o papel do Edge),
//! envio pelo botão, conferência pelo campo de senha e pelo `--checar`, senha mascarada no mapa,
//! no `nd net` e no rascunho; sem senha salva, o login não envia nada.

mod comum;

use comum::{Nd, servir};

fn tem(saida: &str, trecho: &str) {
    assert!(saida.contains(trecho), "{trecho:?} não está em:\n{saida}");
}

fn nao_tem(saida: &str, trecho: &str) {
    assert!(!saida.contains(trecho), "{trecho:?} apareceu em:\n{saida}");
}

#[tokio::test(flavor = "multi_thread")]
async fn login_com_senha_preenchida() {
    let base = servir().await;
    let nd = Nd::new();
    nd.ok(&["start", "--perfil", "temp", "--headless"]);

    // Sem senha salva (perfil temporário): não abre sugestão, não envia o formulário vazio.
    nd.ok(&["open", &format!("{base}/login.html"), "--sem-map"]);
    let r = nd.run(&["login"]);
    assert!(!r.status.success());
    tem(
        &String::from_utf8_lossy(&r.stderr),
        "o Edge não preencheu usuário e senha",
    );
    assert!(
        nd.ok(&["net", "login=1"]).contains("nenhuma chamada"),
        "enviou o form vazio"
    );

    // Preenchido: envia pelo botão do formulário e confere que o campo de senha sumiu.
    let mapa = nd.ok(&["open", &format!("{base}/login.html?pre=1")]);
    tem(&mapa, "input[password] \"Senha\" = \"••••\"");
    nao_tem(&mapa, "segredo123");
    let s = nd.ok(&["login"]);
    tem(&s, "usuário e senha já preenchidos");
    tem(&s, "enviado por clique em");
    tem(&s, "button \"Entrar\"");
    tem(&s, "login ok: o campo de senha sumiu");
    tem(&s, "logada (POST)");

    // A senha não aparece no corpo guardado.
    let net = nd.ok(&["net", "login=1"]);
    let n = net
        .split_whitespace()
        .next()
        .unwrap()
        .trim_start_matches('#')
        .to_string();
    let corpo = nd.ok(&["net", "--corpo", &n]);
    tem(&corpo, "pass = ••••");
    tem(&corpo, "email = ana@exemplo.com");
    // (a resposta do eco devolve o corpo recebido; só o enviado é guardado com máscara)
    nao_tem(corpo.split("resposta:").next().unwrap(), "segredo123");

    // --checar (o login.checar do atlas) decide o resultado.
    nd.ok(&["open", &format!("{base}/login.html?pre=1"), "--sem-map"]);
    let s = nd.ok(&["login", "--checar", "!!document.getElementById('ok')"]);
    tem(&s, "login ok (login.checar deu true)");

    // Senha digitada pelo agente não vai para o rascunho do atlas.
    nd.ok(&["open", &format!("{base}/login.html"), "--sem-map"]);
    let s = nd.ok(&["fill", "Senha", "outra-senha"]);
    nao_tem(&s, "outra-senha");
    let rascunho = nd.ok(&["atlas", "--rascunho"]);
    tem(&rascunho, "- login: {}");
    tem(&rascunho, "valor: '{senha}'");
    nao_tem(&rascunho, "outra-senha");

    // Duas etapas com autocomplete="new-password" (como o gov.br): usuário na primeira tela,
    // senha na segunda; a dica do campo é trocada antes de pedir a sugestão.
    nd.ok(&["open", &format!("{base}/login2.html?pre=1"), "--sem-map"]);
    let r = nd.run(&["login"]);
    assert!(!r.status.success());
    tem(&String::from_utf8_lossy(&r.stderr), "nd login --usuario");
    let s = nd.ok(&[
        "login",
        "--usuario",
        "12345678909",
        "--checar",
        "!!document.getElementById('ok')",
    ]);
    tem(&s, "usuário preenchido na primeira etapa");
    tem(&s, "tela da senha aberta");
    tem(&s, "enviado por clique em");
    tem(&s, "login ok (login.checar deu true)");
    tem(&nd.ok(&["text", "--sel", "#ok"]), "conta 12345678909");
    // Já logado: nada a fazer.
    let s = nd.ok(&["login", "--checar", "!!document.getElementById('ok')"]);
    tem(&s, "já logado; nada enviado");
    // Sem senha salva: troca a dica, recria o campo e desiste sem enviar.
    nd.ok(&["open", &format!("{base}/login2.html"), "--sem-map"]);
    let r = nd.run(&["login", "--usuario", "12345678909"]);
    assert!(!r.status.success());
    tem(&String::from_utf8_lossy(&r.stderr), "o Edge não preencheu senha");
    assert!(
        nd.ok(&["net", "etapa=2"]).contains("nenhuma chamada"),
        "enviou sem senha"
    );

    nd.ok(&["stop"]);
}
