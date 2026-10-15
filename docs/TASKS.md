# Tasks

## 0. Estrutura

- [x] Fork `ultrafunkamsterdam/nodriver` → `arthurbolsoni/nodriver-rs` (público), clone em `~/desktop/nd-projeto/nodriver-rs`, remote `upstream`
- [x] Projeto `nd` em `~/desktop/nd-projeto/nd` (Cargo, git)
- [x] Repo privado `arthurbolsoni/nd`
- [x] Decidido: desktop oculto opcional (`nd start --oculto`)
- [x] Decidido: atlas em `~/nodriver/sites/`
- [x] Decidido: aceitação no Facebook

## 1. Biblioteca `nodriver-rs`

Commit `e6f836a` no fork.

- [x] Workspace Cargo na raiz do fork; crates `nodriver`, `nodriver-cdp` e `cdp-gen`
- [x] Gerador de tipos CDP a partir de `browser_protocol.json` / `js_protocol.json` (equivalente ao `generate_cdp.py`): Browser, Target, Page, Runtime, DOM, Input, Network, Emulation, Fetch, Storage, Security, Log, IO e os referenciados (Debugger)
- [x] `connection`: websocket, ids, respostas, eventos tipados e crus por sessão, timeout
- [x] `connection`: reconexão automática (commit `3346916` no fork)
- [x] `config`: flags do `config.py`, perfil, idioma, porta, headless, executável (Chrome, Edge como alternativa)
- [x] `browser`: lançar, porta por `DevToolsActivePort`, conectar em navegador já aberto, targets, `get`, `close`, perfil em uso, pid do navegador
- [x] `tab`: `evaluate` (await_promise, return_by_value), `select` com espera, `find` por texto (com shadow DOM), espera de carregamento, `activate`, teclas, mouse, screenshot (testado: PNG no tamanho do viewport)
- [x] `element`: caixa, `scroll_into_view`, `mouse_click`, `click`, `send_keys`, `clear_input`, `send_file`, `select_option`, `apply`
- [x] Testes contra páginas locais (13 testes de integração, Edge headless, perfil temporário)
- [x] README do fork descrevendo o crate

## 2. `nd` base

- [x] Dependência `nodriver` por path
- [x] CLI (clap): `start`, `stop`, `status`, `daemon`, `open`, `tabs`, `tab`, `eval`, `text`
- [x] Daemon HTTP em `127.0.0.1` com token; estado em `~/.nd/daemon.json` (`ND_DIR` muda o diretório)
- [x] Desktop oculto opcional (`--oculto`): porte do `edge_isolado.py` (CreateDesktopW, foco emulado, perfil ocupado)
- [x] Espera pós-navegação: rede ociosa (500 ms sem requisição) + DOM parado (300 ms), teto de 10 s
- [x] Abas novas abertas pela página são acompanhadas (Target.setDiscoverTargets); daemon religa domínios depois de reconexão
- [x] Daemon encerra sozinho quando o Edge morre
- [x] `fechar.ps1` encerra o daemon (`nd stop`, depois taskkill)
- [x] Conferência de detecção nos sites que passam com o nodriver Python
- [x] Testes de ponta a ponta (`cargo test`: 3 de integração + 3 de unidade)

## 3. Map + ações

- [x] `nd.js` (mundo isolado): interativos, nomes, estado, forms, diálogos, títulos, avisos,
  shadow DOM, iframes do mesmo domínio, corte em 300 linhas com recorte na área visível
- [x] Refs em registro na página (em vez de `data-nd`) + localizador de reserva no daemon
- [x] `click`, `fill`, `pick`, `check`, `upload`, `key`, `wait`; alvo por ref ou texto
- [x] Diff pós-ação (elementos, avisos, texto novo, página nova)
- [x] Iframe de outro domínio por target CDP (ou mundo isolado no frame)
- [x] `nd open` com mapa; espera por commit + DCL (3 s) + rede do site + DOM parado
- [x] Teclas com modificador na biblioteca (`Control+a`, `Shift+Tab`)
- [x] Testes: `tests/acoes.rs` (página `form.html`) e unitários de recorte, diff e rede

## 4. Rede

- [x] Network em todas as abas, buffer 500, corpos até 200 KB (soma limitada a 40 MB)
- [x] Filtro de imagem/fonte/anúncio/métricas/log
- [x] `nd net` com forma do JSON, operação GraphQL, `--corpo`, `--caminho`, `--saida`; página
  atual por padrão
- [x] `nd api` via `fetch` na página: `--json`, `--set`, `--query`, `--metodo`, `--corpo`, `--saida`
- [x] Resumo da rede na saída das ações
- [x] Biblioteca: `attach` serializado (duas sessões no mesmo alvo quebravam `getResponseBody`)
- [x] Testes: `tests/rede.rs` (página `rede.html`, APIs no servidor de teste) e unitários de forma,
  JSON, caminho, form e corpo alterado

## 5. Receitas

- [x] Levantar dos scripts do `~/nodriver`: `iframe_file`, `blobcap`, `formsave`, `fetchsave`, `clickcapture`, `printpdf`, `viaunica`, `live.py`, `*_apply.py` (equivalências em ANOTACOES)
- [x] Diálogos automáticos (aceitos pelo daemon, aparecem na saída; feito na fase 3)
- [x] Downloads sempre ligados (`~/.nd/downloads`), mostrados na saída das ações; navegação que
  virou download marcada na rede
- [x] `nd download <alvo>`: download, resposta-arquivo na mesma aba ou em aba nova (GET/POST, pelo
  `Fetch` no alvo do navegador), blob em aba, blob baixado, blob só na página
- [x] `nd download --url`, lista; `nd pdf`
- [x] Abas abertas pela ação na saída (`aba nova [n]`)
- [x] `pick` em lista virtualizada com campo de busca próprio
- [x] Testes: `tests/arquivos.rs` (página `arquivos.html`), caso novo em `tests/acoes.rs`

## 6. Atlas + replay

- [x] Formato YAML + serde (`serde_yaml_ng`), atlas em `~/nodriver/sites/` (`ND_ATLAS` muda)
- [x] `nd atlas` (lista), `nd atlas <site>`, `nd atlas --rascunho` (passos da sessão pelo nome)
- [x] `nd run` com parâmetros, `--ate`, `--desde`, `confirmar` + `--confirmado`, `opcional`,
  checagem de login, `--seco`, parada com map + net e comando para retomar no passo que falhou
- [x] Testes: `tests/atlas.rs` e unitários de passo, parâmetros e pedidos

## 7. Skill e regra

- [x] `~/.claude/skills/navegar/SKILL.md`
- [x] Linha na seção nodriver do `~/.claude/CLAUDE.md`
- [x] `nd` no PATH (`cargo install --path .` → `~/.cargo/bin/nd.exe`)

## 8. Aceitação

- [ ] Tarefa do zero com `nd` no Facebook
- [ ] Atlas gravado, repetição com `nd run`
- [ ] Contagem de chamadas de ferramenta: método atual × `nd`
