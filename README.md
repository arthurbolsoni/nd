# nd

Automação de navegador (Edge via CDP) para o Claude Code. Usa a biblioteca
[nodriver-rs](https://github.com/arthurbolsoni/nodriver-rs).

- [Plano](docs/PLANO.md)
- [Tasks](docs/TASKS.md)
- [Anotações](docs/ANOTACOES.md)

## Uso

```
nd start [--oculto] [--perfil DIR|temp] [--lang pt-BR] [--headless]
nd open <url> [--nova]        navega e espera rede ociosa + DOM parado
nd tabs                       lista as abas (* = atual)
nd tab <n|trecho>             escolhe a aba atual
nd eval '<js>' | -f arq.js    JavaScript na aba atual
nd text [--sel CSS] [--max N] texto visível
nd status
nd stop
```

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
