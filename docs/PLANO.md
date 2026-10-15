# nd — plano

Automação de navegador para o Claude Code: Edge controlado por CDP, com um daemon que mantém a
conexão aberta, um cliente de linha de comando curto, snapshot com refs, captura de rede, receitas
para armadilhas comuns, atlas por site e replay de fluxos.

## Repositórios

| Repo | Visibilidade | Pasta | Conteúdo |
|---|---|---|---|
| `arthurbolsoni/nodriver-rs` | público, fork de `ultrafunkamsterdam/nodriver` (AGPL-3.0) | `~/desktop/nd-projeto/nodriver-rs` | biblioteca Rust: porte do nodriver |
| `arthurbolsoni/nd` | privado | `~/desktop/nd-projeto/nd` | ferramenta `nd`: daemon, cliente, map, rede, atlas, replay, skill |

O `nd` depende da biblioteca por `path = "../nodriver-rs/crates/nodriver"` durante o
desenvolvimento e por `git = "https://github.com/arthurbolsoni/nodriver-rs"` depois.

## Biblioteca `nodriver-rs`

Porte da API pública do nodriver Python 0.50.x. O que ficou fora (inspetor externo, realce visual,
gravação de vídeo, OpenCV, proxy com credenciais, ...) está no README do crate, com o motivo.

Layout no fork: workspace Cargo na raiz, Python original mantido no lugar como referência até o
porte cobrir o que o `nd` usa.

```
Cargo.toml                 workspace
crates/nodriver-cdp/       tipos CDP gerados do browser_protocol.json / js_protocol.json
crates/nodriver/           Browser, Tab, Element, Config, Connection
nodriver/                  Python original (referência)
```

Módulos e equivalência com o Python:

| Rust | Python | Faz |
|---|---|---|
| `config` | `core/config.py` | flags de lançamento, perfil, idioma, headless, porta |
| `browser` | `core/browser.py` | lança Chrome/Edge, lê a porta de debug, lista e atualiza targets, `get(url, new_tab)`, `close()`, cookies, permissões, contexto com proxy |
| `connection` | `core/connection.py` | websocket CDP, ids de comando, respostas, assinatura de eventos por domínio |
| `tab` | `core/tab.py` | `evaluate`, `select(css)` com espera, `find(texto, best_match)`, espera de carregamento, `activate`, rolagem por gesto, histórico, janela, screenshot de página inteira, `localStorage`, `xpath`, arrastar |
| `element` | `core/element.py` | `mouse_click` no centro da caixa, `send_keys`, `clear_input`, `send_file`, `scroll_into_view`, arrastar, atributos, pai/filhos, screenshot do elemento |
| `nodriver-cdp` | `generate_cdp.py` + `cdp/` | geração de tipos e comandos a partir do protocolo (build script ou gerador versionado) |

Regras:

- Sem chromedriver e sem flags de automação; lista de flags igual à do `config.py`.
- Domínios CDP ativados só quando um comando precisa (sem `Runtime.enable` automático).
- tokio + tokio-tungstenite + serde/serde_json.
- Testes com `cargo test` contra páginas locais.

## Ferramenta `nd`

Um executável, `nd.exe`, em dois papéis:

- `nd daemon` — mantém o Edge e a conexão (via `nodriver-rs`), captura a rede de todas as abas e
  atende HTTP em `127.0.0.1` com token em `.token`.
- `nd <comando>` — cliente; manda o comando ao daemon e imprime a resposta.
- `nd start` dispara `nd daemon` em segundo plano; `nd stop` encerra daemon e Edge.

Partes do `nd`:

- Desktop oculto opcional (`CreateDesktopW` + `CreateProcessW`, crate `windows`), porte do
  `~/.claude/skills/nodriver-isolado/files/edge_isolado.py`, com emulação de foco e checagem de
  perfil ocupado. `nd start` abre o Edge no desktop do usuário; `nd start --oculto` abre no
  desktop oculto.
- JS injetado em arquivos `.js` (`include_str!`): snapshot, receitas, diff pós-ação.
- Atlas em YAML (serde).
- `~/nodriver/fechar.ps1` passa a encerrar o daemon.

### Comandos

