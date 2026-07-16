# Setting Up Bastion on a VPS

Running Bastion on a VPS means it's available 24/7, from anywhere, without leaving your computer on. This guide uses Ubuntu 22.04 — the process is similar on any Debian-based distro.

---

## Choosing a VPS

Any provider works. Some affordable options:

| Provider | Minimum recommended plan | Approximate price |
|----------|--------------------------|-------------------|
| Oracle Cloud | VM.Standard.E2.1.Micro (Always Free) | Free |
| Hetzner | CX22 (2 vCPU, 4 GB RAM) | ~€4/month |
| DigitalOcean | Basic Droplet (1 vCPU, 2 GB RAM) | ~$6/month |
| Vultr | Cloud Compute (1 vCPU, 2 GB RAM) | ~$6/month |

Bastion runs well with 1 vCPU and 2 GB of RAM.

---

## Step 1 — Access the VPS

```bash
ssh root@YOUR_VPS_IP
```

---

## Step 2 — Create a non-root user

```bash
adduser bastion
usermod -aG sudo bastion
su - bastion
```

---

## Step 3 — Install Docker

```bash
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker bastion
newgrp docker
```

Verify:

```bash
docker --version
```

---

## Step 4 — Point a domain to the VPS

In your domain registrar's panel, create an A record:

```
bastion.yourdomain.com  →  YOUR_VPS_IP
```

Wait for propagation (usually a few minutes).

> No domain? You can use a free subdomain from services like [DuckDNS](https://www.duckdns.org) or [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/).

---

## Step 5 — Install Bastion

```bash
bash <(curl -fsSL https://bastion.run/install)
cd ~/bastion
```

---

## Step 6 — Configure `.env`

```bash
cp .env.example .env
nano .env
```

Fill in the API keys. For `JWT_SECRET`:

```bash
openssl rand -hex 32
```

---

## Step 7 — Configure the domain in Caddyfile

Edit the `Caddyfile`:

```bash
nano Caddyfile
```

Replace `your-domain.example.com` with your actual domain:

```
bastion.yourdomain.com {
    reverse_proxy localhost:3000
}
```

Caddy obtains the HTTPS certificate automatically via Let's Encrypt. No additional configuration needed beyond the domain.

---

## Step 8 — Start Bastion

```bash
docker compose up -d
```

Check it's running:

```bash
docker compose ps
docker compose logs -f
```

---

## Step 9 — Configure the firewall

Allow only the necessary ports:

```bash
sudo ufw allow ssh
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp
sudo ufw enable
```

Port 3000 (OpenClaw) **should not** be exposed directly — it's already configured to only accept connections from `127.0.0.1`. Caddy handles the proxy.

---

## Keeping Bastion updated

To update:

```bash
cd ~/bastion
docker compose pull
docker compose up -d
```

To set up automatic updates, install Watchtower:

```bash
docker run -d \
  --name watchtower \
  -v /var/run/docker.sock:/var/run/docker.sock \
  containrrr/watchtower \
  --schedule "0 0 4 * * *" \
  --cleanup
```

This updates containers automatically every day at 4am.

---

## Backing up your data

Important data lives in:

```
~/bastion/personas/     # your personas and history
~/bastion/config/       # OpenClaw configuration
~/bastion/db/           # SQLite database (if DB_STRATEGY=sqlite)
~/bastion/.env          # your API keys
```

To back up:

```bash
tar -czf bastion-backup-$(date +%Y%m%d).tar.gz \
  ~/bastion/personas \
  ~/bastion/config \
  ~/bastion/db \
  ~/bastion/.env
```

---

## Next steps

- [Security guide](security.md) — additional settings to protect your VPS
- [Connect the mobile app](connecting-the-app.md)
