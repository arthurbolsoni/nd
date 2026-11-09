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
nd scroll [--subir] [--vezes N] [--fim] [--dentro ALVO]   rola (gesto de rolagem) e mostra o que carregou
nd login [--usuario CPF|EMAIL] [--enviar ALVO] [--checar JS]   login com a senha salva no Edge (o nd não vê a senha)
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
nd step <objetivo> [-d chave=valor]... [--max N] [--confianca X] [--confirmado]   o Decider dirige a página
nd captcha [arquivo] [--alvo X] [--linhas L --colunas C --recorte X,Y,W,H] [--seco]   captcha de grade pelo Decider
nd captcha --decider | --numerar | --clicar N,N | --arrastar DE,PARA [--celula PX]   desafio que não é grade de fotos
nd tabs                             lista as abas (* = atual)
nd tab <n|trecho>                   escolhe a aba atual
nd eval '<js>' | -f arq.js          JavaScript na aba atual
nd text [--sel CSS] [--max N]       texto visível
nd status
nd stop [--tudo]                    fecha as abas da sessão; a última sessão (ou --tudo) fecha o Edge
nd -s <sessão> <comando>            comando na janela da sessão (padrão: ND_SESSAO ou a do Claude Code)
```

- Alvo: ref do mapa (`e12`) ou texto (nome do elemento: rótulo, texto, aria-label,
  placeholder). Texto que bate com mais de um elemento dá erro com a lista de refs.
- `nd pick` com texto que bate em mais de uma opção não escolhe a primeira: dá erro. Em lista
  customizada a lista segue aberta e o erro traz as refs das opções (`e18 option "..."`); clique
  na certa com `nd click e18`. Em `<select>` nativo o erro lista os textos. Texto igual a uma
  opção continua escolhendo essa opção.
- `nd step <objetivo>`: quem decide cada passo é o **Decider** (`perplexity/pplx-decider-v1.2`,
  modelo de decisão da Perplexity via OpenRouter), não uma LLM. A cada passo ele recebe o objetivo,
  os dados (`-d nome=valor`), o histórico, a saída da última ação e o mapa da página, e responde
  três perguntas de múltipla escolha (ação, alvo e valor) com probabilidades; o `nd` executa e
  repete até ele dizer `concluir`. Ele não escreve texto livre: só digita os valores de `-d` ou
  um número visível na página (um token mostrado na tela, por exemplo). Cada linha mostra a
  confiança do passo, e o fim imprime chamadas, tokens e o valor gasto em dólar.
  Para com código 2 e imprime o mapa atual quando não deve seguir sozinho: clique em botão
  sensível (enviar, pagar, apagar, confirmar; `--confirmado` libera), a mesma ação três vezes,
  confiança abaixo de `--confianca` ou limite de `--max` passos. Precisa de `OPENROUTER_API_KEY`
  (ambiente ou linha em `~/.nd/.env`); `ND_DECIDER_MODEL` e `ND_DECIDER_URL` trocam modelo e
  endereço. Comparação com o Claude em `bench/`.
- `nd captcha`: captcha de grade ("selecione todos os quadrados com semáforos", reCAPTCHA e hCaptcha).
  Divide a imagem em peças e pergunta ao Decider, peça a peça, se atende à instrução (sim/não com
  probabilidade). A grade é achada pelas linhas brancas entre as peças. A instrução vem de `--alvo`,
  da frase que a página mostra no mapa (hCaptcha: "Select all drinks that can be hot or cold") ou do
  cabeçalho recortado da imagem, que segue junto de cada peça (reCAPTCHA); não há lista de objetos.
  Com `arquivo` imprime a grade marcada (`X` atende) e as peças de confiança baixa; sem `arquivo`
  captura a aba atual (o quadro do reCAPTCHA ou hCaptcha, se houver, senão a janela) e clica nas peças
  marcadas (`--seco` não clica). Imprime o tempo de captura, peças e cliques. Foto única sem linhas
  brancas precisa de `--linhas` e `--colunas` (e `--recorte` com cabeçalho). Dentro do `nd step` roda
  sozinho: quando o mapa da página cita captcha e a captura tem uma grade, o `nd` marca as peças e o
  passo seguinte fica com o Decider (clicar em Verificar/Próximo); até 6 rodadas por execução. Desafio
  aberto que não é grade de peças (arrastar, clicar em formas) devolve o controle com código 2.
  Mesma chave e mesmas variáveis do `nd step`. `ND_CAPTCHA_SALVAR=arquivo.png` grava a captura.
  Desafio que não é grade de fotos (hCaptcha: ícone que quebra o padrão, formas iguais, arrastar
  a peça): `nd captcha --numerar` grava `<estado>/captcha-grade.png`, a captura do desafio com uma
  grade de células de 48 px numeradas de 1 em diante (por linha), para quem chamou olhar a imagem;
  `--clicar 6,10` clica no centro das células e `--arrastar 3,9` arrasta da célula 3 para a 9. O
  `--decider` faz o Decider escolher as células olhando a imagem numerada (clicar nas N melhores, ou
  pegar e soltar no "Drag ..."); a instrução vem do mapa ou de `--alvo`. O `nd step` faz isso sozinho:
  até 4 tentativas por execução (cada desafio do hCaptcha tem 2 páginas) e, se falhar, devolve o
  controle com a imagem numerada para quem chamou. Acerto medido na demo do hCaptcha: grade de fotos
  quase sempre; "torre incompleta" e "arrastar o item do padrão" em parte das vezes; "clicar nas TRÊS
  formas" e "arrastar a forma" quase nunca (confiança de 0,03 a 0,15).
- Ações imprimem o que mudou: `+` elemento novo, `-` sumiu (saiu do DOM ou foi escondido), `~`
  mudou, `fora da vista N` (segue na página, recortado por uma área como carrossel), `texto novo:`,
  avisos, diálogos aceitos, ou `página nova` com o mapa. `--sem-diff` não lê a página depois.
- O mapa inclui o que a rolagem (da página ou de uma área com rolagem própria) deixou fora da
  tela; depois de `nd scroll`, o diff lista só o que carregou.
- `nd login` acha o formulário de login (página, iframes, diálogos), confere se o Edge preencheu
  usuário e senha (pelo `:autofill`, sem ler o valor), tenta a sugestão de senha salva pelo
  teclado quando não preencheu, envia com clique real e confere: `--checar` (o `login.checar` do
  atlas) ou o campo de senha sumir. Sem senha salva, para sem enviar. `--usuario` faz a primeira
  tela de login em duas etapas (CPF ou e-mail, continuar). Campo marcado
  `autocomplete="new-password"`/`off` (gov.br) tem a dica trocada e, se preciso, é recriado para o
  Edge oferecer a senha salva. Com `--checar` verdadeiro e sem formulário, diz "já logado". Senhas saem mascaradas
  (`••••`) no mapa, no `nd net`/`nd api` (form, JSON, multipart, query) e no rascunho do atlas.
- `nd scroll` rola ~0,9 tela por vez com o gesto de rolagem do navegador (como a roda do mouse),
  espera a página assentar a cada vez e mostra a rede e os itens novos. `--fim` segue até a página
  parar de crescer (limite de 30 rolagens; `--vezes` muda). `--dentro` rola a área com rolagem
  própria (lista, painel) que contém o elemento.
- `wait net` considera as requisições concluídas desde a última ação.
- `nd net`: chamadas XHR/fetch/documento da página atual, uma por linha, com a forma do JSON da
  resposta e o nome da operação GraphQL:
  `#3 GET 200 /w/rest.php/v1/search/title?q=Recife 1.7 KB 30 ms → {pages: [{id: num, title: str, …} ×10]}`
  Chamadas de iframe de outro site e de worker entram na lista da aba, marcadas com `(iframe)` ou
  `(worker)`; elas não seguram a espera da ação (`nd wait net` espera por elas).
- `nd api N` refaz a chamada `#N` por `fetch` dentro da página (cookies e origem do site), com o
  corpo ou a URL alterados; chamada de iframe é refeita dentro do iframe.
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
- Sessões: um daemon e um Edge para todos; cada sessão age na sua aba, numa janela própria. A
  sessão vem de `-s`/`--sessao`, `ND_SESSAO` ou `CLAUDE_CODE_SESSION_ID` (subagentes herdam este
  último e precisam de `-s`). A primeira sessão usa a aba que já estava aberta; as seguintes
  ganham janela nova. Pop-up fica com a sessão da aba que o abriu; `nd tabs` marca as abas de outra
  sessão e `nd tab` recusa escolher uma delas. O rascunho do atlas é por sessão. Os comandos
  seguem um de cada vez no daemon (o de uma sessão espera o da outra terminar).
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
