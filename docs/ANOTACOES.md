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

## 2026-10-15 — fase 3 (map + ações)

### Decisões

- A biblioteca da página (`src/js/nd.js`) roda num mundo isolado por quadro
  (`Page.createIsolatedWorld`, nome `nd`). A página não vê as funções nem as refs, e protótipos
  alterados pelo site não afetam o mapa. `nd eval` continua no mundo da página.
- Refs ficam num registro em memória do mundo isolado (ref → `WeakRef` do elemento), em vez do
  atributo `data-nd` do plano. Atributo no DOM é visível ao site, dispara `MutationObserver` dele
  e some junto com o nó no re-render do mesmo jeito.
- Localizador de reserva no daemon: papel + nome + posição entre iguais. Ref de nó trocado pelo
  re-render é achada de novo; ref de documento anterior dá "ref de uma página anterior".
- Refs recomeçam em `e1` quando o documento principal muda (`performance.timeOrigin`).
- Iframe do mesmo domínio é lido por dentro (`contentDocument`). Iframe de outro domínio: o
  daemon pega o `frameId` do elemento (`DOM.describeNode`); se há target `iframe` com esse id
  (outro processo), usa a sessão dele, senão cria o mundo isolado no frame pela sessão da aba.
  Até 10 iframes e 3 níveis por mapa.
- Clique: ponto calculado no quadro (com `elementFromPoint` em vários pontos da caixa) mais a
  origem de cada iframe até o principal; mouse via `Input.dispatchMouseEvent` na aba. Elemento
  coberto dá erro com a ref e o texto de quem cobre; `--js` usa `el.click()`.
- `fill`: foco, seleciona o conteúdo, `Input.insertText` (ou tecla por tecla com `--teclas`);
  confere o valor e, se a digitação não pegou, usa o setter nativo + `input`/`change`. Num iframe
  de outro domínio clica antes, para o foco do teclado ir para o quadro.
- `pick`: `<select>` nativo por texto ou value; senão digita (campo) ou clica (botão) e espera até
  5 s por uma opção visível (`role=option`, `menuitem`, `li` de `listbox`/`menu`).
- `upload` em botão/label: `Page.setInterceptFileChooserDialog` + clique + `fileChooserOpened` →
  `DOM.setFileInputFiles` pelo `backendNodeId`.
- Diálogos (`alert`, `confirm`, `prompt`, `beforeunload`) são aceitos pelo daemon e aparecem na
  saída do comando. O daemon liga `Page.enable` em toda aba.
- `nd open` imprime o mapa (cabeçalho, linha de espera, elementos); `--sem-map` só título, URL e
  espera.
- Diff pós-ação: URL, título, avisos novos, elementos novos (`+`), sumidos (`-`), mudados (`~`) e
  linhas de texto visível novas (inclui shadow DOM e iframes). Nó trocado pelo re-render com a
  mesma descrição não entra. Documento novo → "página nova" + mapa.
- Mapa: até 300 linhas. Página maior → recorte na área visível (uma tela acima, duas abaixo) com
  a contagem do que ficou acima e abaixo. Diálogo modal aberto → só ele.
- Teclas com modificador (`Control+a`, `Shift+Tab`, `Alt+ArrowLeft`) e F1–F12 no `press_key` da
  biblioteca (commit no fork).

### Espera depois de navegar ou agir

- Navegação: espera o commit (`Page.frameNavigated` do frame principal com o `loaderId`) e o
  `DOMContentLoaded` por até 3 s. O Marketplace do Facebook manda o HTML em streaming e o
  `DOMContentLoaded` vem aos ~10 s, com a página já utilizável.
- Rede ociosa (500 ms, contados a partir da ação): documento principal até a resposta começar;
  script e CSS de qualquer origem; XHR/fetch só do mesmo site (domínio registrável). Fora:
  imagem, fonte, mídia, beacon, EventSource/WebSocket, `blob:`/`data:`, hosts e caminhos de
  anúncio e métricas, documento de iframe, requisição aberta há mais de 5 s.
- DOM parado: 300 ms sem mudança de estrutura ou texto (atributos ficam de fora: carrossel e
  animação mudam atributo o tempo todo), até 3 s.
- Medido (release, headless, perfil temporário, pt-BR): Wikipedia 1,4 s; Facebook 2,1 s;
  Marketplace deslogado 4,5 s no total; GitHub 1,6 s (5 s na primeira carga a frio); g1 4–8 s.
  No g1 a rede do site termina em ~2 s e o `Runtime.evaluate` do teste de DOM parado só volta
  seis segundos depois: a thread da página está ocupada pelos scripts de anúncio.
- `nd map` sozinho: 55–75 ms nessas páginas.

### Outros achados

- O documento de um iframe de outro processo começa na sessão da aba (`requestWillBeSent`) e
  termina na sessão do target dele; contado, segurava a espera até o teto.
