# Getting Started — 10 minutes

This guide covers installing Bastion on your own computer (Mac, Linux, or Windows with WSL2).

---

## Prerequisites

Before you start, you need:

| What | How to get it |
|------|---------------|
| Docker Desktop | [docs.docker.com/get-docker](https://docs.docker.com/get-docker/) |
| API key from 1 LLM | Anthropic, OpenAI, Google Gemini, or Groq — any works |
| Telegram bot | Talk to [@BotFather](https://t.me/BotFather) and create a bot — it gives you a token |

> No Telegram? You can also use WhatsApp via Evolution API. See the section at the end.

---

## Step 1 — Run the installer

Open your terminal and run:

```bash
bash <(curl -fsSL https://bastion.run/install)
```

The installer will:
- Check if Docker is installed
- Download Bastion files to the `~/bastion` folder
- Create the `.env` file from the template

If you prefer to do it manually, clone the repository:

```bash
git clone https://github.com/samurai-py/bastion.git ~/bastion
cd ~/bastion
cp .env.example .env
```

---

## Step 2 — Fill in `.env`

Open `~/bastion/.env` in your favorite text editor and fill in:

```env
# LLM — fill in at least one key
ANTHROPIC_API_KEY=sk-ant-...
# OPENAI_API_KEY=sk-...
# GEMINI_API_KEY=...
# GROQ_API_KEY=...

# Telegram
TELEGRAM_BOT_TOKEN=123456789:AAF...

# JWT — generate a secure random string
JWT_SECRET=replace-with-a-long-random-string
```

To generate a secure `JWT_SECRET` in the terminal:

```bash
openssl rand -hex 32
```

---

## Step 3 — Start Bastion

```bash
cd ~/bastion
docker compose up -d
```

Wait for Docker to download the images (first time only). When done:

```bash
docker compose ps
```

You should see two containers running: `openclaw` and `caddy`.

---

## Step 4 — Send `/start` on Telegram

Open Telegram, find the bot you created, and send `/start`.

Bastion will start the guided onboarding — it will ask your name, the areas of your life you want it to help with, and set up TOTP authentication (you'll need the **Authy** app on your phone).

Onboarding takes about 5 minutes.

---

## Checking everything is working

To see logs in real time:

```bash
docker compose logs -f openclaw
```

To stop Bastion:

```bash
docker compose down
```

To update to the latest version:

```bash
docker compose pull
docker compose up -d
```

---

## Using WhatsApp instead of Telegram

Add the following variables to `.env`:

```env
WHATSAPP_API_URL=https://your-evolution-api.com
WHATSAPP_API_KEY=your-api-key
WHATSAPP_NUMBER=15551234567
```

And comment out the `TELEGRAM_BOT_TOKEN` line.

---

## Next steps

- [Setting up on a VPS](vps-setup.md) — to access from anywhere
- [Security guide](security.md) — best practices to protect your instance
- [Connect the mobile app](connecting-the-app.md)
