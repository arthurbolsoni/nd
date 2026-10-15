//! Testes de `nd net` e `nd api` na página `rede.html`: captura com corpo, forma do JSON,
//! operação GraphQL, JSON em sequência, binário, caminho, alteração de corpo e query, cookies e
//! cabeçalhos do site no fetch refeito.

mod comum;

use comum::{Nd, servir};

fn tem(saida: &str, trecho: &str) {
    assert!(saida.contains(trecho), "{trecho:?} não está em:\n{saida}");
}

/// Número (`12`) da chamada de `nd net` cuja linha contém `trecho`.
fn numero(net: &str, trecho: &str) -> String {
    let l = net
        .lines()
        .find(|l| l.contains(trecho))
        .unwrap_or_else(|| panic!("{trecho:?} não está em:\n{net}"));
    l.split_whitespace()
        .next()
        .unwrap()
        .trim_start_matches('#')
        .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn net_e_api() {
    let base = servir().await;
    let nd = Nd::new();
    let arqs = tempfile::tempdir().unwrap();
    nd.ok(&["start", "--perfil", "temp", "--headless"]);
    nd.ok(&["open", &format!("{base}/rede.html")]);

    let s = nd.ok(&["click", "Buscar lista"]);
    tem(&s, "rede (1):");
    tem(&s, "GET 200 /api/lista?pagina=1");
    tem(
        &s,
        "→ {itens: [{id: num, nome: str} ×3], pagina: num, total: num}",
    );
    tem(&nd.ok(&["wait", "net", "/api/lista"]), "GET 200");
    tem(&nd.ok(&["click", "Enviar JSON"]), "POST 200 /api/eco?x=1 [Busca]");
    tem(&nd.ok(&["click", "Enviar form"]), "[ListaQuery]");
    tem(&nd.ok(&["click", "Stream"]), "→ 2 JSON em sequência: {a: num}");
    tem(
        &nd.ok(&["click", "Baixar binário"]),
        "→ binário (application/octet-stream)",
    );

    let net = nd.ok(&["net"]);
    tem(&net, "GET 200 doc /rede.html");
    let lista = numero(&net, "/api/lista");
    let json = numero(&net, "[Busca]");
    let form = numero(&net, "[ListaQuery]");
    let bin = numero(&net, "/arquivo.bin");
    assert_eq!(nd.ok(&["net", "/api/lista"]).lines().count(), 1);
    // "lista" pega a URL /api/lista e a operação ListaQuery.
    assert_eq!(nd.ok(&["net", "lista"]).lines().count(), 2);
    let b = nd.ok(&["net", "busca"]);
    assert_eq!(b.lines().count(), 1, "filtro pela operação:\n{b}");
    let novas = nd.ok(&["net", "--novas"]);
    assert_eq!(novas.lines().count(), 1, "{novas}");
    tem(&novas, "/arquivo.bin");

    let s = nd.ok(&["net", "--corpo", &lista, "--caminho", "itens[].nome"]);
    tem(&s, "[\n  \"a\",\n  \"b\",\n  \"c\"\n]");
    let s = nd.ok(&["net", "--corpo", &json]);
    tem(&s, "operação: Busca");
    tem(&s, "\"termo\": \"casa\"");
    let s = nd.ok(&["net", "--corpo", &form]);
    tem(&s, "enviado (form):");
    tem(&s, "variables = {\"count\":2}");
    let r = nd.run(&["net", "--corpo", &lista, "--caminho", "nada"]);
    assert!(!r.status.success());
    tem(
        &String::from_utf8_lossy(&r.stderr),
        "chaves: itens, pagina, total",
    );
    let arq = arqs.path().join("lista.json");
    tem(
        &nd.ok(&["net", "--corpo", &lista, "--saida", arq.to_str().unwrap()]),
        "gravada em",
    );
    assert!(std::fs::read_to_string(&arq).unwrap().contains("\"total\":9"));

    // Refeita pela página: query nova, cookie do site, número da chamada capturada.
    let s = nd.ok(&["api", &lista, "--query", "pagina=2"]);
    tem(&s, "GET 200 application/json");
    tem(&s, "(capturada como #");
    tem(&s, "forma: {itens:");
    tem(&s, "\"pagina\": 2");
    let s = nd.ok(&[
        "api",
        &json,
        "--set",
        "variables.termo=praia",
        "--caminho",
        "corpo",
    ]);
    tem(&s, "praia");
    tem(&s, "\\\"pagina\\\":1");
    let s = nd.ok(&["api", &json, "--caminho", "token"]);
    tem(&s, "\"tok1\"");
    let s = nd.ok(&["api", &json, "--caminho", "cookie"]);
    tem(&s, "sessao=abc123");
    let s = nd.ok(&["api", &form, "--set", "variables.count=10", "--caminho", "corpo"]);
    tem(
        &s,
        "fb_api_req_friendly_name=ListaQuery&variables=%7B%22count%22%3A10%7D",
    );
    let s = nd.ok(&[
        "api",
        &json,
        "--json",
        r#"{"variables":{"pagina":5}}"#,
        "--caminho",
        "corpo",
    ]);
    tem(&s, "\\\"pagina\\\":5");
    tem(&s, "\\\"termo\\\":\\\"casa\\\"");

    let arq = arqs.path().join("arquivo.bin");
    tem(
        &nd.ok(&["api", &bin, "--saida", arq.to_str().unwrap()]),
        "gravado em",
    );
    assert_eq!(std::fs::read(&arq).unwrap(), (0u8..=255).collect::<Vec<u8>>());

    let r = nd.run(&["api", "9999"]);
    assert!(!r.status.success());
    tem(&String::from_utf8_lossy(&r.stderr), "não está mais guardada");
    // Iframe de outro site (processo próprio) e worker: as chamadas entram no buffer da aba,
    // marcadas com a origem.
    tem(&nd.ok(&["click", "Consultar CEP"]), "cep ");
    nd.ok(&["wait", "net", "cep=01310"]);
    let net = nd.ok(&["net", "cep="]);
    tem(&net, "?cep=01310 (iframe)");
    let cep = numero(&net, "cep=01310");
    tem(&nd.ok(&["net", "--corpo", &cep]), "\"cep\": \"01310-100\"");
    // Refeita dentro do iframe: origem e cookies são os dele.
    let s = nd.ok(&["api", &cep, "--caminho", "origem"]);
    tem(&s, "\"http://localhost:");
    nd.ok(&["click", "Worker"]);
    nd.ok(&["wait", "net", "pagina=7"]);
    let s = nd.ok(&["net", "pagina=7"]);
    assert_eq!(s.lines().count(), 1, "{s}");
    tem(&s, "/api/lista?pagina=7 (worker)");
    tem(&s, "→ {itens:");
    tem(&nd.ok(&["text", "--sel", "#saida"]), "worker 7");

    // Outra página: `nd net` mostra só as chamadas dela; `--antigas` traz as anteriores.
    nd.ok(&["open", &format!("{base}/outra.html"), "--sem-map"]);
    let s = nd.ok(&["net"]);
    assert_eq!(s.lines().count(), 1, "{s}");
    tem(&s, "doc /outra.html");
    tem(&nd.ok(&["net", "--antigas"]), "/api/lista");

    nd.ok(&["stop"]);
}
