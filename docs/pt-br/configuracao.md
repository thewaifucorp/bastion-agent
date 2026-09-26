# Configuração

O Bastion separa configuração não secreta de credenciais. Mantenha comportamento revisável em `bastion.toml`; injete tokens por `.env` ou pelo cofre de segredos do deploy.

## Precedência

O binário carrega primeiro `bastion.toml` (ou o caminho em `BASTION_CONFIG`) e depois variáveis com o prefixo `BASTION__`, usando `__` como separador de níveis.

```bash
BASTION__AGENT__DEFAULT_MODEL=seu-modelo cargo run -- daemon
BASTION__SESSION__DB_PATH=/data/sessions.db cargo run -- daemon
```

## Escolha de provider e modelo

Na TUI local, use `/connect` para ver a configuração segura de um provider e
`/models` para abrir o seletor pesquisável de modelos recomendados. A escolha
é salva ao lado do banco de sessões do daemon e volta automaticamente no próximo
start. `/model` mostra a escolha ativa; `/model reset` remove a preferência e
restaura `agent.default_model` do `bastion.toml`.

As chaves continuam fora da conversa e do TOML: configure-as no `.env` ou no
cofre de segredos do deploy antes de selecionar aquele provider.

### Assinatura do ChatGPT (Codex)

Uma assinatura ChatGPT Plus/Pro pode servir a inferência enquanto o Bastion
mantém o próprio loop, memória, ferramentas e aprovações. No console do daemon:

1. `/auth connect codex [perfil]` — faz o login; `perfil` é um rótulo opcional
   (`trabalho`, `pessoal`) para manter mais de uma conta.
2. `/model codex/<modelo>@<perfil>` — usa a assinatura nos próximos turnos.
3. `/model status` — mostra qual conta e modelo servem o turno e o consumo que
   de fato se conhece.

`/auth status` e `/auth disconnect <perfil>` gerenciam a conexão. Não é o
`/connect codex` da TUI, que faz login do Codex CLI dentro do container para o
backend `codex_app_server`.

O jeito de fazer login é definido no `bastion.toml`:

```toml
[subscriptions.codex]
login = "device"   # padrão: mostra um código para aprovar em qualquer aparelho
# login = "browser" # abre uma URL nesta máquina; callback em 127.0.0.1:1455
```

Use `device` em VPS ou container. `browser` só funciona com o navegador na
mesma máquina do daemon (ou com a porta 1455 redirecionada para ele); se a 1455
estiver ocupada ele usa a 1457, e falha se as duas estiverem.

O conector é `Experimental`: funciona de ponta a ponta, mas faz login como o
client OAuth público do Codex CLI, contra um endpoint que a OpenAI não
documenta.

### Workspace

O único diretório onde as ferramentas do Bastion (pack de git, extensões por
subprocesso) e os runtimes externos (Claude Code, Codex, OpenCode) trabalham.
Cada owner ganha `<raiz>/<owner>` para sessões de runtime.

```toml
[workspace]
root = "/home/eu/projetos/bastion-work"
```

Sem essa chave: `BASTION_WORKSPACE_DIR`, depois `$BASTION_DATA_DIR/workspace`,
depois `~/.local/share/bastion/workspace` (`$XDG_DATA_HOME` quando definido) no
Linux ou `~/Library/Application Support/Bastion/workspace` no macOS. O Compose
usa `/bastion-data/workspace`, que persiste. Nunca é o diretório de onde o
daemon foi iniciado.

Ferramentas que executam programas não veem o ambiente do daemon. Servidor MCP
por stdio recebe `PATH`, `HOME`, `TMPDIR`, `LANG`, `LC_ALL` mais o que a tabela
dele nomear (`env = { KEY = "v" }`, `env_passthrough = ["GITHUB_TOKEN"]`,
`cwd`). O pack de git roda com ambiente próprio, sem a sua config global de git
e com hooks do repositório desligados; `git` lê sem aprovação, `git_write`
(init, add, commit, branch) pede aprovação a cada chamada.

## Ajustes principais

| Área | Chave | Finalidade |
| --- | --- | --- |
| Agente | `agent.default_model` | Nome do modelo usado pelo runtime. |
| Agente | `agent.daily_budget_usd` | Orçamento diário configurado. |
| Sessão | `session.db_path` | Local do banco SQLite de sessões. |
| Sessão | `session.autocompact_threshold` | Limiar de compactação. |
| Logs | `logging.log_path` | Arquivo de logs JSON. |
| TUI | `tui.theme`, `tui.accent` | Preset RGB ou cor customizada do terminal. |
| TUI | `tui.mascot`, `tui.animations`, `tui.game`, `tui.pet` | Exibição, progressão e pet pack opcional. |
| MCP | `mcp.tool_call_timeout_secs` | Timeout de chamadas de ferramentas. |

## Segredos e variáveis

Coloque os valores abaixo em `.env`, jamais no TOML versionado.

| Variável | Uso |
| --- | --- |
| `TELEGRAM_BOT_TOKEN` | Canal Telegram. |
| `BASTION_PUBLISH_HOST`, `BASTION_HTTP_PORT` | Interface e porta publicadas pelo Compose; o padrão é `127.0.0.1:8080`. |
| `BASTION_WEBHOOK_ADDR` | Endereço de bind interno do webhook/pareamento mobile no container. |
| `APP_JWT_SECRET` | Assinatura JWT do webhook e do pareamento mobile. |
| `BASTION_BOOTSTRAP_TOKEN` | Acesso inicial de API/TUI limitado ao proprietário; rotacione depois do onboarding. |
| `BASTION_INFER_TOKEN` | Autentica chamadas dos sidecars ao gateway de inferência. |
| `WHATSAPP_PHONE_NUMBER_ID`, `WHATSAPP_ACCESS_TOKEN`, `WHATSAPP_APP_SECRET`, `WHATSAPP_VERIFY_TOKEN` | Canal WhatsApp Cloud API. |
| `DISCORD_BOT_TOKEN` | Canal Discord. |
| `SLACK_BOT_TOKEN`, `SLACK_APP_TOKEN` | Slack Socket Mode. |
| `BASTION_OTEL_STDOUT` | Habilita exportação OpenTelemetry no stdout quando `true`. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Habilita exportação OTLP/gRPC. |

## Identidades e canais

A tabela `[[identity]]` associa um `owner_id` canônico a identificadores específicos de canal. Um remetente não mapeado é rejeitado.

```toml
[[identity]]
owner_id = "mario"
telegram_chat_id = "12345678"
discord_user_id = "111222333444555"
slack_user_id = "U01ABCDEF"
email_address = "mario@example.com"
```

O webhook local vem habilitado; canais externos vêm desabilitados. Um canal só inicia com `enabled = true` e todas as credenciais obrigatórias no ambiente. Veja [Canais](canais.md).

No Compose, o mesmo `bastion.toml` é usado com overrides `BASTION__...` para caminhos e URLs internas. Não existe um segundo arquivo de configuração.

## Checklist seguro

- Mantenha `.env` fora do Git e rotacione qualquer segredo exposto.
- Mapeie somente pessoas autorizadas.
- Comece por um canal e valide os logs antes de habilitar outro.
- Trate mensagens públicas de Discord/Slack e e-mail recebido como conteúdo não confiável.
- Avalie privacidade antes de habilitar eventos de conteúdo na telemetria.
