# Primeiros passos

Este guia coloca um processo Bastion local e inspecionável para funcionar. Ele começa propositalmente pela interface de terminal: habilite um canal somente depois de entender suas credenciais e o mapeamento de proprietários.

## O que você precisa

- Uma toolchain Rust estável recente com Cargo.
- Git.
- Configuração de um provedor de modelo compatível com seu ambiente.
- Docker e Docker Compose apenas se for usar o deploy por Compose.

O repositório consome crates `bastion-core` por uma tag Git fixada; portanto, o primeiro `cargo build` pode baixar e compilar mais dependências que um CLI pequeno.

## Execute o primeiro turno

1. Clone o repositório e entre nele.

   ```bash
   git clone https://github.com/thewaifucorp/bastion-agent.git
   cd bastion-agent
   ```

2. Revise `bastion.toml`. Ele contém padrões não secretos, como modelo, caminho de sessão, canais habilitados e servidores MCP.

3. Coloque credenciais do provedor e tokens de canais em um `.env` local. O binário carrega `.env` quando ele existe; o arquivo é ignorado pelo Git.

4. Compile e faça uma solicitação.

   ```bash
   cargo run -- agent --message "Resuma o que você pode fazer com segurança nesta instalação."
   ```

5. Inicie o daemon interativo quando quiser uma sessão persistente.

   ```bash
   cargo run -- daemon
   ```

## Execute a stack Compose

O arquivo Compose incluso compila o core e os sidecars locais. Ele monta `bastion.toml` como somente leitura e guarda o estado em volumes nomeados.

```bash
docker compose up --build
```

Na configuração fornecida, o core expõe a porta `8080`. Trate-a como superfície administrativa: restrinja o bind ou o firewall, defina `APP_JWT_SECRET` e não a publique amplamente apenas para testar.

## Confirme que está saudável

```bash
docker compose ps
docker compose logs -f core
```

Em um build local, os logs seguem `logging.log_path` em `bastion.toml`. No Compose padrão, eles ficam no volume de dados do Bastion.

## Próximos passos

- [Configuração](configuracao.md) para modelo, identidade e deploy.
- [Canais](canais.md) antes de adicionar um token de mensageria.
- [Segurança](seguranca.md) antes de tornar a instância acessível fora da sua máquina.
- [Desenvolvimento](desenvolvimento.md) se você pretende alterar o código.
