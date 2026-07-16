# Canais

Os canais são portas opcionais para o mesmo runtime de agente. Habilite um por vez, mapeie os proprietários permitidos em `[[identity]]` e valide os logs antes de adotá-lo no dia a dia.

## Mapeamento de proprietário

Cada canal associa o identificador do remetente a um `owner_id` canônico. Remetentes desconhecidos são rejeitados.

```toml
[[identity]]
owner_id = "mario"
telegram_chat_id = "12345678"
whatsapp_phone = "+5511900000000"
discord_user_id = "111222333444555"
slack_user_id = "U01ABCDEF"
email_address = "mario@example.com"
```

## Requisitos

| Canal | Configuração | Requisito |
| --- | --- | --- |
| Telegram | `TELEGRAM_BOT_TOKEN` | Mapeie a conversa Telegram. |
| Webhook/mobile | `BASTION_WEBHOOK_ADDR` e `APP_JWT_SECRET` | Proteja o endereço alcançável. |
| WhatsApp | `[channels.whatsapp]` e quatro valores `WHATSAPP_*` | Exige bind de webhook e valida assinatura Meta. |
| Discord | `[channels.discord]` e `DISCORD_BOT_TOKEN` | Ative Message Content Intent no portal Discord. |
| Slack | `[channels.slack]`, `SLACK_BOT_TOKEN`, `SLACK_APP_TOKEN` | Usa Socket Mode; os dois tokens são diferentes. |
| E-mail | `[channels.email]` e credenciais correspondentes | E-mail recebido é sempre não confiável. |
| Voz | `[channels.voice]` | Microfone/alto-falante local; wake word é opt-in. |

Mensagens públicas de Discord e Slack são não confiáveis; DMs não. WhatsApp valida a assinatura antes de interpretar o corpo. Essas classificações alimentam decisões de capacidade e egress do runtime.

## Checklist

1. Configure a identidade antes de iniciar o canal.
2. Guarde credenciais em `.env`.
3. Teste com uma identidade permitida e depois com uma não permitida.
4. Só então exponha um webhook ou bot a outras pessoas.
