# Presença e modo Bastion

A presença é um contrato único para toda superfície que mostra o Bastion ou
age sobre ele: a TUI, o app web, o app do celular, um aparelho de mesa, um
canal de chat. Tem três partes:

- **Estado para fora**: um snapshot compacto (`GET /presence/snapshot`) e
  avisos de mudança no `/events`.
- **Ações para dentro**: uma rota (`POST /presence/action`) para aprovar,
  negar, trocar de persona, cuidar do companion ou deixar um recado num
  aparelho.
- **Superfícies**: um registro de aparelhos pareados, cada um com o próprio
  token de escopo limitado.

O **modo Bastion** é construído em cima disso: um plugin instalado em outro
agente de código (o Claude Code primeiro) transforma esse agente num Bastion.
A identidade, as personas, o modelo de memória, o companion e as regras são do
Bastion; o agente hospedeiro roda o modelo e as ferramentas.

As rotas são servidas pelo daemon ao lado de `/companion` e autenticadas como
ela (`x-bastion-token`, token de dono ou JWT do app). As rotas de aparelho
também aceitam o token do aparelho (`x-bastion-device-token`), limitado ao que
ele pode fazer.

O contrato completo, com todos os formatos, está em
[docs/en/presence.md](../en/presence.md). Resumo:

## Configuração

```toml
[presence]
enabled = true            # monta as rotas /presence (padrão: true)
approval_wait_secs = 45   # quanto uma aprovação remota espera antes de devolver ao hospedeiro
journal = true            # anota no diário local quando um turno do hospedeiro termina
context_max_chars = 6000  # limite do contexto injetado no hospedeiro
advertise_url = ""        # URL que os aparelhos usam para chegar ao daemon
device_approvals = false  # aparelhos com toque podem aprovar/negar (veja abaixo)

[[presence.rules]]
tool = "Bash"             # glob no nome da ferramenta
input = "git push*"       # glob opcional no input principal (comando, caminho, url)
decision = "remote"       # allow | deny | ask | remote
```

| decisão | efeito no hospedeiro |
|---|---|
| `allow` | roda sem a pergunta do próprio hospedeiro |
| `deny` | bloqueado, com a regra como motivo |
| `ask` | o hospedeiro pergunta à pessoa, como faria normalmente |
| `remote` | o Bastion abre uma aprovação em toda superfície e espera até `approval_wait_secs`; aprovado vira `allow`, negado vira `deny`, sem resposta vira `ask` |

## O que nunca sai

O snapshot nunca carrega prompts, respostas, conteúdo de arquivo, diffs ou
saída de ferramenta. O contexto enviado ao hospedeiro passa pela mesma regra
de egress do servidor MCP: memória ou persona `local_only` nunca sai da
máquina. O diário fica `local_only` e nunca volta para um hospedeiro na nuvem.

## Aparelhos

```bash
bastion presence pair 192.168.0.42 4827   # o código mostrado no aparelho
bastion presence devices
bastion presence unpair bastion-4f2a
```

O tráfego dos aparelhos é HTTP sem criptografia na rede local: quem lê esse
tráfego pode copiar o token de um aparelho. Por isso `device_approvals` vem
desligado; ligue só numa rede em que você confia. Tudo na presença é separado
por dono: cada dono só vê e só responde o que é seu.

O firmware de referência para o GeekMagic SmallTV (Ultra e Pro) está em
[`firmware/smalltv`](../../firmware/smalltv/README.md).

## Modo Bastion no Claude Code

```bash
claude plugin marketplace add ./integrations/claude-code
claude plugin install bastion@bastion
```

| hook | o Bastion |
|---|---|
| `SessionStart` | registra a sessão; injeta persona, memória central, objetivos e as outras personas |
| `UserPromptSubmit` | injeta as memórias relevantes ao prompt |
| `PreToolUse` | aplica `[[presence.rules]]`; uma regra `remote` espera aprovação de qualquer superfície |
| `PostToolUse`, `Notification`, `Subagent*`, `PreCompact` | atualiza a presença e o companion |
| `Stop` | marca o turno como concluído; grava a entrada local do diário |
| `SessionEnd` | encerra a sessão |

Comandos: `/bastion:status`, `/bastion:persona <nome>`,
`/bastion:approve <id>`, `/bastion:deny <id>`. Com o daemon parado, todo hook
sai em silêncio e o Claude Code funciona exatamente como sem o plugin. Detalhes
de instalação, tokens e MCP em [integrations/claude-code](../../integrations/claude-code/README.md).
