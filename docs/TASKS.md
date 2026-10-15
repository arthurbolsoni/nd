# Tasks

## 0. Estrutura

- [x] Fork `ultrafunkamsterdam/nodriver` → `arthurbolsoni/nodriver-rs` (público), clone em `~/desktop/nodriver-rs`, remote `upstream`
- [x] Projeto `nd` em `~/desktop/nd` (Cargo, git)
- [x] Repo privado `arthurbolsoni/nd`
- [ ] Decidir: desktop oculto como padrão
- [ ] Decidir: local do atlas (`~/nodriver/sites/` ou dentro de `~/.claude`)
- [ ] Decidir: site do teste de aceitação

## 1. Biblioteca `nodriver-rs`

- [ ] Workspace Cargo na raiz do fork; crates `nodriver` e `nodriver-cdp`
- [ ] Gerador de tipos CDP a partir de `browser_protocol.json` / `js_protocol.json` (equivalente ao `generate_cdp.py`), só os domínios usados: Browser, Target, Page, Runtime, DOM, Input, Network, Emulation, Fetch
- [ ] `connection`: websocket, ids, respostas, eventos por domínio, timeout, reconexão
- [ ] `config`: flags do `config.py`, perfil, idioma, porta, headless, executável (Chrome/Edge)
- [ ] `browser`: lançar, ler porta de debug, conectar em navegador já aberto (host/porta), targets, `get`, `close`
- [ ] `tab`: `evaluate` (await_promise, return_by_value), `select` com espera, `find` por texto, espera de carregamento, `activate`
- [ ] `element`: caixa, `scroll_into_view`, `mouse_click`, `send_keys`, `clear_input`, `send_file`
- [ ] Testes contra páginas locais
- [ ] README do fork descrevendo o crate

## 2. `nd` base

- [ ] Dependência `nodriver` por path
- [ ] CLI (clap): `start`, `stop`, `daemon`, `open`, `tabs`, `tab`, `eval`, `text`
- [ ] Daemon HTTP em `127.0.0.1` + `.token`
- [ ] Desktop oculto: porte do `edge_isolado.py` (CreateDesktopW, foco emulado, perfil ocupado, `--visivel`)
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

- [ ] Tarefa do zero com `nd` no site escolhido
- [ ] Atlas gravado, repetição com `nd run`
- [ ] Contagem de chamadas de ferramenta: método atual × `nd`
