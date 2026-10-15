//! Testes de arquivos na página `arquivos.html`: download comum na saída do clique, `nd download`
//! (PDF em aba nova por GET e POST, blob em aba, blob baixado, blob só na página), `--url`, lista
//! e `nd pdf`.

mod comum;

use comum::{Nd, PDF, servir};

fn tem(saida: &str, trecho: &str) {
    assert!(saida.contains(trecho), "{trecho:?} não está em:\n{saida}");
}

/// Caminho gravado numa linha "arquivo (...): CAMINHO (tamanho...)" ou "download: CAMINHO (...)".
fn caminho(saida: &str, inicio: &str) -> std::path::PathBuf {
    let l = saida
        .lines()
        .find(|l| l.starts_with(inicio))
        .unwrap_or_else(|| panic!("{inicio:?} não está em:\n{saida}"));
    let resto = l.split_once(": ").unwrap().1;
    resto.rsplit_once(" (").unwrap().0.into()
}

#[tokio::test(flavor = "multi_thread")]
async fn downloads_e_pdf() {
    let base = servir().await;
    let nd = Nd::new();
    let outra = tempfile::tempdir().unwrap();
    nd.ok(&["start", "--perfil", "temp", "--headless"]);
    nd.ok(&["open", &format!("{base}/arquivos.html"), "--sem-map"]);
    let pasta = nd.dir.path().join("downloads");

    // Anexo: o navegador baixa, a saída do clique mostra o arquivo com o nome sugerido.
    let s = nd.ok(&["click", "Baixar anexo"]);
    let p = caminho(&s, "download: ");
    assert_eq!(p, pasta.join("anexo.pdf"), "{s}");
    tem(&s, "doc /anexo → download");
    assert_eq!(std::fs::read(&p).unwrap(), PDF);

    // PDF por GET numa aba nova: pego pela resposta; a aba nova aparece na saída.
    let s = nd.ok(&["download", "Abrir PDF"]);
    tem(&s, "arquivo (resposta GET");
    tem(&s, "aba nova [");
    let p = caminho(&s, "arquivo (");
    assert_eq!(p, pasta.join("doc.pdf"));
    assert_eq!(std::fs::read(&p).unwrap(), PDF);
    nd.ok(&["tab", "arquivos"]);

    // PDF por POST numa aba nova, gravado noutra pasta.
    let s = nd.ok(&["download", "Gerar PDF", "--pasta", outra.path().to_str().unwrap()]);
    tem(&s, "arquivo (resposta POST");
    let p = caminho(&s, "arquivo (");
    assert_eq!(p, outra.path().join("gerar.pdf"));
    assert_eq!(std::fs::read(&p).unwrap(), PDF);
    nd.ok(&["tab", "arquivos"]);

    // Blob aberto numa aba e revogado logo depois: lido do Map guardado pelo gancho.
    let s = nd.ok(&["download", "Blob em aba"]);
    tem(&s, "arquivo (blob em aba nova)");
    let p = caminho(&s, "arquivo (");
    assert_eq!(std::fs::read(&p).unwrap(), b"%PDF-1.4 blob\n");
    nd.ok(&["tab", "arquivos"]);

    // Blob baixado por a[download]: download do navegador.
    let s = nd.ok(&["download", "Blob baixar"]);
    tem(&s, "arquivo (download)");
    let p = caminho(&s, "arquivo (");
    assert_eq!(p, pasta.join("tabela.csv"));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "a;b\n1;2\n");

    // Blob criado e só mostrado num iframe.
    let s = nd.ok(&["download", "Blob na página"]);
    tem(&s, "arquivo (blob da página)");
    assert_eq!(std::fs::read_to_string(caminho(&s, "arquivo (")).unwrap(), "ola");

    let r = nd.run(&["download", "Não faz nada", "--max", "2"]);
    assert!(!r.status.success());
    tem(&String::from_utf8_lossy(&r.stderr), "não gerou arquivo em 2 s");

    // URL baixada com a sessão da aba (relativa à página).
    let s = nd.ok(&["download", "--url", "/doc.pdf"]);
    tem(&s, "arquivo (url, status 200)");
    assert_eq!(caminho(&s, "arquivo ("), pasta.join("doc (2).pdf"));
    let saida = outra.path().join("x.bin");
    nd.ok(&[
        "download",
        "--url",
        "/arquivo.bin",
        "--saida",
        saida.to_str().unwrap(),
    ]);
    assert_eq!(std::fs::read(&saida).unwrap(), (0u8..=255).collect::<Vec<u8>>());

    let lista = nd.ok(&["download"]);
    for t in ["anexo.pdf", "gerar.pdf", "tabela.csv", "x.bin"] {
        tem(&lista, t);
    }

    let arq = outra.path().join("pagina.pdf");
    tem(&nd.ok(&["pdf", arq.to_str().unwrap()]), "arquivo (pdf da página)");
    assert!(std::fs::read(&arq).unwrap().starts_with(b"%PDF"));

    nd.ok(&["stop"]);
}