| Comando | Faz |
|---|---|
| `nd start [--oculto]` / `nd stop` | sobe ou encerra daemon e Edge |
| `nd open <url> [--nova]` | navega e espera rede ociosa + DOM parado |
| `nd tabs` / `nd tab <n\|trecho>` | lista abas, escolhe a ativa |
| `nd map [--tudo] [--dentro <ref>]` | snapshot compacto com refs |
| `nd text [--sel X]` | `innerText` da página ou de um trecho |
| `nd click <ref\|"texto">` | clique por CDP no centro do elemento |
| `nd fill <ref> "valor"` | preenche campo, com tratamento de input controlado (React) |
| `nd pick <ref> "opção"` | select nativo, combobox, autocomplete |
| `nd check <ref>` / `nd upload <ref> <arq>` / `nd key <tecla>` | checkbox, upload, teclas |
| `nd scroll [--vezes N] [--fim] [--dentro <alvo>] [--subir]` | rola com o gesto de rolagem do navegador; mostra rede e itens novos; `--fim` até parar de crescer |
| `nd wait text\|sel\|url\|net <x>` | espera explícita |
| `nd eval '<js>'` / `nd eval -f arq.js` | JS livre |
| `nd net [filtro] [--corpo n]` | XHR/fetch capturados |
| `nd api <n> [--json '{...}']` | refaz chamada capturada via `fetch` na página |
| `nd download <alvo> [--pasta]` / `--url` | clica e grava o arquivo gerado; baixa URL com a sessão |
| `nd pdf <arq>` | impressão da aba em PDF |
| `nd atlas <site>` | mostra o atlas do site |
| `nd run <site> <fluxo> -p k=v [--ate N]` | replay de fluxo do atlas |

Ação que muda a página devolve um diff: URL nova, elementos que apareceram ou sumiram, toast ou
erro visível.

### Snapshot (`nd map`)

- Elementos interativos visíveis: button, a, input, select, textarea, `[role=…]`,
  `[contenteditable]`, elementos com `onclick` ou `cursor:pointer`.
- Nome: label, aria-label, placeholder, texto, title. Estado: valor, checked, disabled, expanded,
  obrigatório.
- Formulários agrupados, títulos h1–h3 como âncora.
- Shadow DOM aberto; iframe do mesmo domínio por JS, de outro domínio pelo target CDP do frame.
- Padrão: viewport e entorno. `--tudo`: página inteira. Corte em ~300 linhas com indicação do
  restante.

```
# Candidatura — jobs.site.com/apply/123
form "Dados pessoais"
  e3  input[text]  "Nome*"          = ""
  e4  input[email] "E-mail*"        = "arthur@…"
  e5  combobox     "Estado"         = "Selecione"
e12 button "Enviar candidatura"  [disabled]
iframe#2 (outro domínio: forms.x.com) — 4 elementos → nd map --dentro iframe#2
```

Refs: registro em memória no mundo isolado da página (ref → elemento) e localizador de reserva no
daemon (papel + nome + posição entre iguais). Ref perdida por re-render é resolvida pelo
localizador; falha nos dois devolve "ref velha, rode nd map". O plano inicial usava o atributo
`data-nd`; a troca está nas anotações da fase 3.

### Rede

- Domínio `Network` em todas as abas; buffer das últimas 500 requisições XHR/fetch/documento.
- Guarda método, URL, status, tipo, corpo enviado, corpo da resposta JSON até 200 KB.
- `nd net`: uma linha por chamada com a forma do JSON (chaves e tipos).
- `nd api`: `fetch` de dentro da página com `credentials: 'include'`; origem, cookies e CSRF
  ficam os do site.
- Fora da lista: imagem, fonte, analytics (lista de domínios e tipos).

### Receitas

Levantadas dos scripts do `~/nodriver` (`iframe_file`, `blobcap`, `formsave`, `fetchsave`,
`clickcapture`, `printpdf`, `viaunica`, `live.py`, `*_apply.py`):

- `fill`: foco + clique, limpa, `Input.insertText`; se o valor não ficar, setter nativo + eventos
  `input`/`change`; confere o valor lido.
- `pick`: select nativo direto; combobox = clicar, digitar, esperar a lista, escolher pelo texto;
  lista que abre com campo de busca focado recebe o texto (lista virtualizada); opção inexistente
  devolve as opções disponíveis.
- `upload`: `DOM.setFileInputFiles` (iframe e input escondido).
- Diálogos `alert`/`confirm` aceitos e registrados no diff.
- Espera pós-ação: rede ociosa + DOM parado por 300 ms, teto de 10 s.
- Downloads sempre ligados, na saída da ação; abas abertas pela ação na saída.
- `nd download <alvo>`: download, resposta-arquivo na mesma aba ou em aba nova (GET ou POST),
  blob em aba, blob baixado, blob só na página. `--url` baixa com a sessão da aba.
- `nd pdf`: `Page.printToPDF` com fundo e tamanho de página do CSS.

### Atlas

