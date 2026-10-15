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

- [ ] Dependência `nodriver` por path
- [ ] CLI (clap): `start`, `stop`, `daemon`, `open`, `tabs`, `tab`, `eval`, `text`
- [ ] Daemon HTTP em `127.0.0.1` + `.token`
- [ ] Desktop oculto: porte do `edge_isolado.py` (CreateDesktopW, foco emulado, perfil ocupado, `--oculto`)
- [ ] Espera pós-navegação: rede ociosa + DOM parado
- [ ] `fechar.ps1` encerra o daemon
- [ ] Conferência de detecção nos sites que passam com o nodriver Python

## 3. Map + ações

- [ ] `map.js`: interativos, nomes, estado, forms, títulos, shadow DOM, iframes, corte
- [ ] Refs `data-nd` + localizador de reserva no daemon
- [ ] `click`, `fill`, `pick`, `check`, `upload`, `key`, `wait`
- [ ] Diff pós-ação
- [ ] Iframe de outro domínio por target CDP

## 4. Rede

- [ ] Network em todas as abas, buffer 500, corpos JSON até 200 KB
- [ ] Filtro de imagem/fonte/analytics
- [ ] `nd net` com forma do JSON
- [ ] `nd api` via `fetch` na página

## 5. Receitas

- [ ] Levantar dos scripts do `~/nodriver`: `iframe_file`, `blobcap`, `formsave`, `fetchsave`, `clickcapture`, `printpdf`, `viaunica`, `live.py`, `*_apply.py`
- [ ] Diálogos automáticos
- [ ] `download` (blob), `pdf`

## 6. Atlas + replay

- [ ] Formato YAML + serde
- [ ] `nd atlas <site>`
- [ ] `nd run` com parâmetros, `--ate`, `confirmar`, parada com map + net no passo que falhou

## 7. Skill e regra

- [ ] `~/.claude/skills/navegar/SKILL.md`
- [ ] Linha na seção nodriver do `~/.claude/CLAUDE.md`

## 8. Aceitação

- [ ] Tarefa do zero com `nd` no Facebook
- [ ] Atlas gravado, repetição com `nd run`
- [ ] Contagem de chamadas de ferramenta: método atual × `nd`
