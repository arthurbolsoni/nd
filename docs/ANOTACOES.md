# Anotações

## 2026-10-15

### Decisões

- Linguagem: Rust nos dois repos.
- `nodriver-rs`: fork público de `ultrafunkamsterdam/nodriver`; recebe a biblioteca Rust.
- `nd`: repo privado; ferramenta que usa a biblioteca.
- Cliente CDP próprio, sem chromiumoxide: chromiumoxide ativa domínios por conta própria
  (`Runtime.enable`), detectável por anti-bot.

### Nodriver (upstream)

- Licença AGPL-3.0. O fork e o porte em Rust ficam AGPL. Uso privado do `nd` sem distribuição
  não gera obrigação de publicar o código do `nd`; distribuir o `nd` ou oferecê-lo como serviço
  de rede gera.
- Versão no `main` do fork: 0.50.6 (commit `219a60d`). Instalada no pyenv: 0.50.3.
- Núcleo em `nodriver/core/`: `browser.py` 907 linhas, `tab.py` 2082, `element.py` 1205,
  `connection.py` 564, `config.py` 320, `util.py` 5063.
- Tipos CDP gerados por `generate_cdp.py` em `nodriver/cdp/` (64 módulos).
- Flags padrão em `config.py` (linhas ~117–199): `--remote-allow-origins=*`, `--no-first-run`,
  `--no-service-autorun`, `--no-default-browser-check`, `--homepage=about:blank`, `--no-pings`,
  `--password-store=basic`, `--disable-infobars`, `--disable-breakpad`,
  `--disable-dev-shm-usage`, `--disable-session-crashed-bubble`,
  `--disable-search-engine-choice-screen`, `--user-data-dir`, `--disable-features=…`,
  `--remote-debugging-host/port`.

### Ambiente

- Rust 1.95.0, toolchain `stable-x86_64-pc-windows-msvc`.
- Edge: `C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe`. Chrome desinstalado.
- Perfil de automação: `~/nodriver/profile-edge` (um Edge por perfil; daemon aberto ocupa o
  perfil).
- Desktop oculto atual: `~/.claude/skills/nodriver-isolado/files/edge_isolado.py`.
- GitHub por SSH falha neste shell (sem chave); clone e push por HTTPS com
  `credential.helper=!gh auth git-credential`.

### Código existente a aproveitar

- `~/nodriver/lib.py`: `start()`, `js()`, `close()` por `Browser.close`.
- `~/nodriver/live.py`: ações em JSON (`js`, `type`, `click`, `clicktext`, `file`, `clickxy`,
  `enter`, `sleep`, `dump`) e descoberta da porta pelo processo do Edge.
- Scripts com receitas: `iframe_file`, `blobcap`, `formsave`, `fetchsave`, `clickcapture`,
  `printpdf`, `viaunica`, `*_apply.py`.

### Referências

- Hermes Agent, browser: https://hermes-agent.nousresearch.com/docs/user-guide/features/browser
- Playwright MCP, snapshots de acessibilidade: https://www.mintlify.com/microsoft/playwright-mcp/concepts/accessibility-snapshots
- workflow-use (browser-use), record/replay: https://www.sourcepulse.org/projects/2513677
- TERX, replay de CDP: https://pypi.org/project/terx/0.3.1/
- taprun: https://taprun.dev/integrations/browser-use.html
- Record & Replay para agentes (arXiv 2505.17716): https://arxiv.org/pdf/2505.17716
- Internal APIs Are All You Need (arXiv 2604.00694): https://arxiv.org/pdf/2604.00694
- Skim, execução especulativa (arXiv 2605.16565): https://arxiv.org/pdf/2605.16565
- IETF draft AI Manifest: https://www.ietf.org/archive/id/draft-han-ai-manifest-02.html
- Browserless, batching de ações: https://docs.browserless.io/mcp/browser-agent
