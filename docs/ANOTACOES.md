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

## 2026-10-15 — fase 1 (biblioteca)

### Comportamentos do Edge encontrados

- `msedge.exe` sempre se relança: o processo iniciado sai com código 0 e o navegador roda como
  filho dele, mesmo com perfil novo. Saída com código 0 não indica perfil em uso.
- Perfil em uso é detectado antes do lançamento: no Windows o `lockfile` do perfil não pode ser
  apagado enquanto o navegador roda. No Linux/macOS, `SingletonLock` aponta para `<host>-<pid>`.
- O pid real do navegador vem de `SystemInfo.getProcessInfo` (processo do tipo `browser`).
- O endpoint HTTP do DevTools (`/json/version`) não fecha a conexão depois da resposta; o corpo é
  lido pelo `Content-Length`, que vem sem espaço depois dos dois pontos.
- Porta de debug 0: o navegador escolhe e grava em `DevToolsActivePort` (linha 1 porta, linha 2
  caminho do websocket).

### Decisões da biblioteca

- Lib pública em inglês (código, comentários, erros); docs do `nd` em português.
- Uma conexão websocket por navegador; abas por sessões `flatten`.
- `Page.enable` só antes da primeira navegação da aba; `Runtime.enable` nunca.
- `find` por texto em JS (`crates/nodriver/src/js/find.js`): nós de texto e atributos
  `aria-label`, `placeholder`, `title`, `value`, `alt`, incluindo shadow DOM aberto.
- Testes: `cargo test -p nodriver` (13 testes, ~20 s em paralelo). `NODRIVER_HEADFUL=1` mostra
  as janelas.

## 2026-10-15 — reconexão e screenshot

- Websocket caído com o navegador vivo: a conexão reabre em segundo plano (até 3 tentativas)
  e anexa sessão nova a cada aba registrada. `Tab` busca a sessão atual na conexão, então a
  mesma `Tab` continua valendo.
- Inscrições por aba e as globais continuam recebendo eventos; inscrição presa a um session id
  cru para de receber. Evento sintético `Nodriver.reconnected` com `{generation}`.
- Comando em andamento na queda volta `Disconnected` e não é reenviado (o navegador pode ter
  executado). `Page.enable` é reenviado na sessão nova; outros domínios ativados pelo chamador
  (ex.: `Network.enable`) precisam ser ativados de novo. O daemon do `nd` deve escutar
  `Nodriver.reconnected` e reativar `Network` para não perder a captura de rede.
- Navegador morto: as tentativas falham, os streams terminam e as chamadas voltam
  `Disconnected`.
- `Browser::close` desliga a reconexão antes de fechar.
- Testes: 13 (browser) + 3 (reconexão) + 1 (screenshot), estáveis em 3 rodadas paralelas.

## 2026-10-15 — fase 2 (`nd` base)

### Achados

- `--remote-debugging-port=0` faz o Edge 155 reportar `navigator.webdriver === true`. Com porta
  fixa fica `false`. A lib passou a escolher uma porta livre antes de lançar (como o
  `util.free_port` do nodriver) e a esperar o `/json/version` (commit `f2f87ea` no fork). Com porta
  fixa o Edge não grava `DevToolsActivePort`.
- O Windows faz o processo filho herdar todo handle herdável. O daemon herdava o pipe do terminal
  (`nd start | sed`, o Bash do Claude Code) e quem lia a saída ficava preso até o daemon morrer.
  O `nd start` marca stdin/stdout/stderr como não herdáveis antes de criar o daemon.
- O daemon é criado com `CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW` e, quando o job permite,
  `CREATE_BREAKAWAY_FROM_JOB`; sobrevive entre chamadas do Bash do Claude Code.
- Latência de um comando simples (`nd eval 1`, build release): ~60–70 ms.

### Conferência de detecção (perfil temporário, desktop oculto)

| Site | `nd` | nodriver Python |
|---|---|---|
| bot.sannysoft.com | 58 linhas, 0 falhas/avisos | 0 falhas/avisos |
| browserscan.net/bot-detection | Test Results: Normal (CDP Normal, webdriver false) | Test Results: Normal |
| nowsecure.nl (Cloudflare) | conteúdo da página ("NOWSECURE BY NODRIVER") | conteúdo da página |
| `navigator.webdriver` | false | False |

### LinkedIn com `~/nodriver/profile-edge`

- `nd` (oculto, pt-BR) em `/feed/`: redireciona para `/checkpoint/challenge/...` ("Let's do a quick
  security check", com "Sign in / Join now").
- Logo depois, `li_check_login.py` (nodriver Python, oculto, mesmo perfil): mesmo checkpoint
  ("Vamos fazer uma verificação rápida de segurança").
- A verificação precisa de ação humana no Edge visível. Não se sabe se a visita do `nd` a disparou
  ou se já estava pendente.
- Esclarecido pelo Arthur: o LinkedIn entrava sozinho pelo login do Google (Gmail), numa conta
  que não é a dele. A restrição ("O acesso à sua conta foi temporariamente restrito") era dessa
  conta. Com o Edge visível, a sessão passou a ser a conta `arthurxavierbolsoni` e o `/feed/`
  carrega sem verificação. O perfil tem o cookie `g_state` (Google One Tap).
- No atlas do LinkedIn (fase 6): não usar "Continuar com Google"; conferir a conta ativa pelo
  link `/in/arthurxavierbolsoni/` no feed.
