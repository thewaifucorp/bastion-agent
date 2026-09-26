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

## Instalação nativa (desktop, sem Docker)

Roda o Bastion direto na sua máquina (Linux ou macOS). Os logins de assinatura
funcionam do jeito normal — o navegador abre na mesma máquina, e `claude`,
`codex` e `opencode` usam o login que você já tem — porque não há nada entre o
Bastion e o seu host.

Requisitos: Git, toolchain Rust (`cargo`) e [uv](https://docs.astral.sh/uv/)
para os sidecars Python. No Linux, kernel com Landlock (5.13+, 6.7+ para as
regras de rede completas) ou namespaces sem privilégio funcionando para o
bubblewrap; no macOS, o `sandbox-exec` que já vem no sistema.

```bash
./installer.sh --native              # --with-voice inclui a voz local (~1 GB de modelos)
```

O que ele faz:

- compila o `bastion` e instala o launcher em `~/.local/bin`;
- guarda o estado em `<diretório de instalação>/data` (`BASTION_DATA_DIR`) e
  escreve `bastion.native.toml` (mesclado sobre o `bastion.toml` versionado)
  com `[sandbox] mode = "required"` e os sidecars a rodar;
- instala cada sidecar (memupalace, skill-writer, self-improving e, se pedido,
  voice) no próprio virtualenv e baixa os modelos — em execução os sidecars
  **não têm rede nenhuma**: falam com o Bastion e entre si por Unix sockets em
  um diretório privado (`$XDG_RUNTIME_DIR/bastion`, senão
  `$TMPDIR/bastion-<uid>`, 0700; `BASTION_RUN_DIR` sobrescreve), nunca por TCP;
- registra um serviço de usuário: `systemctl --user status bastion` no Linux,
  o launch agent `ai.thewaifucorp.bastion` no macOS (logs em `data/logs/`).

Tudo o que o Bastion executa — harnesses de agente, pack de git, extensões,
sidecars — roda confinado pelo sandbox do sistema (veja
[Configuração](configuracao.md#sandbox)). O daemon não inicia se o host não
tiver backend de sandbox.

Faça login numa assinatura com `bastion connect claude|codex|opencode` (roda o
login do próprio CLI na sua máquina) ou, para a assinatura do ChatGPT no loop do
próprio Bastion, `/auth connect codex` com `[subscriptions.codex] login = "browser"`.

`bastion update --apply --yes` atualiza a instalação nativa no lugar
(recompila, sidecars, reinicia o serviço) e volta a versão anterior se a nova
falhar no health check.

Para desenvolver sem o serviço: `./installer.sh --native --no-start` e depois
`bastion daemon` no diretório de instalação.
