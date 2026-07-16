# Notas de instalação

O caminho de instalação auditável é clonar este repositório e compilá-lo com Cargo, ou construir a stack Compose inclusa. `installer.sh` é um script operacional em manutenção: leia-o antes de rodar, pois ele pode preparar o host, verificar Docker, clonar arquivos e instalar skills opcionais.

```bash
git clone https://github.com/thewaifucorp/bastion-agent.git
cd bastion-agent
cargo build
cargo run -- daemon
```

Para o deploy local multi-serviço:

```bash
docker compose up --build
```

Antes de qualquer caminho, revise `bastion.toml`, crie `.env` privado para segredos e leia [Configuração](configuracao.md). Não use comandos `curl | shell` sem verificar de forma independente o endpoint, a URL do repositório e a versão.
