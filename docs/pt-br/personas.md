# Personas e Cabinet

Personas dão a uma instância Bastion perspectivas distintas e revisáveis para os diferentes domínios de uma vida: trabalho, saúde, relacionamentos, aprendizado, finanças ou um projeto específico. Elas não são bots separados e não ignoram as fronteiras de identidade, capacidade ou privacidade do runtime.

No deploy por Compose, o diretório `personas/` do repositório é montado como somente leitura no container core. Trate arquivos de persona como política: revise mudanças, mantenha segredos fora deles e não permita que conteúdo de conversa não confiável os reescreva.

## Cabinet

O comando de console abaixo convoca personas nomeadas para a próxima deliberação Cabinet elegível:

```text
/cabinet <persona1> [persona2 ...]
```

Cabinet serve para trade-offs, não para consenso falso. Ele pode preservar discordâncias enquanto produz uma recomendação sintetizada; isso ajuda quando prioridades concorrentes precisam ficar explícitas e ser reconsideradas.

Exemplos:

```text
/cabinet carreira saude financas
/cabinet dono-do-projeto tech-lead
```

Use personas para dar um lar durável ao contexto. Use Cabinet quando esses contextos devem discordar antes de você decidir.
