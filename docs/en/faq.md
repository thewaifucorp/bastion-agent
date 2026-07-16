# Frequently Asked Questions

---

## Installation and setup

**Do I need to know how to code to install Bastion?**
No. The installer does everything automatically. You just need to know how to open a terminal and edit a text file.

**Does it work on Windows?**
Yes, with WSL2 (Windows Subsystem for Linux). Install WSL2, then follow the installation guide normally inside the Linux environment.

**Which LLM should I use?**
Any of them works. To get started, Groq is free and fast. For more elaborate responses, Claude (Anthropic) or GPT-4o (OpenAI) are better. You can configure multiple providers and OpenClaw will automatically fall back between them.

**Can I use it without Telegram? Just through the mobile app?**
Telegram (or WhatsApp) is required for the initial onboarding and for some configurations like `/connect-app`. Once set up, you can use it primarily through the mobile app.

---

## Personas and behavior

**How many personas can I have?**
There's no limit. In practice, 3 to 6 personas is what most people use.

**Can Bastion use the wrong persona?**
Yes, especially at the beginning. If this happens frequently, add more keywords to the correct persona or remove ambiguous keywords. You can ask directly: "Add the keyword 'deploy' to my Tech Lead persona".

**Can I have two personas active at the same time?**
Yes. If a message touches multiple contexts, Bastion activates all relevant personas simultaneously, each weighted by its `current_weight`.

**What happens if I have no personas configured?**
Bastion uses a neutral default behavior until you create personas. Onboarding creates the first ones automatically.

---

## Data and privacy

**Where is my data stored?**
On your computer or VPS. The SQLite database is at `~/bastion/db/life-log.db`. Your personas are at `~/bastion/personas/`. Nothing goes to external servers except LLM calls.

**Does the LLM see my conversations?**
Yes — messages are sent to the LLM for processing. This is unavoidable. If this is a concern, use a local LLM via Ollama (supported by OpenClaw).

**Can I export my data?**
Yes. Everything is in text files (Markdown) and SQLite — open formats you can read and export with any tool.

**What happens if I delete the database?**
Bastion loses the interaction history (life log), but personas and configurations remain intact — they live in the Markdown files at `~/bastion/personas/`.

---

## Security and authentication

**What is TOTP and why do I need it?**
TOTP is the 6-digit code that changes every 30 seconds in the Authy app. It protects Bastion even if someone has access to your Telegram — without the code, they can't use the agent.

**I forgot to set up Authy during onboarding. What do I do?**
Send `/start` again. Onboarding is idempotent — you can redo it without losing your personas.

**Can I disable TOTP?**
Not recommended, but possible by editing `USER.md` and setting `totp_configured: false`. Without TOTP, anyone with access to your Telegram can use Bastion.

**Can someone use Bastion if they have access to my Telegram?**
No, if TOTP is configured. They would also need the Authy code, which only exists on your phone.

---

## Mobile app

**Does the app work without internet?**
No. The app needs to communicate with your Bastion, which in turn needs to call the LLM. Without internet, nothing works.

**Does the app token expire?**
Yes, after 90 days. When it expires, generate a new code with `/connect-app` and reconnect.

**Can I connect multiple phones?**
Yes. Each device has its own token. Use `/devices` to see all of them and `/revoke` to disconnect any one.

---

## Common issues

**Bastion doesn't respond on Telegram**
1. Check if containers are running: `docker compose ps`
2. View logs: `docker compose logs -f openclaw`
3. Confirm that `TELEGRAM_BOT_TOKEN` in `.env` is correct

**"Invalid TOTP code"**
Make sure your phone's time is synchronized. TOTP depends on the exact time — a difference of more than 30 seconds invalidates the code.

**Bastion is slow**
This is probably LLM latency. Try switching to a faster model (Groq is the fastest). Also check if the VPS is overloaded: `docker stats`.

**Lost access to Authy and can't authenticate**
1. Try recovering Authy via cloud backup (if it was enabled)
2. If no backup, access the server directly via SSH, edit `~/bastion/USER.md` and set `totp_configured: false`, then restart: `docker compose restart openclaw`
3. Send `/start` on Telegram to reconfigure TOTP
