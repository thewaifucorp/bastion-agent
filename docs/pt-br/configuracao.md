# Configuração

O Bastion separa configuração não secreta de credenciais. Mantenha comportamento revisável em `bastion.toml`; injete tokens por `.env` ou pelo cofre de segredos do deploy.

## Precedência

O binário carrega primeiro `bastion.toml` (ou o caminho em `BASTION_CONFIG`) e depois variáveis com o prefixo `BASTION__`, usando `__` como separador de níveis.

```bash
BASTION__AGENT__DEFAULT_MODEL=seu-modelo cargo run -- daemon
BASTION__SESSION__DB_PATH=/data/sessions.db cargo run -- daemon
```

## Ajustes principais

| Área | Chave | Finalidade |
| --- | --- | --- |
| Agente | `agent.default_model` | Nome do modelo usado pelo runtime. |
| Agente | `agent.daily_budget_usd` | Orçamento diário configurado. |
| Sessão | `session.db_path` | Local do banco SQLite de sessões. |
| Sessão | `session.autocompact_threshold` | Limiar de compactação. |
| Logs | `logging.log_path` | Arquivo de logs JSON. |
| MCP | `mcp.tool_call_timeout_secs` | Timeout de chamadas de ferramentas. |

## Segredos e variáveis

Coloque os valores abaixo em `.env`, jamais no TOML versionado.

| Variável | Uso |
| --- | --- |
| `TELEGRAM_BOT_TOKEN` | Canal Telegram. |
| `BASTION_WEBHOOK_ADDR` | Endereço de bind do webhook/pareamento mobile. |
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