- Aba criada pelo `open --nova` podia receber o `Page.navigate` antes do `Page.enable` do
  preparo (que corre em paralelo com a descoberta de alvos) e o commit não chegava; o `open` liga
  o `Page` antes de navegar.
- `innerText` do body não inclui o conteúdo de shadow DOM; o texto do diff junta o de cada shadow
  root aberto.
- `checkVisibility()` dá falso para `display: contents` (sem caixa); o mapa desce nos filhos.
- `www.mercadolivre.com.br` no headless com perfil temporário: página de erro em espanhol
  ("Hubo un error accediendo a esta pagina").
- Uma execução da suíte inteira teve `nd open` do `lento.html` falhando no `fluxo_completo`
  (stderr não guardado); não repetiu em sete execuções seguidas.

## 2026-10-15 — fase 4 (rede)

### Decisões

- Buffer único das últimas 500 chamadas XHR/fetch/documento de todas as abas, numeradas (`#12`)
  para `nd net --corpo` e `nd api`. Guarda método, URL, status, mime, cabeçalhos enviados, corpo
  enviado e resposta em texto até 200 KB cada; soma dos corpos limitada a 40 MB (os mais antigos
  saem primeiro). Resposta binária fica marcada, sem corpo.
- Respostas pedidas com `Network.getResponseBody` logo depois do `loadingFinished`, só para mime
  de texto (JSON, texto, XML, JS, form). Corpo enviado vem do `requestWillBeSent` ou de
  `Network.getRequestPostData` quando não veio junto.
- Fora do buffer: anúncio, métricas e log (mesmo filtro da espera de rede, mais `/ajax/bz` do
  Facebook). Imagem, fonte e mídia não entram (só XHR, fetch e documento).
- `nd net` mostra a página atual da aba (desde o último documento principal); `--antigas`,
  `--todas`, `--novas` (desde a última ação), filtro por trecho de URL, método ou nome da
  operação.
- Forma do JSON: chaves e tipos na ordem original (`serde_json` com `preserve_order`), listas
  com o tamanho e as chaves dos primeiros 20 itens juntas. Aceita `for (;;);` / `)]}'` na frente
  e vários JSON em sequência (NDJSON).
- Nome da operação na linha: `operationName` (JSON), `fb_api_req_friendly_name` ou
  `operationName` (form ou URL).
- `--caminho data.itens[].nome` seleciona parte da resposta (em `nd net --corpo` e `nd api`).
- `nd api N` refaz a chamada por `fetch` no mundo isolado da aba da chamada: mesma origem e
  cookies da página (conferido pelo eco do cookie e do cabeçalho `x-token` no teste). Copia os
  cabeçalhos enviados, menos os que o `fetch` não aceita (`cookie`, `origin`, `referer`,
  `sec-*`...). `--json` funde no corpo; `--set variables.count=20` muda um valor (no form, o
  primeiro trecho é o campo e o resto vai dentro do JSON do campo); `--query pagina=2` muda a URL;
  `--saida` grava os bytes da resposta. A chamada refeita também é capturada e a saída diz o
  número dela.
- Ações listam as chamadas feitas durante a ação (`rede (n):`, até 6) antes do diff.
- `nd cdp <método> [json]` (oculto): CDP cru na aba atual, para depuração.

### Corrida no `attach` da biblioteca

- `Network.getResponseBody` dava "No resource with given identifier found" para toda resposta no
  `nd`, e funcionava no teste da biblioteca. Depois de `Network.disable` + `Network.enable` pela
  sessão atual, passava a funcionar.
- Causa: duas chamadas simultâneas de `Browser::tab` para o mesmo alvo (o laço de início do daemon
  e a descoberta de alvos; ou `open --nova` e a descoberta) faziam dois `Target.attachToTarget`.
  O alvo ficava com duas sessões: o `Network.enable` ia para uma, e o `getResponseBody` para a
  outra, que não guardava corpos.
- `Connection::attach` agora é serializado por um mutex (commit no fork), com teste de
  `attach` concorrente e de corpo depois de navegação.

### Limites

- A rede de iframe de outro processo (target próprio) não é capturada; só a das abas.
- Documento em streaming (Facebook) fica "em andamento" no `nd net` até o HTML terminar.

## 2026-10-15 — fase 5 (receitas)

### Scripts do `~/nodriver` e o equivalente no `nd`

