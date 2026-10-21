//! Testes do `nd` de ponta a ponta: comandos base (start, open, eval, abas, stop).

mod comum;

use std::process::Command;
use std::time::{Duration, Instant};

use comum::{Nd, servir, vivo};

#[tokio::test(flavor = "multi_thread")]
async fn fluxo_completo() {
    let base = servir().await;
    let nd = Nd::new();

    // Command::output lê o stdout até o fim: se o daemon herdasse o pipe, isto travaria.
    let inicio = Instant::now();
    let status = nd.ok(&["start", "--perfil", "temp", "--headless"]);
    assert!(inicio.elapsed() < Duration::from_secs(60));
    assert!(status.contains("perfil temp"), "{status}");
    assert!(nd.estado().exists());

    let segundo = nd.ok(&["start", "--perfil", "temp", "--headless"]);
    assert!(segundo.contains("já está rodando"), "{segundo}");

    // open espera a requisição lenta e a mudança do DOM depois dela.
    let saida = nd.ok(&["open", &format!("{base}/lento.html")]);
    assert!(saida.starts_with("# Lento — 127.0.0.1"), "{saida}");
    assert!(saida.contains("estável em"), "{saida}");
    assert!(saida.contains("h1 Página lenta"), "{saida}");
    assert!(!saida.contains("aviso"), "{saida}");
    let texto = nd.ok(&["text", "--sel", "#dados"]);
    assert_eq!(texto.trim(), "dados: 42");

    assert_eq!(nd.ok(&["eval", "1 + 2"]).trim(), "3");
    assert_eq!(nd.ok(&["eval", "'texto'"]).trim(), "texto");
    let js = nd.dir.path().join("x.js");
    std::fs::write(&js, "({a: [1, 2]})").unwrap();
    let obj = nd.ok(&["eval", "-f", js.to_str().unwrap()]);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&obj).unwrap(),
        serde_json::json!({"a": [1, 2]})
    );

    let erro = nd.run(&["eval", "throw new Error('falhou aqui')"]);
    assert!(!erro.status.success());
    assert!(String::from_utf8_lossy(&erro.stderr).contains("falhou aqui"));

    nd.ok(&["open", &format!("{base}/outra.html"), "--nova"]);
    let abas = nd.ok(&["tabs"]);
    assert_eq!(abas.lines().count(), 2, "{abas}");
    assert!(
        abas.lines().nth(1).unwrap().starts_with("* [2] Outra página"),
        "{abas}"
    );

    // Aba aberta pela página (target=_blank) aparece e pode ser escolhida pelo título.
    nd.ok(&["eval", "document.getElementById('nova').click()"]);
    let fim = Instant::now() + Duration::from_secs(10);
    while nd.ok(&["tabs"]).lines().count() < 3 && Instant::now() < fim {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(nd.ok(&["tabs"]).lines().count(), 3);

    let escolhida = nd.ok(&["tab", "outra"]);
    assert!(escolhida.contains("Outra página"), "{escolhida}");
    assert_eq!(nd.ok(&["eval", "document.title"]).trim(), "Outra página");
    nd.ok(&["tab", "1"]);
    assert_eq!(nd.ok(&["eval", "document.title"]).trim(), "Lento");
    let r = nd.run(&["tab", "9"]);
    assert!(!r.status.success());

    let t = nd.ok(&["text", "--max", "5"]);
    assert!(t.contains("use --max"), "{t}");

    nd.ok(&["stop"]);
    assert!(!nd.estado().exists());
    let r = nd.run(&["status"]);
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("não está rodando"));
}

#[tokio::test(flavor = "multi_thread")]
async fn sessoes_tem_abas_proprias() {
    let base = servir().await;
    let nd = Nd::new();
    nd.ok(&["-s", "a", "start", "--perfil", "temp", "--headless"]);
    nd.ok(&["-s", "a", "open", &format!("{base}/lento.html"), "--sem-map"]);
    nd.ok(&["-s", "b", "open", &format!("{base}/outra.html"), "--sem-map"]);

    // Comandos das duas sessões ao mesmo tempo: cada um na sua aba.
    let titulos: Vec<_> = std::thread::scope(|sc| {
        let tarefas: Vec<_> = (0..4)
            .flat_map(|_| ["a", "b"])
            .map(|s| {
                let nd = &nd;
                sc.spawn(move || (s, nd.ok(&["-s", s, "eval", "document.title"])))
            })
            .collect();
        tarefas.into_iter().map(|t| t.join().unwrap()).collect()
    });
    for (s, titulo) in titulos {
        let esperado = if s == "a" { "Lento" } else { "Outra página" };
        assert_eq!(titulo.trim(), esperado, "sessão {s}");
    }

    let abas = nd.ok(&["-s", "b", "tabs"]);
    assert!(
        abas.contains("Lento") && abas.contains("(outra sessão)"),
        "{abas}"
    );
    let r = nd.run(&["-s", "b", "tab", "Lento"]);
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("outra sessão"));

    // stop de uma sessão fecha só a aba dela.
    let parou = nd.ok(&["-s", "b", "stop"]);
    assert!(parou.contains("segue aberto"), "{parou}");
    assert!(nd.estado().exists());
    assert_eq!(nd.ok(&["-s", "a", "eval", "document.title"]).trim(), "Lento");
    assert!(!nd.ok(&["-s", "a", "tabs"]).contains("Outra página"));
    nd.ok(&["-s", "a", "stop"]);
    assert!(!nd.estado().exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn perfil_em_uso_falha_com_mensagem() {
    let perfil = tempfile::tempdir().unwrap();
    let p = perfil.path().to_str().unwrap();
    let a = Nd::new();
    a.ok(&["start", "--perfil", p, "--headless"]);
    let b = Nd::new();
    let r = b.run(&["start", "--perfil", p, "--headless"]);
    assert!(!r.status.success());
    let err = String::from_utf8_lossy(&r.stderr);
    assert!(err.contains("já está aberto em outro Edge"), "{err}");
    a.ok(&["stop"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn daemon_some_quando_o_edge_morre() {
    let nd = Nd::new();
    nd.ok(&["start", "--perfil", "temp", "--headless"]);
    let estado: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(nd.estado()).unwrap()).unwrap();
    let edge = estado["edge_pid"].as_u64().unwrap();
    let daemon = estado["pid"].as_u64().unwrap();
    Command::new("taskkill")
        .args(["/T", "/F", "/PID", &edge.to_string()])
        .output()
        .unwrap();
    let fim = Instant::now() + Duration::from_secs(20);
    while vivo(daemon) && Instant::now() < fim {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        !vivo(daemon),
        "daemon continuou depois do Edge morrer\n{}",
        nd.log()
    );
    assert!(!nd.estado().exists());
}
