# Testes

O Bastion usa testes Rust para o runtime de produto e testes Python para skills locais. `tests/` inclui cobertura de conformidade, adversarial, integração, contratos live e ponta a ponta; nem todos devem ser executados com credenciais de produção.

## Checks locais rápidos

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

O GitHub Actions roda esses comandos em pull requests e pushes para `main`, além de `bash scripts/check-scope-and-scrub.sh`.

## Testes de skills

```bash
python3 -m pytest skills/ -q
python3 -m pytest skills/weight-system/tests/ -v --rootdir=.
```

Use a segunda forma para focar em uma skill. Instale primeiro as dependências daquela skill; o projeto não assume um único ambiente Python para todos os componentes opcionais.

## Testes Rust focados

```bash
cargo test config
cargo test --test extension_adversarial
```

Leia o teste antes de executar qualquer arquivo marcado como live ou end-to-end: ele pode exigir serviços externos, credenciais, binários locais ou Docker.

Ao adicionar cobertura, prefira fixtures e stubs a credenciais reais e nomeie explicitamente a propriedade de segurança protegida.
