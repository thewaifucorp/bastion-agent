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
Ele extrai o binário de release da imagem e instala um launcher em
`~/.local/bin/bastion`; adicione esse diretório ao `PATH` caso seu shell ainda
não o inclua. Depois da instalação, o comando normal é simplesmente:

```bash
bastion
```

Modos úteis:

```bash
./installer.sh --prepare-only       # prepara .env sem exigir Docker
./installer.sh --no-start           # configura e compila sem iniciar
./installer.sh --non-interactive    # usa chaves exportadas no ambiente
./installer.sh --dir /opt/bastion   # caminho explícito
```

## Atualizando uma instalação em execução

Consulte a GitHub Release oficial a partir do host:

```bash
bastion update
```

Para aplicar explicitamente a release mais recente:

```bash
bastion update --apply --yes
```

O instalador busca a tag da release, recusa checkout com alterações locais
rastreadas, reconstrói e reinicia o Compose, faz health check do `core` e
restaura a revisão anterior se a nova versão não ficar saudável.

Toda instalação por Compose também recebe um updater estreito no host. Em um
canal confiável/mapeado ou na TUI, `/update` mostra o estado e `/update apply`
pede o mesmo fluxo local. O container nunca recebe o socket Docker nem escrita
no checkout; é uma ação explícita do dono, jamais atualização automática.

## Rust nativo

```bash
./installer.sh --prepare-only
cargo build --locked
cargo run
```

O `bastion.toml` versionado usa caminhos locais e URLs MCP em loopback. O Compose
sobrescreve esses valores para sua rede e volumes. Veja [Configuração](configuracao.md).
