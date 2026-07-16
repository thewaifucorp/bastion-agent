# Documentação do Bastion

Bem-vindo. Estes guias descrevem o código deste repositório — não uma versão hospedada antiga nem outro gateway. Comece pelo caminho curto e habilite apenas o que você pretende operar.

## Começar e operar

1. [Primeiros passos](iniciando.md) — compile o Bastion, execute o primeiro turno no terminal e conheça as opções de deploy.
2. [Configuração](configuracao.md) — configure `bastion.toml`, variáveis de ambiente, identidades e canais.
3. [Canais](canais.md) — conecte Telegram, webhook/pareamento mobile, WhatsApp, Discord, Slack, e-mail ou voz local.
4. [Segurança](seguranca.md) — entenda a fronteira de confiança antes de conectar uma conta real.

## Conhecer o produto

- [Arquitetura](arquitetura.md) — runtime, canais, serviços MCP, armazenamento e extensões.
- [Personas](personas.md) — organize o comportamento do agente com personas.
- [App mobile](app-mobile.md) — compile e pareie o cliente Flutter.
- [Modo crise](modo-crise.md) — opere com uma postura deliberadamente mais restrita.
- [FAQ](faq.md) — dúvidas operacionais comuns.

## Desenvolver e contribuir

- [Desenvolvimento](desenvolvimento.md) — setup local e convenções do projeto.
- [Testes](testes.md) — verificações Rust, skills Python e integração.
- [Notas sobre o instalador](como-instalar.md) — escopo e limitações do instalador atual.
- [Deploy em VPS](configurando-a-vps.md) — checklist de deploy, não uma receita para expor a instância.
- [Migração do ClawHub](migracao-clawhub.md) — revise skills importadas antes de ativá-las.

Para o substrato estável que sustenta esta aplicação, veja [bastion-core](https://github.com/thewaifucorp/bastion-core).
