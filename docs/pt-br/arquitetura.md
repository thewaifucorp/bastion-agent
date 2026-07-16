# Arquitetura

O Bastion é um runtime de produto em Rust sobre `bastion-core`. Este repositório compõe o loop do agente com canais reais, configuração, serviços MCP, extensões e aplicativo mobile.

```text
canal / CLI / mobile
          │
          ▼
 src/channel + src/api
          │  identidade e classificação de confiança
          ▼
 AgentHandle / AgentLoop (bastion-core)
          │
          ├── providers e sessões
          ├── registro de capacidades e aprovações
          ├── memória, personas, cognição e mesh
          ▼
 src/mcp + extensões + sidecars locais
```

## Limites do produto

| Área | Responsabilidade | Local |
| --- | --- | --- |
| Entrada | CLI, carregamento de config e composição | `src/main.rs` |
| Configuração | TOML/env e validação de identidades | `src/config.rs` |
| Canais | Telegram, webhook, WhatsApp, Discord, Slack, e-mail e voz | `src/channel/` |
| API | Inferência e pareamento webhook/mobile | `src/api/`, `src/channel/webhook.rs` |
| MCP | Clientes e servidor stdio opcional | `src/mcp/` |
| Extensões | Host declarativo, WASM e subprocesso | `src/extension/` |
| Companion | Aplicativo Flutter | `mobile/` |
| Skills | Serviços MCP locais e skills reutilizáveis | `skills/` |

## Fluxo de uma mensagem

1. Um adaptador recebe uma mensagem, ou a CLI recebe `agent --message`.
2. O adaptador resolve o remetente na tabela de identidades.
3. O conteúdo é marcado com o nível de confiança adequado.
4. `AgentHandle` envia a solicitação ao loop compartilhado.
5. O runtime usa provider, sessão, memória e capacidades registradas.
6. A resposta volta ao canal de origem. Remetentes desconhecidos não recebem sessão.

## Deploy

`docker-compose.yml` executa o core Rust e sidecars de MemuPalace, escrita de skills, autoaperfeiçoamento e voz. A rede interna `bastion-net` impede egress dos sidecars Python; somente o core também entra em `egress-net`. O estado fica em volumes nomeados.

## MCP e extensões

Clientes MCP fazem parte da composição do produto. Quando compilado com `mcp-server`, `bastion mcp-stdio` oferece transporte stdio local. Extensões declarativas, WASM e subprocesso ficam em `src/extension/`; instalar uma extensão não concede autoridade ilimitada, pois a decisão continua no modelo de capacidades do runtime.
