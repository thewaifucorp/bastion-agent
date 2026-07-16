# Instale o Bastion

## Stack self-hosted completa

Requisitos: Git, Docker Engine e Docker Compose v2.

```bash
git clone https://github.com/thewaifucorp/bastion-agent.git
cd bastion-agent
less installer.sh
./installer.sh
```

O instalador é idempotente: preserva `.env`, gera segredos internos ausentes,
valida o Compose, reconstrói as imagens e inicia a stack. Ele não instala Node,
registry externo de skills, bootstrap legado de plugins nem cria um segundo formato de configuração.

Modos úteis:

```bash
./installer.sh --prepare-only       # prepara .env sem exigir Docker
./installer.sh --no-start           # configura e compila sem iniciar
./installer.sh --non-interactive    # usa chaves exportadas no ambiente
./installer.sh --dir /opt/bastion   # caminho explícito
```

## Rust nativo

```bash
./installer.sh --prepare-only
cargo build --locked
cargo run -- daemon
```

O `bastion.toml` versionado usa caminhos locais e URLs MCP em loopback. O Compose
sobrescreve esses valores para sua rede e volumes. Veja [Configuração](configuracao.md).
