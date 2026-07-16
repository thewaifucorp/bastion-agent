# Configurando os canais (WhatsApp, Discord, Slack, Email, Voz)

Guia de onboarding pros 4 canais externos novos (CHAN-01/CHAN-03) + o canal de voz local
(VOICE-01). Todos vêm com `enabled = false` por padrão em `bastion.toml` — o daemon só
sobe o que você configurar explicitamente. Nenhum passo aqui exige mexer em código: env
vars no `.env` + `[[identity]]`/`[channels.*]` em `bastion.toml` + restart do daemon.

> **Regra geral:** setar as env vars certas + `enabled = true` no `bastion.toml` é
> suficiente pra `main.rs` subir o canal — sem precisar recompilar nada por deployment.

## WhatsApp

1. Crie um app em [developers.facebook.com](https://developers.facebook.com/apps) (tipo
   "Business"), adicione o produto **WhatsApp**.
2. No painel do produto WhatsApp, pegue:
   - `WHATSAPP_PHONE_NUMBER_ID` (ID do número de teste ou do número real)
   - `WHATSAPP_ACCESS_TOKEN` (token temporário de dev, ou permanente via System User)
   - `WHATSAPP_APP_SECRET` (Configurações do app -> Básico)
3. Escolha qualquer string forte pra `WHATSAPP_VERIFY_TOKEN` (você mesmo inventa — é só
   pro handshake de verificação do webhook).
4. No painel "Configuration" do produto WhatsApp, registre o webhook:
   `https://<seu-dominio>/whatsapp/webhook`, usando o MESMO `WHATSAPP_VERIFY_TOKEN` do
   passo 3.
5. Coloque as 4 env vars no `.env`, registre sua linha em `[[identity]]` (veja seção
   abaixo) e suba o daemon com `BASTION_WEBHOOK_ADDR` setado (WhatsApp reaproveita o
   router do webhook — não sobe um segundo servidor).

> **Pitfall:** um app do Meta recém-criado em modo "Development" só consegue mandar
> mensagem pra uma allowlist pequena de números de teste. Alcançar um número real em
> produção pode exigir revisão do app / verificação de número, o que leva um dia ou
> dois. Se mensagens falharem silenciosamente logo no início, budget esse lead time —
> não é bug de código.

## Discord

