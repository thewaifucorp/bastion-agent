# Instalação local — 10 minutos

Este guia cobre a instalação do Bastion no seu próprio computador (Mac, Linux ou Windows com WSL2).

---

## Pré-requisitos

Antes de começar, você precisa ter:

| O que | Como obter |
|-------|-----------|
| Docker Desktop | [docs.docker.com/get-docker](https://docs.docker.com/get-docker/) |
| API key de 1 LLM | Anthropic, OpenAI, Google Gemini ou Groq — qualquer um serve |
| Conta Maton | Crie grátis em [maton.ai](https://maton.ai) e gere uma API key |
| Bot no Telegram | Fale com [@BotFather](https://t.me/BotFather) e crie um bot — ele te dá um token |

> Não tem Telegram? Você também pode usar WhatsApp via Twilio. Veja a seção no final.

---

## Passo 1 — Rode o instalador

Abra o terminal e execute:

```bash
curl -fsSL https://get.bastion.ai | bash
```

O instalador vai:
- Verificar se o Docker está instalado
- Baixar os arquivos do Bastion para a pasta `~/bastion`
- Criar o arquivo `.env` a partir do template

Se preferir fazer manualmente, clone o repositório:

```bash
git clone https://github.com/bastion-ai/bastion.git ~/bastion
cd ~/bastion
cp .env.example .env
```

---

## Passo 2 — Preencha o `.env`

Abra o arquivo `~/bastion/.env` no seu editor de texto favorito e preencha:

```env
# LLM — preencha pelo menos uma chave
ANTHROPIC_API_KEY=sk-ant-...
# OPENAI_API_KEY=sk-...
# GEMINI_API_KEY=...
# GROQ_API_KEY=...

# Maton (obrigatório para integrações como Google Calendar)
MATON_API_KEY=...

# Telegram
TELEGRAM_BOT_TOKEN=123456789:AAF...

# JWT — gere uma string aleatória segura
JWT_SECRET=troque-por-uma-string-longa-e-aleatoria
```

Para gerar um `JWT_SECRET` seguro no terminal:

```bash
openssl rand -hex 32
```

---

## Passo 3 — Suba o Bastion

```bash
cd ~/bastion
docker compose up -d
```

Aguarde o Docker baixar as imagens (só na primeira vez). Quando terminar:

```bash
docker compose ps
```

Você deve ver dois containers rodando: `openclaw` e `caddy`.

---

## Passo 4 — Envie `/start` no Telegram

Abra o Telegram, encontre o bot que você criou e envie `/start`.

O Bastion vai iniciar o onboarding guiado — ele vai perguntar seu nome, as áreas da sua vida que quer gerenciar, e configurar a autenticação TOTP (você vai precisar do app **Authy** no celular).

O onboarding leva cerca de 5 minutos.

---

## Verificando se está tudo certo

Para ver os logs em tempo real:

```bash
docker compose logs -f openclaw
```

Para parar o Bastion:

```bash
docker compose down
```

Para atualizar para a versão mais recente:

```bash
docker compose pull
docker compose up -d
```

---

## Usando WhatsApp em vez de Telegram

Adicione as seguintes variáveis ao `.env`:

```env
TWILIO_ACCOUNT_SID=AC...
TWILIO_AUTH_TOKEN=...
TWILIO_WHATSAPP_NUMBER=whatsapp:+14155238886
```

E comente a linha `TELEGRAM_BOT_TOKEN`.

---

## Próximos passos

- [Como subir numa VPS](vps-setup.md) — para acessar de qualquer lugar
- [Guia de segurança](security.md) — boas práticas para proteger sua instância
- [Conectar o app mobile](connect-app.md)
