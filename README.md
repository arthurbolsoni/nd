# nd

Automação de navegador (Edge via CDP) para o Claude Code. Usa a biblioteca
[nodriver-rs](https://github.com/arthurbolsoni/nodriver-rs).

- [Plano](docs/PLANO.md)
- [Tasks](docs/TASKS.md)
- [Anotações](docs/ANOTACOES.md)

## Uso

```
nd start [--oculto] [--perfil DIR|temp] [--lang pt-BR] [--headless]
nd open <url> [--nova] [--sem-map]  navega, espera a página assentar e mostra o mapa
nd map [--tudo] [--dentro REF] [--max N] [--href]
nd click <alvo> [--js]              clique de mouse (CDP); --js usa el.click()
nd fill <alvo> <valor> [--teclas]   preenche campo de texto
nd pick <alvo> <opção>              select, combobox, autocomplete, menu
nd check <alvo> [--nao]             checkbox, radio, switch
nd upload <alvo> <arquivo>...       input de arquivo ou botão que abre o seletor
nd key <tecla>... [--em ALVO]       Enter, Tab, Escape, Control+a, Shift+Tab...
nd wait text|sel|url|net <valor> [--sumir] [--max S]
nd net [filtro] [--novas] [--antigas] [--todas] [--ultimas N]
nd net --corpo N [--caminho a.b[].c] [--max N] [--saida ARQ]
nd api N [--json '{...}'] [--set a.b=v]... [--query k=v]... [--caminho ...] [--saida ARQ]
nd download <alvo> [--pasta DIR] [--max S] [--js]   clica e grava o arquivo que vier
nd download --url URL [--pasta DIR | --saida ARQ]   baixa com a sessão da aba
nd download                         arquivos da sessão
nd pdf <arq> [--paisagem]           imprime a aba atual em PDF
nd atlas [site]                     atlas do site (~/nodriver/sites/<site>.yaml); sem site, lista
nd atlas --rascunho                 passos desta sessão em YAML, pelo nome, para virar fluxo
nd run <site> <fluxo> -p k=v... [--desde N] [--ate N] [--confirmado] [--seco] [--tudo]
nd tabs                             lista as abas (* = atual)
nd tab <n|trecho>                   escolhe a aba atual
nd eval '<js>' | -f arq.js          JavaScript na aba atual
nd text [--sel CSS] [--max N]       texto visível
nd status
nd stop
```

- Alvo: ref do mapa (`e12`) ou texto (nome do elemento: rótulo, texto, aria-label,
  placeholder). Texto que bate com mais de um elemento dá erro com a lista de refs.
- Ações imprimem o que mudou: `+` elemento novo, `-` sumiu, `~` mudou, `texto novo:`, avisos,
  diálogos aceitos, ou `página nova` com o mapa. `--sem-diff` não lê a página depois.
- `wait net` considera as requisições concluídas desde a última ação.
- `nd net`: chamadas XHR/fetch/documento da página atual, uma por linha, com a forma do JSON da
  resposta e o nome da operação GraphQL:
  `#3 GET 200 /w/rest.php/v1/search/title?q=Recife 1.7 KB 30 ms → {pages: [{id: num, title: str, …} ×10]}`
- `nd api N` refaz a chamada `#N` por `fetch` dentro da página (cookies e origem do site), com o
  corpo ou a URL alterados.
- Downloads ficam sempre ligados e vão para `~/.nd/downloads`; a ação que baixa algo mostra
  `download: <caminho>`. Abas abertas pela ação aparecem como `aba nova [n]`.
- `nd download <alvo>` clica e grava o arquivo que o clique gerar: download, resposta que abre na
  mesma aba ou numa aba nova (PDF por GET ou POST) ou blob criado pela página.

Exemplo de mapa:

```
# Cadastro — 127.0.0.1:8765/form.html
estável em 0.9 s
h1 Cadastro de candidato
e1 form "Dados pessoais"
  e2 input[text] "Nome" [obrigatório]
  e4 select "Estado" = "Selecione" {Selecione|São Paulo|Rio de Janeiro|Minas Gerais}
  e5 checkbox "Aceito os termos" [desmarcado]
  e9 button "Salvar"
e22 iframe "localhost:8765"
  e26 form
    e27 input[text] "CEP"
```

- `nd run` executa os passos de um fluxo do atlas sem LLM; para antes de passo com
  `confirmar: true` e, se um passo falhar, mostra o erro, o mapa, a rede recente e como retomar.
  Formato do atlas em [PLANO](docs/PLANO.md#atlas).
- Perfil padrão: `~/nodriver/profile-edge`. `--perfil temp` usa um perfil descartável.
- `--oculto`: Edge num desktop Windows oculto, fora da tela e sem roubar o foco.
- Estado do daemon em `~/.nd/` (`daemon.json`, `daemon.log`); `ND_DIR` muda o diretório.
- `ND_LOG=debug` aumenta o log do daemon.

## Build e testes

```
cargo build --release
cargo test
```

A biblioteca é lida de `../nodriver-rs` (clone de `arthurbolsoni/nodriver-rs` ao lado deste repo).
