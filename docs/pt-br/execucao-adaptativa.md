# Execução adaptativa

O Bastion encaminha cada solicitação para um de três ciclos de vida progressivos.
O modo é escolhido a cada solicitação, mas você sempre pode substituí-lo. O
vocabulário (`Respond`/`Act`/`Pursue`, `TaskCase`/`Attempt`/`Evidence`/`Verdict`)
é definido pelo [bastion-core](https://github.com/thewaifucorp/bastion-core): o
kernel fornece o mecanismo; o agente controla a ativação e a experiência de uso.

> **Legenda de status:** ✅ implementado · 🧪 experimental/parcial · 🕓 planejado.

## Os três modos

| Modo | O que faz | Registro durável? |
| --- | --- | --- |
| **Respond** | Responde a partir de crenças e contexto, sem efeito externo. É o caminho padrão e mais barato. | Não — nunca cria um `TaskCase`. |
| **Act** | Executa um único efeito delimitado, sem continuidade após o turno. | Apenas efêmero, se aprovação ou recuperação exigir. |
| **Pursue** | Mantém um objetivo durável e retomável, com efeitos dependentes, decomposição ou adaptação. | Sim — um `TaskCase` que sobrevive a reinícios. |

`Respond` não adiciona uma chamada de LLM por padrão; somente `Pursue` persiste
um caso durável. Cada seleção de modo emite telemetria para tornar custo e
decisão auditáveis.

## Ativação e substituição

- ✅ **Console:** a entrada na TUI/no console é classificada antes do turno.
- ✅ **Agendador:** intenções disparadas por agendamentos usam a mesma seleção.
- ✅ **Canais de entrada:** Telegram, e-mail e outros canais escolhem o modo
  antes da execução.
- ✅ **Substituição:** a classificação é uma sugestão; o aviso curto explica
  como substituir o modo daquela solicitação.

## Pursue: cockpit de tarefas

Solicitações `Pursue` viram `TaskCase`s duráveis, inspecionáveis e controláveis
com `/task`:

```text
/task                       # lista suas tarefas abertas (igual a `list`)
/task inspect <id>          # mostra tentativas, evidências e veredito
/task pause <id>            # pausa uma tarefa em execução
/task resume <id>           # retoma uma tarefa pausada
/task steer <id> <texto>    # injeta orientação em uma tarefa em curso
/task cancel <id>           # cancela e registra o motivo Cancelled
```

Cada tarefa é isolada pelo dono. Ela registra `Attempt`s, `Evidence`s e o
`Verdict` do verificador; o próximo passo é recalculado a cada observação, sem
um plano/DAG armazenado.

## Agendamentos, capacidades e orçamento

Os comandos `/schedule` criam agendamentos pessoais duráveis e isolados pelo
dono; eles sobrevivem a reinícios e seguem o mesmo caminho adaptativo.

Dentro de uma tarefa, Bastion oferece navegador governado, runtimes externos
delegados para trabalho de código (com diffs e artefatos como evidência) e
decomposição em tarefas-filhas concorrentes — sem DAG central. Conteúdo de
páginas é dado não confiável, nunca instrução; efeitos sensíveis exigem
aprovação e downloads têm proteção contra SSRF e symlinks inseguros.

Cada modo tem orçamento e telemetria próprios. Cada chamada de LLM recebe motivo
e modo atribuídos, permitindo auditar posteriormente o custo da tarefa.

## Ainda não nesta versão

- Avaliação de resultado offline por um serviço juiz separado.
- Promoção de procedimentos aprendidos para skills/regras compartilhadas.
