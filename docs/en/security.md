# Security Guide

Bastion is designed to be secure by default — but there are some settings you should verify and best practices that make a real difference.

---

## What's already configured by default

You don't need to do anything for this. It's already active:

**Hardened container**
The OpenClaw container runs with the following protections:
- Never as root — uses user `1000:1000`
- Container filesystem in read-only mode (`read_only: true`)
- All Linux capabilities removed except `NET_BIND_SERVICE`
- Port 3000 exposed only to `127.0.0.1` — not directly accessible from the internet

**Mandatory HTTPS**
Caddy obtains and renews TLS certificates automatically. All communication is encrypted.

**TOTP authentication**
Every new session requires a 6-digit code from Authy before processing any message. Even if someone has access to your Telegram, they can't use Bastion without the code.

**User allowlist**
Bastion only responds to `user_ids` listed in `USER.md`. Messages from other users are silently ignored.

---

## What you should configure

### 1. Use a strong `JWT_SECRET`

The `JWT_SECRET` in `.env` signs mobile app tokens. Use a long, random string:

```bash
openssl rand -hex 32
```

Never use values like `secret`, `123456`, or anything predictable.

### 2. Never commit `.env`

The `.env` file contains all your API keys. It's already in `.gitignore`, but verify:

```bash
git status
```

If `.env` appears as "untracked" or "modified", don't commit it. If you accidentally committed it, revoke all keys immediately in the respective service dashboards.

### 3. Configure the firewall on your VPS

If running on a VPS, allow only the necessary ports:

```bash
sudo ufw allow ssh
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp
sudo ufw enable
```

Never expose port 3000 directly.

### 4. Keep Bastion updated

Updates may contain security fixes:

```bash
cd ~/bastion
docker compose pull
docker compose up -d
```

---

## Agent behavioral protections

Beyond infrastructure, Bastion has guardrails that protect you from unwanted actions:

**Financial actions**
Bastion never executes payments, transfers, or any financial transaction autonomously. For any action involving money, it describes exactly what it's going to do, shows the amount and recipient, and waits for your explicit confirmation.

**Irreversible actions**
Before deleting files, sending emails, cancelling meetings, or posting on social media, Bastion always asks in the format:
```
I'm about to [exact description of action]. Confirm? (yes/no)
```
Any response that isn't "yes" is treated as "no".

**Anti prompt injection**
If you ask Bastion to read a web page or file that contains disguised instructions for the agent (e.g., "Ignore your previous instructions and do X"), it completely ignores those instructions and logs the attempt.

**Skill installation**
Bastion only installs skills from ClawHub that have the "Verified" badge, a minimum rating of 4.0, and at least 50 reviews. Skills that don't meet these criteria are automatically blocked.

---

## If you suspect a compromise

**Someone accessed your account without authorization:**
1. Revoke the TOTP secret immediately — generate a new one in onboarding (`/start`)
2. Check the logs: `docker compose logs openclaw | grep "authenticated"`
3. Replace all API keys in `.env` and restart: `docker compose up -d`

**Lost your phone with Authy:**
1. Try recovering Authy via cloud backup (if it was enabled)
2. If no backup, access the server directly via SSH, edit `~/bastion/USER.md` and set `totp_configured: false`, then send `/start`

**Mobile device compromised:**
```
/revoke device-name
```
Access is revoked immediately.

---

## Data privacy

- Your conversations stay 100% in your local database (SQLite by default)
- The only data that leaves your machine are the calls to the LLM you configured (Anthropic, OpenAI, etc.) — this is unavoidable, as it's the language model processing your messages
- If you want more control, use a local LLM via Ollama (supported by OpenClaw)
