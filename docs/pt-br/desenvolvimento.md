# Desenvolvimento

## Setup local

```bash
git clone https://github.com/thewaifucorp/bastion-agent.git
cd bastion-agent
cargo build
cargo test
```

`bastion-agent` usa crates `bastion-core` fixadas por Git. Um build limpo precisa de Git funcional e acesso às dependências. Para suites de skills Python, instale as dependências declaradas por cada skill em um ambiente isolado.

## Comandos úteis

| Comando | Finalidade |
| --- | --- |
| `cargo run -- daemon` | Inicia o daemon interativo. |
| `cargo run -- agent --message "…"` | Executa um turno e sai. |
| `cargo build --all-features` | Compila recursos opcionais. |
| `cargo fmt --check` | Verifica formatação Rust. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Executa o gate estrito do CI. |
| `cargo test` | Executa testes Rust. |
| `python3 -m pytest skills/ -q` | Executa testes das skills Python. |
| `bash scripts/check-scope-and-scrub.sh` | Executa a verificação de escopo público. |

## Convenções

- Rust inseguro é proibido pelos lints do crate.
- Prefira eventos `tracing` estruturados a saída ad-hoc.
- Evite `unwrap` e `expect` fora de testes, salvo invariante comprovada.
- Documentação Rust pública fica em inglês.
- Toda ferramenta deve passar pelo registro de capacidades.

Este repositório cuida do produto: canais, configuração, extensões e mobile. Alterações no loop do agente, providers, memória, personas, cognição ou mesh normalmente pertencem a `bastion-core`. Antes de abrir PR, rode os checks acima e atualize as duas árvores de documentação.
