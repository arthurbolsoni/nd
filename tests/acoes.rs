//! Testes do `nd map` e das ações na página `form.html`: input controlado como o do React,
//! select, checkbox escondido, autocomplete, lista customizada, overlay, alert, rede, upload
//! (direto e pelo seletor), diálogo modal, shadow DOM, iframe do mesmo domínio e de outro site,
//! re-render e navegação.

mod comum;

use comum::{Nd, ref_de, servir};

fn tem(saida: &str, trecho: &str) {
    assert!(saida.contains(trecho), "{trecho:?} não está em:\n{saida}");
}

#[tokio::test(flavor = "multi_thread")]
async fn mapa_e_acoes() {
    let base = servir().await;
    let nd = Nd::new();
    let arqs = tempfile::tempdir().unwrap();
    let um = arqs.path().join("um.txt");
    let dois = arqs.path().join("dois.pdf");
    std::fs::write(&um, "1").unwrap();
    std::fs::write(&dois, "2").unwrap();
    nd.ok(&["start", "--perfil", "temp", "--headless"]);

    let mapa = nd.ok(&["open", &format!("{base}/form.html")]);
    assert!(mapa.starts_with("# Cadastro — 127.0.0.1"), "{mapa}");
    // O documento do iframe de outro site não segura a espera de rede.
    assert!(!mapa.contains("aviso:"), "{mapa}");
    for trecho in [
        "h1 Cadastro de candidato",
        "form \"Dados pessoais\"",
        "input[text] \"Nome\" [obrigatório]",
        "select \"Estado\" = \"Selecione\" {Selecione|São Paulo|Rio de Janeiro|Minas Gerais}",
        "checkbox \"Aceito os termos\" [desmarcado]",
        "combobox \"Cidade\"",
        "clicável \"Card clicável com duas linhas\"",
        "input[file] #oculto [oculto]",
        "button \"Somar\"",
        "button \"Botão interno\"",
        "iframe \"localhost:",
        "button \"Botão lá embaixo\"",
    ] {
        tem(&mapa, trecho);
    }
    // Conteúdo do iframe de outro site, dentro do form dele (iframe > form > input).
    assert!(
        mapa.lines()
            .any(|l| l.starts_with("    e") && l.contains("input[text] \"CEP\"")),
        "{mapa}"
    );
    assert!(!mapa.contains("Tem certeza"), "diálogo fechado apareceu:\n{mapa}");

    let r = nd.run(&["click", "Salvar"]);
    assert!(!r.status.success());
    let err = String::from_utf8_lossy(&r.stderr);
    tem(&err, "coberto por");
    tem(&err, "Usamos cookies");
    tem(&nd.ok(&["click", "Aceitar cookies"]), "- sumiram 1:");

    // Input controlado: a digitação passa pelo estado do "framework".
    tem(
        &nd.ok(&["fill", "Nome", "Arthur Bolsoni"]),
        "~ e2 input[text] \"Nome\" = \"Arthur Bolsoni\"",
    );
    assert_eq!(nd.ok(&["text", "--sel", "#eco"]).trim(), "Arthur Bolsoni");
    nd.ok(&["fill", "E-mail", "a@b.com"]);
    tem(
        &nd.ok(&["pick", "Estado", "rio de"]),
        "escolhido \"Rio de Janeiro\"",
    );
    tem(
        &nd.ok(&["check", "Aceito os termos"]),
        "checkbox \"Aceito os termos\" [marcado]",
    );
    tem(&nd.ok(&["check", "Aceito os termos"]), "já estava marcado");
    tem(&nd.ok(&["check", "Aceito os termos", "--nao"]), "[desmarcado]");
    nd.ok(&["check", "Aceito os termos"]);
    tem(&nd.ok(&["pick", "Cidade", "Santos"]), "escolhido \"Santos\"");
    tem(&nd.ok(&["pick", "Escolha a cor", "verde"]), "escolhido \"Verde\"");
    let s = nd.ok(&["click", "Salvar"]);
    tem(&s, "texto novo:");
    tem(
        &s,
        "enviado: Arthur Bolsoni / a@b.com / RJ / true / Santos / Verde",
    );

    let s = nd.ok(&["click", "Avisar"]);
    tem(&s, "diálogo alert \"Salvo!\" aceito");
    tem(&s, "depois do alert");

    // O diff espera a requisição de 1,5 s e o DOM depois dela.
    tem(&nd.ok(&["click", "Carregar dados"]), "valor 42");
    tem(&nd.ok(&["wait", "net", "/api/lenta"]), "GET 200");

    let mapa = nd.ok(&["map"]);
    tem(
        &nd.ok(&["upload", &ref_de(&mapa, "#arquivo"), um.to_str().unwrap()]),
        "arquivo: um.txt",
    );
    tem(
        &nd.ok(&["upload", "Anexar currículo", dois.to_str().unwrap()]),
        "oculto: dois.pdf",
    );

    let s = nd.ok(&["click", "Abrir diálogo"]);
    tem(&s, "dialog \"Confirmação\"");
    tem(&s, "input[text] \"Motivo\"");
    let m = nd.ok(&["map"]);
    tem(&m, "diálogo modal aberto");
    assert!(!m.contains("Somar"), "fundo do modal apareceu:\n{m}");
    nd.ok(&["fill", "Motivo", "teste"]);
    tem(&nd.ok(&["click", "Confirmar"]), "confirmado: teste");

    let s = nd.ok(&["click", "Somar"]);
    tem(&s, "texto novo:\n  1");
    tem(&nd.ok(&["click", "Botão interno"]), "button \"Interno clicado\"");

    // Iframe de outro site: foco por clique, upload pela sessão do target dele.
    let mapa = nd.ok(&["map"]);
    nd.ok(&["fill", "CEP", "01310-100"]);
    nd.ok(&["upload", &ref_de(&mapa, "#doc"), um.to_str().unwrap()]);
    tem(&nd.ok(&["click", "Enviar externo"]), "externo: 01310-100 um.txt");

    // Re-render troca o nó: a ref é achada de novo pelo papel e nome.
    let antigo = ref_de(&nd.ok(&["map"]), "\"Confirmar pedido\"");
    nd.ok(&["click", "Trocar botão"]);
    let s = nd.ok(&["click", &antigo]);
    tem(&s, "recriado pela página");
    tem(&s, "alvo novo clicado");

    nd.ok(&["fill", "Observações", "abc", "--sem-diff"]);
    nd.ok(&["key", "Control+a", "x", "--em", "Observações", "--sem-diff"]);
    assert_eq!(
        nd.ok(&["eval", "document.getElementById('obs').value"]).trim(),
        "x"
    );

    let r = nd.run(&["click", "Botão que não existe"]);
    assert!(!r.status.success());
    tem(&String::from_utf8_lossy(&r.stderr), "nenhum elemento com");

    tem(&nd.ok(&["wait", "text", "alvo novo"]), "ok em");
    let r = nd.run(&["wait", "text", "nunca aparece", "--max", "1"]);
    assert!(!r.status.success());

    let s = nd.ok(&["click", "Ir para outra página"]);
    tem(&s, "página nova\n# Outra página");
    tem(&s, "link \"abrir lenta\"");
    let r = nd.run(&["click", &antigo]);
    assert!(!r.status.success());
    tem(&String::from_utf8_lossy(&r.stderr), "página anterior");

    nd.ok(&["stop"]);
}