| Script | Faz | No `nd` |
|---|---|---|
| `iframe_file.py` | arquivo em input dentro de iframe | `nd upload` (iframe do mesmo domínio e de outro, input escondido, botão que abre o seletor) |
| `blobcap.py`, `blobcap2.py` | clica e captura blob/`window.open`/`a.click` | `nd download <alvo>` |
| `formsave.py`, `viaunica.py` | envia form por fetch e grava a resposta | `nd download <botão>` (resposta do POST pega pelo `Fetch`) |
| `fetchsave.py`, `jssave.py` | baixa URL com a sessão da aba | `nd download --url`, `nd api N --saida` |
| `clickcapture.py` | clica e lista rede e abas novas | saída das ações (`rede (n)`, `aba nova [n]`, `download:`) |
| `printpdf.py` | aba em PDF | `nd pdf` |
| `live.py` | ações por JSON (`type`, `click`, `file`, `dump`...) | `fill`, `click`, `upload`, `map`; `clickxy` não entrou (o mapa lê o iframe do reCAPTCHA) |
| `gh_apply.py` | foco por JS, confere o valor digitado | `fill` confere e define por JS se a digitação não pegou |
| `lever_apply.py` | local digitado letra a letra | `fill --teclas` |
| `inhire_apply.py` | dropdown virtualizado com campo "Pesquisar"; idioma muda rótulos e DDI | `pick` digita na busca da lista; `nd start --lang pt-BR` |
| `deel_apply.py`, `ashby_apply.py` | combobox MUI, Sim/Não com `aria-pressed`, upload que refaz o form | `pick`; estado `[pressionado]` no mapa; diff mostra campos que mudaram |
| `*.go` / `*.stop` | segura antes de enviar | passo `confirmar` do atlas (fase 6) |

### Decisões

- Downloads ligados no início do daemon: `Browser.setDownloadBehavior` (`allowAndName`, eventos)
  na pasta `~/.nd/downloads`. O arquivo chega com o guid como nome e é renomeado para o nome
  sugerido (`nome (2).ext` se já existe). Religado depois de reconexão.
- A ação espera downloads iniciados por ela até 15 s e mostra `download: <caminho>`; a navegação
  que virou download aparece na rede como `→ download` (o navegador cancela o documento com
  `ERR_ABORTED`).
- `nd download <alvo>` liga `Fetch` no alvo do navegador (sem sessão) só durante o comando, para
  respostas de documento em qualquer aba, inclusive a aba nova que o clique abre: resposta 2xx que
  não é HTML e não é anexo tem o corpo gravado (`Fetch.getResponseBody`) e segue. Anexo fica para o
  download do navegador.
- Blob: antes do clique, `URL.createObjectURL` ganha um `Proxy` no mundo da página que guarda os
  blobs num `Map`. Aba nova com URL `blob:` é lida desse `Map` (a página pode já ter revogado a
  URL), em pedaços de 4 MB. Blob criado sem download nem aba (mostrado num iframe) vira o arquivo
  depois de 2 s.
- `nd download --url` usa o `fetch` do `nd api` (mundo isolado, `credentials: include`); nome pelo
  `Content-Disposition` ou pela URL.

### Achados

- No Edge com janela, download para pasta com `/` e `\` misturados (`ND_DIR` vindo do Git Bash)
  termina como "Couldn't download - Download error"; no headless funciona. `estado::dir()` passou
  a normalizar o caminho com `std::path::absolute`.
- No Edge headless, PDF abre no visualizador (não vira download) e o download comum abre a aba
  `edge://downloads-hub/`, que fica fora de `aba nova`.
- `Fetch.enable` no alvo do navegador pega a resposta de documento de todas as abas, inclusive
  POST com `target=_blank`; o corpo vem pelo `Fetch.getResponseBody` também quando o visualizador
  de PDF abre.
- O filtro de evento da biblioteca casa nome exato ou prefixo terminado em ponto: `"Browser."`, não
  `"Browser.download"`.
- `<input name="id">` dentro de um form faz `form.id` devolver o input (DOM clobbering) e quebrava
  o mapa; o `nd.js` lê `getAttribute('id')`, e o título protege `document.title` do mesmo caso.
- Conferido em site real: link de PDF do web.archive.org (PDF dentro do iframe da barra do
  arquivo) gravado por `nd download` no Edge com janela.

## 2026-10-15 — fase 6 (atlas + replay)

### Decisões

- `nd run` roda no cliente: lê o YAML, troca os parâmetros e manda cada passo ao daemon como o
  comando equivalente. O replay usa as mesmas ações, esperas e diffs dos comandos avulsos.
- YAML por `serde_yaml_ng` (o `serde_yaml` foi descontinuado), convertido para JSON com a ordem
  das chaves mantida.
- `{nome}` é trocado só quando `nome` é parâmetro conhecido; `{x}` sem declarar fora de passo
  `eval` conta como parâmetro esquecido e o fluxo não começa. JavaScript (`{a}` em
  desestruturação) fica fora dessa busca.
- O alvo `Alvo` guarda o nome do elemento; o histórico grava o texto dado ou, quando a ação foi
  por ref, o nome do elemento da ref. `open`, `tab` e `wait` também entram no histórico.
- Saída do replay: cabeçalho `[n/total]` e até 8 linhas por passo (`--tudo` mostra tudo; `text` e
  `eval` saem inteiros). `open` no replay não imprime o mapa.
- Passo com `confirmar` para o replay com saída de sucesso e o comando de retomada; o agente pede
  o ok ao usuário e roda `--desde N --confirmado`.