1. Crie uma aplicação em [discord.com/developers/applications](https://discord.com/developers/applications).
2. Na aba "Bot", crie um bot e pegue o `DISCORD_BOT_TOKEN` (nunca compartilhe esse
   token — ele nunca é logado pelo Bastion, T-10-05-03).
3. Convide o bot pro seu servidor (aba "OAuth2" -> "URL Generator", scope `bot`,
   permissões mínimas: ler/enviar mensagens).
4. Coloque `DISCORD_BOT_TOKEN` no `.env`, registre `discord_user_id` (o seu, como
   string) em `[[identity]]`, `enabled = true` em `[channels.discord]`.

> **Pitfall:** o **"Message Content Intent"** PRECISA estar habilitado no Developer
> Portal (aba Bot -> "Privileged Gateway Intents"). O código já pede essa intent, mas
> se o toggle do portal estiver desligado, o bot recebe os eventos com o conteúdo da
> mensagem sempre vazio — e nenhum erro é logado. Se o bot "responde" mas sempre parece
> não ter entendido nada, é isso.

## Slack

1. Crie um app em [api.slack.com/apps](https://api.slack.com/apps).
2. Habilite **Socket Mode** (aba "Socket Mode").
3. Em "OAuth & Permissions", instale o app no workspace e pegue o `SLACK_BOT_TOKEN`
   (começa com `xoxb-`).
4. Em "Basic Information" -> "App-Level Tokens", crie um token com o scope
   `connections:write` — esse é o `SLACK_APP_TOKEN` (começa com `xapp-`).
5. Coloque AMBOS os tokens no `.env`, registre `slack_user_id` (o seu Member ID do
   Slack) em `[[identity]]`, `enabled = true` em `[channels.slack]`.

> **Pitfall:** Socket Mode exige os DOIS tokens — trocar um pelo outro (ou esquecer o
> App-Level Token) faz o handshake do websocket falhar com um erro de auth pouco
> claro. `SLACK_BOT_TOKEN` (`xoxb-`) autentica chamadas de API normais (postar
> mensagem); `SLACK_APP_TOKEN` (`xapp-`) é especificamente pra abrir a conexão Socket
> Mode. Nenhum dos dois funciona sozinho.

## Email

1. Pegue `EMAIL_ADDRESS` (o endereço completo) e `EMAIL_PASSWORD`. Alguns provedores
   (ex.: Gmail) exigem uma **senha de app** específica em vez da senha normal da conta
   — procure "app password" nas configurações de segurança do provedor.
2. Pegue `EMAIL_IMAP_HOST` e `EMAIL_SMTP_HOST` (ex.: `imap.gmail.com` /
   `smtp.gmail.com`).
3. As portas padrão são 993 (IMAP) / 587 (SMTP) — só sobrescreva via
   `EMAIL_IMAP_PORT`/`EMAIL_SMTP_PORT` se seu provedor usar outra coisa.
4. Coloque tudo no `.env`, registre `email_address` em `[[identity]]`, `enabled =
   true` em `[channels.email]`.

## Voz (Voice)

Diferente dos 4 canais acima, voz **não precisa de nenhuma conta/token externo** — a
autenticação é a presença física do microfone/alto-falante no host do daemon.

1. Habilite em `bastion.toml`: `[channels.voice] enabled = true`.
2. Suba o sidecar de voz (STT/TTS rodam nele, nunca no core `FROM scratch`):
   `docker compose up -d voice`.
3. Confirme que o host do daemon tem microfone/alto-falante funcionando.
4. Aperte e segure a tecla push-to-talk (Espaço) pra gravar, solte pra enviar.

Wake-word (`wake_word_enabled`) é opt-in e vem desligado por padrão — só push-to-talk
funciona até você ligar explicitamente essa flag (e configurar o modelo de wake-word,
`VOICE_WAKE_WORD_MODEL_PATH`).

## A tabela `[[identity]]`

Todos os 7 canais (Telegram, Webhook, WhatsApp, Discord, Slack, Email — Voz não
precisa, ela resolve o owner localmente) resolvem o owner através da MESMA tabela
`[[identity]]` em `bastion.toml`. Um bloco por owner humano, mapeando o `owner_id`
canônico pra cada identificador de canal que essa pessoa usa:

```toml
[[identity]]
owner_id         = "mario"
telegram_chat_id = "12345678"
webhook_token    = "token-mario"
whatsapp_phone   = "+5511999999999"
discord_user_id  = "111222333444555"
slack_user_id    = "U01ABCDEF"
email_address    = "mario@example.com"
```

Só precisa preencher as colunas dos canais que essa pessoa realmente usa — as outras
ficam vazias/omitidas. Um exemplo comentado já vem em `bastion.toml`; descomente e
edite pra registrar a si mesmo.

> **Atenção pra quem já usava Telegram/Webhook antes desta versão:** o mecanismo de
> resolução de owner mudou de env vars (`BASTION_TELEGRAM_OWNERS`/
> `BASTION_WEBHOOK_OWNERS`) pra essa tabela `[[identity]]`. Se você não registrar sua
> linha aqui ANTES de atualizar, o Telegram/Webhook param de te reconhecer como owner
> — as mensagens caem no "sender desconhecido, ignorado silenciosamente" (mesma
> política de segurança de sempre, CR-03).

## Checklist final de verificação

Depois de configurar pelo menos um canal e reiniciar o daemon:

1. Confira o log (`bastion.log` ou `docker compose logs -f core`) e procure o evento
   `{channel}_started` correspondente (ex.: `discord_started`, `whatsapp_started`).
2. Mande uma mensagem real nesse canal, a partir da SUA identidade registrada em
   `[[identity]]`.
3. Confirme que uma resposta chega de volta.
4. Se algo falhar silenciosamente, releia os Pitfalls acima antes de assumir que é bug
   — a maioria dos problemas de setup desses canais é configuração no painel do
   provedor (Meta/Discord/Slack), não código.
