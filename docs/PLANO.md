# nd — plano

Automação de navegador para o Claude Code: Edge controlado por CDP, com um daemon que mantém a
conexão aberta, um cliente de linha de comando curto, snapshot com refs, captura de rede, receitas
para armadilhas comuns, atlas por site e replay de fluxos.

## Arquitetura

Um executável, `nd.exe`, em dois papéis:

- `nd daemon` — mantém o Edge e a conexão CDP, captura a rede de todas as abas e atende HTTP em
  `127.0.0.1` com token em `~/nodriver/nd/.token`.
- `nd <comando>` — cliente; manda o comando ao daemon e imprime a resposta.

Componentes:

- Cliente CDP próprio (tokio + tokio-tungstenite + serde_json). Sem chromiumoxide: só os domínios
  que o comando precisa são ativados.
- Lançamento do Edge com os flags do nodriver 0.50.3 (sem chromedriver, sem flags de automação),
  perfil `~/nodriver/profile-edge`.
- Desktop oculto (`CreateDesktopW` + `CreateProcessW`, crate `windows`), porte do
  `edge_isolado.py`. Padrão; `nd start --visivel` abre no desktop do usuário para login, captcha
  ou 2FA.
- JS injetado em arquivos `.js` (`include_str!`): snapshot, receitas, diff pós-ação.
- Atlas em YAML (serde).
- `fechar.ps1` passa a encerrar o daemon.

## Comandos

| Comando | Faz |
|---|---|
| `nd start [--visivel]` / `nd stop` | sobe ou encerra daemon e Edge |
| `nd open <url> [--nova]` | navega e espera rede ociosa + DOM parado |
| `nd tabs` / `nd tab <n\|trecho>` | lista abas, escolhe a ativa |
| `nd map [--tudo] [--dentro <ref>]` | snapshot compacto com refs |
| `nd text [--sel X]` | `innerText` da página ou de um trecho |
| `nd click <ref\|"texto">` | clique por CDP no centro do elemento |
| `nd fill <ref> "valor"` | preenche campo, com tratamento de input controlado (React) |
| `nd pick <ref> "opção"` | select nativo, combobox, autocomplete |
| `nd check <ref>` / `nd upload <ref> <arq>` / `nd key <tecla>` | checkbox, upload, teclas |
| `nd wait text\|sel\|url\|net <x>` | espera explícita |
| `nd eval '<js>'` / `nd eval -f arq.js` | JS livre |
| `nd net [filtro] [--corpo n]` | XHR/fetch capturados |
| `nd api <n> [--json '{...}']` | refaz chamada capturada via `fetch` na página |
| `nd download` / `nd pdf <arq>` | download de blob, impressão em PDF |
| `nd atlas <site>` | mostra o atlas do site |
| `nd run <site> <fluxo> -p k=v [--ate N]` | replay de fluxo do atlas |

Ação que muda a página devolve um diff: URL nova, elementos que apareceram ou sumiram, toast ou
erro visível.

## Snapshot (`nd map`)

- Elementos interativos visíveis: button, a, input, select, textarea, `[role=…]`,
  `[contenteditable]`, elementos com `onclick` ou `cursor:pointer`.
- Nome: label, aria-label, placeholder, texto, title. Estado: valor, checked, disabled, expanded,
  obrigatório.
- Formulários agrupados, títulos h1–h3 como âncora.
- Shadow DOM aberto; iframe do mesmo domínio por JS, de outro domínio pelo target CDP do frame.
- Padrão: viewport e entorno. `--tudo`: página inteira. Corte em ~300 linhas com indicação do
  restante.

Formato:

```
# Candidatura — jobs.site.com/apply/123
form "Dados pessoais"
  e3  input[text]  "Nome*"          = ""
  e4  input[email] "E-mail*"        = "arthur@…"
  e5  combobox     "Estado"         = "Selecione"
e12 button "Enviar candidatura"  [disabled]
iframe#2 (outro domínio: forms.x.com) — 4 elementos → nd map --dentro iframe#2
```

Refs: atributo `data-nd="eN"` no elemento e localizador de reserva no daemon (role + nome, CSS
curto). Ref perdida por re-render é resolvida pelo localizador; falha nos dois devolve
"ref velha, rode nd map".

## Rede

- Domínio `Network` em todas as abas; buffer das últimas 500 requisições XHR/fetch/documento.
- Guarda método, URL, status, tipo, corpo enviado, corpo da resposta JSON até 200 KB.
- `nd net`: uma linha por chamada com a forma do JSON (chaves e tipos).
- `nd api`: `fetch` de dentro da página com `credentials: 'include'`; origem, cookies e CSRF
  ficam os do site.
- Fora da lista: imagem, fonte, analytics (lista de domínios e tipos).

## Receitas

Levantadas dos scripts do `~/nodriver` (`iframe_file`, `blobcap`, `formsave`, `fetchsave`,
`clickcapture`, `printpdf`, `viaunica`, `live.py`, `*_apply.py`):

- `fill`: foco + clique, limpa, `Input.insertText`; se o valor não ficar, setter nativo + eventos
  `input`/`change`; confere o valor lido.
- `pick`: select nativo direto; combobox = clicar, digitar, esperar a lista, escolher pelo texto;
  opção inexistente devolve as opções disponíveis.
- `upload`: `DOM.setFileInputFiles` (iframe e input escondido).
- Diálogos `alert`/`confirm` aceitos e registrados no diff.
- Espera pós-ação: rede ociosa + DOM parado por 300 ms, teto de 10 s.
- Download de blob e PDF.

## Atlas

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
    params: [url, cv]
    passos:
      - open: "{url}"
      - fill: { nome: "Nome", valor: "Arthur Bolsoni" }
      - upload: { nome: "Resume", arquivo: "{cv}" }
      - click: { nome: "Submit Application", confirmar: true }
      - esperar: { texto: "Thanks for applying" }
```

- Passos por nome e role, não por ref.
- Sem cookie, token, senha ou dado pessoal além do que o fluxo usa; o resto vira `{param}`.

## Replay

`nd run <site> <fluxo> -p k=v`: executa os passos sem LLM. Falha num passo para ali e imprime
número do passo, erro, `nd map` e `nd net` recente. Passo com `confirmar: true` não roda no
replay; `--ate N` limita até o passo N.

## Skill e regra

- Skill `navegar` (`~/.claude/skills/navegar/SKILL.md`):
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
- `cargo test` cobre cada comando, ref após re-render, diff pós-ação, rede, `nd api` e replay com
  falha no passo certo.
- Aceitação: site real sem skill; tarefa do zero com `nd`, atlas gravado, repetição com `nd run`;
  comparação do número de chamadas de ferramenta com o método atual.

## Ordem

1. Cliente CDP, lançamento do Edge, desktop oculto, daemon + cliente, `open`/`tabs`/`eval`/`text`,
   `fechar.ps1`. Conferência de detecção nos sites que hoje passam com nodriver.
2. `map` + refs, `click`/`fill`/`pick`/`check`/`upload`/`key`/`wait`, diff pós-ação.
3. Rede: `net`, `api`.
4. Receitas restantes: diálogos, download, PDF.
5. Atlas + `run`.
6. Skill `navegar` + linha no CLAUDE.md.
7. Aceitação em site real.

## Em aberto

- Desktop oculto como padrão.
- Local do atlas: `~/nodriver/sites/` ou dentro de `~/.claude` (sincronizado).
- Site do teste de aceitação.
