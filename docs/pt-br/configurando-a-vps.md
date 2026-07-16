# Checklist de deploy em VPS

Execute o Bastion em uma VPS apenas se estiver preparado para operar um host de agente: mantenha o host atualizado, controle a exposição de rede, proteja credenciais e monitore logs.

- Use Linux com Docker Compose ou ambiente de build Rust.
- Crie uma conta operacional não-root e restrinja SSH.
- Decida se webhook/mobile ficará privado, em VPN ou atrás de proxy autenticado.
- Guarde segredos fora do repositório, incluindo `APP_JWT_SECRET` quando webhook/mobile estiver ativo.

```bash
git clone https://github.com/thewaifucorp/bastion-agent.git
cd bastion-agent
# crie .env privadamente e revise bastion.toml
docker compose up --build -d
docker compose ps
docker compose logs -f core
```

O Compose publica `8080`; restrinja essa porta antes de torná-la acessível pela internet e preserve a separação de redes entre core e sidecars. Faça backup dos volumes nomeados, rotacione credenciais quando necessário e teste rejeição de identidade em cada canal externo. Veja [Segurança](seguranca.md) e [Canais](canais.md).