`~/nodriver/sites/<dominio>.yaml`:

```yaml
site: jobs.ashbyhq.com
atualizado: 2026-10-15
login:
  checar: "!!document.querySelector('[data-testid=avatar]')"
  url: https://...
paginas:
  candidatura: { url: "/{empresa}/{vaga}/application", obs: "form em 2 etapas" }
api:
  - { metodo: POST, url: "/api/non-user-graphql?op=ApiJobPosting", uso: "dados da vaga" }
armadilhas:
  - "Upload do CV dentro de iframe de outro domínio"
fluxos:
  candidatar:
    login: false
    params: [url, cv]          # ou mapa nome: padrão
    passos:
      - open: "{url}"
      - click: Aceitar cookies
        opcional: true
      - fill: { alvo: "Nome", valor: "Arthur Bolsoni" }
      - upload: { alvo: "Resume", arquivo: "{cv}" }
      - click: { alvo: "Submit Application", confirmar: true }
      - esperar: { texto: "Thanks for applying" }
```

- Passos por nome (o texto que o `nd` aceita como alvo), não por ref; `nome` vale como `alvo`.
- Ações: `open`, `click`, `fill`, `pick`, `check`, `upload`, `key`, `scroll`, `wait`/`esperar`, `download`,
  `eval`, `text`, `tab`, `pdf`, com os mesmos campos das opções da linha de comando.
- `scroll: 3` (vezes), `scroll: fim` ou mapa `{vezes, fim, dentro, subir}`; `vezes` aceita `"{param}"`.
- `opcional: true`: falha não para o fluxo. `confirmar: true`: o replay para antes do passo.
- `open` com caminho (`/x`) vai para `https://<site>/x`.
- `login: true` no fluxo: depois do primeiro `open`, `login.checar` precisa dar `true`.
- Sem cookie, token, senha ou dado pessoal além do que o fluxo usa; o resto vira `{param}`.

### Replay

`nd run <site> <fluxo> -p k=v`: executa os passos sem LLM. Falha num passo para ali e imprime
número do passo, erro, `nd map` e `nd net` recente e o comando para retomar. Passo com
`confirmar: true` para o replay antes dele; `--desde N --confirmado` segue depois do ok do
usuário. `--ate N` limita até o passo N; `--seco` mostra os passos com os parâmetros trocados.

`nd atlas --rascunho`: os passos feitos na sessão do daemon, em YAML, com o alvo pelo nome
(ref usada vira o nome do elemento), para virar fluxo.

### Skill e regra

- Skill `browser` (`~/.claude/skills/browser/SKILL.md`):
  1. `nd atlas <site>`; com fluxo, `nd run`.
  2. Sem atlas: só leitura → `nd text`; interação → `nd open`, `nd map`, `nd net`.
  3. Dado na rede → `nd api`.
  4. Ações por `nd`, sem arquivo de sondagem.
  5. No fim, gravar ou atualizar o atlas e `nd stop`.
- Uma linha na seção nodriver do `~/.claude/CLAUDE.md` apontando para a skill.
- `nodriver-isolado`, `linkedin-perfil` e `strider-perfil` seguem como estão.

## Testes

- Páginas em `tests/fixtures/` servidas localmente: form React controlado, combobox com
  autocomplete, iframe de outro domínio (outra porta) com upload, shadow DOM, SPA com fetch, botão
  que troca o nó no re-render, `alert`.
- Biblioteca: `cargo test` de lançamento, conexão, evaluate, select/find, clique, digitação,
  upload, abas.
- `nd`: cada comando, ref após re-render, diff pós-ação, rede, `nd api`, replay com falha no passo
  certo.
- Detecção: abrir pelo `nd` os sites que hoje passam com o nodriver Python (LinkedIn, um com
  Cloudflare) e comparar.
- Aceitação no Facebook: tarefa do zero com `nd`, atlas gravado, repetição com `nd run`;
  comparação do número de chamadas de ferramenta com o método atual.

## Fases

Detalhe em [TASKS.md](TASKS.md).

1. Biblioteca: conexão, lançamento, tipos CDP, Tab/Element.
2. `nd` base: daemon + cliente, desktop oculto, `open`/`tabs`/`eval`/`text`, `fechar.ps1`,
   conferência de detecção.
3. `map` + refs + ações + diff.
4. Rede: `net`, `api`.
5. Receitas restantes.
6. Atlas + `run`.
7. Skill `browser` + regra no CLAUDE.md.
8. Aceitação.
9. Rolagem (`nd scroll`, passo `scroll`) e porte completo da biblioteca.
