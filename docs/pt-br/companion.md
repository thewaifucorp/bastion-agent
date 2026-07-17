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

Dentro da TUI, `/theme <rgb|cyan|blue|magenta|amber|green|mono>` troca o tema
na hora e `/theme #RRGGBB` define um accent customizado. A escolha persiste em
`~/.config/bastion/tui.json` (precedência: `bastion.toml` < `tui.json` <
variáveis de ambiente).

## Mascote nativo

A família nativa agora é o Keeper em pixel art (capacete de aço, visor escuro,
rosto que muda por estado) e o Patchwork, que assume quando o Cabinet é
convocado. Estados extras locais: um comando de barra desconhecido mostra o
Keeper em dúvida (olhos abertos e uma interrogação âmbar) e `/pet sleep`
mostra o rosto de descanso com zzz.

Em terminais com protocolo gráfico (Kitty, WezTerm, Ghostty, foot, Konsole,
iTerm2 e VS Code com `terminal.integrated.enableImages`), o mascote é
desenhado como imagem de verdade — pixel art idêntica ao mark do README,
pequena e nítida, com o selo de estado (escudo, ?, !, zzz…) num slot fixo à
direita da cabeça. Sem protocolo (terminal do Zed, GNOME Terminal padrão),
cai para um rosto mínimo de glifos — só os olhos e o selo, nítido em
qualquer fonte. `BASTION_TUI_GRAPHICS=off` força o modo texto.

## Cuidado e progressão

O game mode recompensa turnos concluídos, nunca volume de tokens. Turnos de
build e Cabinet rendem um pouco mais de XP cosmético, mas níveis nunca concedem
ferramentas, permissões ou bypass de políticas. A entrada humana também
preenche um medidor de momentum ao vivo: cada 80 caracteres vira 1 XP no envio,
com limite de 3 XP por mensagem. Uma resposta concluída da IA contribui com 1
XP fixo, independentemente da quantidade de tokens ou caracteres.

Dentro da TUI:

```text
/pet stats
/pet game on
/pet feed apple
/pet water
/pet play puzzle
/pet sleep nap
/pet use caminho/pet.toml
```

Digite um espaço depois de `/pet feed`, `/pet play` ou `/pet sleep` para abrir
os menus de escolha com emojis. A despensa tem oito comidas, `play` tem oito
atividades e o descanso tem quatro durações; cada opção restaura uma combinação
diferente dos medidores de água, comida, diversão e descanso mostrados por
`/pet stats`.

As necessidades avançam apenas durante tempo ativo observado; intervalos
ociosos acima de cinco minutos já contam como pausa. A água também funciona
como lembrete gentil para o dev. O companion nunca morre, perde XP ou culpa o
usuário.

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
