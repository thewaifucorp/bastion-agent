# Companion de terminal

A TUI do Bastion trata o mascote como estado visível do runtime, não como
adesivo. A família nativa usa o Terminal Familiar no onboarding, o Keeper nos
estados normais de guarda/pensamento e o Patchwork em build e Cabinet. O painel
fixo se compacta automaticamente em terminais pequenos.

## Tema

Configure o terminal no `bastion.toml`:

```toml
[tui]
theme = "rgb" # rgb, cyan, blue, magenta, amber, green, mono
# accent = "#68d9ff"
mascot = true
animations = true
game = false
# pet = "extensions/acme/keeper/ui/pet.toml"
```

`BASTION_TUI_THEME`, `BASTION_TUI_ACCENT`, `BASTION_TUI_MASCOT`,
`BASTION_TUI_ANIMATIONS`, `BASTION_TUI_GAME` e `BASTION_TUI_PET` sobrescrevem
esses valores exclusivos da TUI. `NO_COLOR` desativa a paleta RGB.

## Cuidado e progressão

O game mode recompensa turnos concluídos, nunca volume de tokens. Turnos de
build e Cabinet rendem um pouco mais de XP cosmético, mas níveis nunca concedem
ferramentas, permissões ou bypass de políticas.

Dentro da TUI:

```text
/pet stats
/pet game on
/pet feed
/pet water
/pet play
/pet sleep
/pet use caminho/pet.toml
```

Alimentar, brincar e dormir formam o ciclo de cuidado do companion; a água
também funciona como lembrete gentil para o dev. Necessidades avançam apenas durante tempo
ativo observado; intervalos ociosos acima de cinco minutos já contam como
pausa. O companion nunca morre, perde XP ou culpa o usuário. `play` pode ser
alongar, caminhar, respirar, fazer um check-in ou qualquer reset curto — não
apenas Pomodoro.

## Sessões de outros agentes

Claude, Codex, OpenCode e outras integrações podem reportar eventos genéricos
sem depender dos binários deles:

```bash
bastion companion event session-start --source codex
bastion companion event activity --source codex
bastion companion event session-stop --source codex
```

O servidor MCP também expõe a ferramenta local `bastion_companion_event`, com
os argumentos `event` e `source`. Hooks/plugins ainda precisam chamar uma dessas
pontes; o Bastion não finge observar um processo independente sem integração.

## Pet packs customizados

Um pet é um asset TOML estrito e somente de dados, com sete estados animados
obrigatórios: `onboarding`, `guard`, `thinking`, `build`, `cabinet`, `success` e
`alert`. Frames têm no máximo 21 colunas por 6 linhas, caracteres de controle e
ANSI são rejeitados, e o arquivo não executa código nem solicita autoridade.

Use a skill incluída `bastion-pet-pack` para gerar e validar um pack. Uma
extension pode distribuir `ui/pet.toml` como `Provided::Ui("pet")` com conjunto
de permissões vazio.
