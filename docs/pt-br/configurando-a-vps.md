# Subindo o Bastion numa VPS

Rodar o Bastion numa VPS significa que ele fica disponível 24/7, de qualquer lugar, sem precisar deixar o computador ligado. Este guia usa Ubuntu 22.04 — o processo é similar em qualquer distro Debian-based.

---

## Escolhendo uma VPS

Qualquer provedor serve. Algumas opções acessíveis:

| Provedor | Plano mínimo recomendado | Preço aproximado |
|----------|--------------------------|-----------------|
| Oracle Cloud | VM.Standard.E2.1.Micro (Always Free) | Grátis |
| Hetzner | CX22 (2 vCPU, 4 GB RAM) | ~€4/mês |
| DigitalOcean | Droplet Basic (1 vCPU, 2 GB RAM) | ~$6/mês |
| Vultr | Cloud Compute (1 vCPU, 2 GB RAM) | ~$6/mês |

O Bastion roda bem com 1 vCPU e 2 GB de RAM.

---

## Passo 1 — Acesse a VPS

```bash
ssh root@SEU_IP_DA_VPS
```

---

## Passo 2 — Crie um usuário não-root

```bash
adduser bastion
usermod -aG sudo bastion
su - bastion
```

---

## Passo 3 — Instale o Docker

```bash
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker bastion
newgrp docker
```

Verifique:

```bash
docker --version
```

---

## Passo 4 — Aponte um domínio para a VPS

No painel do seu registrador de domínio, crie um registro A:

```
bastion.seudominio.com  →  SEU_IP_DA_VPS
```

Aguarde a propagação (geralmente alguns minutos).

> Não tem domínio? Você pode usar um subdomínio gratuito de serviços como [DuckDNS](https://www.duckdns.org) ou [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/).

---

## Passo 5 — Instale o Bastion

```bash
curl -fsSL https://get.bastion.ai | bash
cd ~/bastion
```

---

## Passo 6 — Configure o `.env`

```bash
cp .env.example .env
nano .env
```

Preencha as chaves de API. Para o `JWT_SECRET`:

```bash
openssl rand -hex 32
```

---

## Passo 7 — Configure o domínio no Caddyfile

Edite o `Caddyfile`:

```bash
nano Caddyfile
```

Substitua `your-domain.example.com` pelo seu domínio real:

```
bastion.seudominio.com {
    reverse_proxy localhost:3000
}
```

O Caddy obtém o certificado HTTPS automaticamente via Let's Encrypt. Não precisa configurar nada além do domínio.

---

## Passo 8 — Suba o Bastion

```bash
docker compose up -d
```

Verifique se está rodando:

```bash
docker compose ps
docker compose logs -f
```

---

## Passo 9 — Configure o firewall

Libere apenas as portas necessárias:

```bash
sudo ufw allow ssh
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp
sudo ufw enable
```

A porta 3000 (OpenClaw) **não** deve ser exposta diretamente — ela já está configurada para aceitar conexões apenas de `127.0.0.1`. O Caddy faz o proxy.

---

## Mantendo o Bastion atualizado

Para atualizar:

```bash
cd ~/bastion
docker compose pull
docker compose up -d
```

Para configurar atualização automática, instale o Watchtower:

```bash
docker run -d \
  --name watchtower \
  -v /var/run/docker.sock:/var/run/docker.sock \
  containrrr/watchtower \
  --schedule "0 0 4 * * *" \
  --cleanup
```

Isso atualiza os containers automaticamente todo dia às 4h da manhã.

---

## Backup dos dados

Os dados importantes ficam em:

```
~/bastion/personas/     # suas personas e histórico
~/bastion/config/       # configurações do OpenClaw
~/bastion/db/           # banco SQLite (se DB_STRATEGY=sqlite)
~/bastion/.env          # suas chaves de API
```

Para fazer backup:

```bash
tar -czf bastion-backup-$(date +%Y%m%d).tar.gz \
  ~/bastion/personas \
  ~/bastion/config \
  ~/bastion/db \
  ~/bastion/.env
```

---

## Próximos passos

- [Guia de segurança](security.md) — configurações adicionais para proteger sua VPS
- [Conectar o app mobile](connect-app.md)
