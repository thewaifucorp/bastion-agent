# Channels

Channels are optional entry points to the same agent runtime. Enable one at a time, map its allowed owners in `[[identity]]`, and verify logs before treating it as a daily interface.

## Owner mapping is required

Every channel resolves a channel-specific sender identifier to a canonical `owner_id` from `bastion.toml`. Unknown senders are rejected. Add only the fields relevant to your channel:

```toml
[[identity]]
owner_id = "mario"
telegram_chat_id = "12345678"
whatsapp_phone = "+5511900000000"
discord_user_id = "111222333444555"
slack_user_id = "U01ABCDEF"
email_address = "mario@example.com"
```

## Channel requirements

| Channel | Enable/configure | Secret or operational requirement |
| --- | --- | --- |
| Telegram | Enable `[channels.telegram]` and set `TELEGRAM_BOT_TOKEN` | Telegram chat identity must be mapped. |
| Webhook/mobile/TUI | Enable `[channels.webhook]`; set `BASTION_WEBHOOK_ADDR` and `APP_JWT_SECRET` | Protect the reachable address; pairing and `bastion chat` use this surface. |
| WhatsApp | Enable `[channels.whatsapp]` and set all four `WHATSAPP_*` values | Requires webhook bind address and Meta signature verification. |
| Discord | Enable `[channels.discord]`, set `DISCORD_BOT_TOKEN` | Enable Discord’s Message Content intent in the Developer Portal. |
| Slack | Enable `[channels.slack]`, set `SLACK_BOT_TOKEN` and `SLACK_APP_TOKEN` | Uses Socket Mode; both token types are distinct. |
| Email | Enable `[channels.email]`; set address, password, IMAP and SMTP variables | Inbound email is always treated as untrusted. |
| Voice | Enable `[channels.voice]` | Local microphone/speaker; wake word is opt-in. |

## Trust behavior

Discord and Slack messages from public channels are classified as untrusted; direct messages are not. Email is always untrusted. WhatsApp validates the raw request signature before parsing its body. These distinctions are not cosmetic: they feed the runtime’s capability and egress decisions.

## Rollout checklist

1. Configure the identity row before starting the channel.
2. Put credentials in `.env`, never in a committed file.
3. Start the daemon and inspect structured logs.
4. Test from an allowed identity.
5. Test from a disallowed identity and confirm that it receives no agent session.
6. Only then make a webhook or bot reachable to others.
