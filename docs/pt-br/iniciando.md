# Primeiros passos

Este guia coloca um processo Bastion local e inspecionável para funcionar. Ele começa propositalmente pela interface de terminal: habilite um canal somente depois de entender suas credenciais e o mapeamento de proprietários.

## O que você precisa

- Uma toolchain Rust estável recente com Cargo.
- Git.
- Configuração de um provedor de modelo compatível com seu ambiente.
- Docker e Docker Compose apenas se for usar o deploy por Compose.

O repositório consome crates `bastion-core` por um commit Git fixado; portanto, o primeiro build pode baixar e compilar mais dependências que uma CLI pequena.

## Execute o primeiro turno

1. Clone o repositório e entre nele.

   ```bash
   git clone https://github.com/thewaifucorp/bastion-agent.git
   cd bastion-agent
   ```

2. Revise `installer.sh` e prepare o `.env` com os segredos internos obrigatórios.

   ```bash
   ./installer.sh --prepare-only
   ```

3. Revise `bastion.toml`. Ele contém os padrões não secretos, caminhos locais, canais e servidores MCP.

4. Compile e faça uma solicitação.

   ```bash
   cargo run -- agent --message "Resuma o que você pode fazer com segurança nesta instalação."
   ```

5. Abra o Bastion.

   ```bash
   cargo run
   ```

   Sem subcomando, `bastion` abre a TUI, inicia o runtime local se necessário,
   espera a prontidão e usa automaticamente o token de bootstrap local.
   `cargo run -- chat --url https://seu-host` é a forma remota explícita;
   `cargo run -- daemon` continua disponível para operação em foreground.

## Execute a stack Compose

O arquivo Compose incluso compila o core e os sidecars locais. Ele monta `bastion.toml` como somente leitura e guarda o estado em volumes nomeados.

```bash
./installer.sh
bastion
```

Na configuração fornecida, o core publica `127.0.0.1:8080`. Defina
`BASTION_PUBLISH_HOST` somente quando um proxy reverso ou cliente remoto precisar
alcançá-lo, mantendo essa superfície administrativa protegida por firewall.
`BASTION_HTTP_PORT` altera a porta do host; ao sobrescrevê-la, aponte
`BASTION_URL` para a URL correspondente. O instalador gera `APP_JWT_SECRET`,
`BASTION_INFER_TOKEN` e um token de bootstrap limitado ao proprietário.

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
