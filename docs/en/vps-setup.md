# VPS deployment checklist

Run Bastion on a VPS only when you are ready to operate an agent host: patch the host, control network exposure, protect credentials, and monitor logs.

## Before deployment

- Use a supported Linux host with Docker Compose or a Rust build environment.
- Create a dedicated non-root operating account and use restricted SSH access.
- Decide whether the webhook/mobile surface is private, VPN-only, or fronted by an authenticated reverse proxy.
- Prepare secrets outside the repository, including `APP_JWT_SECRET` when using webhook/mobile.

## Deploy

```bash
git clone https://github.com/thewaifucorp/bastion-agent.git
cd bastion-agent
# create .env privately, then review bastion.toml
docker compose up --build -d
docker compose ps
docker compose logs -f core
```

The provided Compose file publishes `8080`. Restrict that port before making it internet reachable, and preserve the core/sidecar Docker network split when adapting the deployment.

Back up named volumes, rotate credentials on personnel or device changes, and test identity rejection for every external channel. See [Security](security.md) and [Channels](channels.md).
