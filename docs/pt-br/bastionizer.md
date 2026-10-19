# Bastionizer (sem daemon)

O Bastionizer é um conjunto de skills que faz qualquer agente (Claude Code, Codex, …)
agir como o Bastion **sem rodar o Bastion**: personas por área da vida,
memória local, lembretes, skills que o próprio agente escreve e melhora,
Cabinet e um companion na status line. Tudo fica em `~/.bastion`
(`$BASTION_HOME`). O único código é um script Python 3.9+ só com a biblioteca
padrão (SQLite com FTS5 já vem nela). Sem servidor, sem processo em segundo
plano, sem rede.

É diferente do [plugin do Claude Code](../../integrations/claude-code/README.md), que
liga o Claude Code a um daemon do Bastion rodando (aprovações pelo celular,
aparelho de mesa, memória compartilhada).

## Skills

| Skill | O que o agente ganha |
|---|---|
| `bastion` | Quem ele é e o ciclo: rotear → lembrar → agir → guardar → aprender. Brief da sessão. |
| `bastion-personas` | Criar, rotear, trocar e pesar personas (`SOUL.md`, mesmo contrato do Bastion). |
| `bastion-memory` | Memórias em SQLite + FTS5 (BM25, importância, recência), grafo de conhecimento, esquecimento suave; busca nas conversas antigas. |
| `bastion-skill-forge` | Escreve uma skill depois de uma tarefa que deu certo; registra resultados; corrige e versiona skills que falharam. |
| `bastion-reminders` | Lembretes, repetição, soneca, follow-ups proativos, notificação no desktop (opcional). |
| `bastion-cabinet` | Personas deliberam sobre um trade-off, com dissenso preservado e recomendação ponderada. |

## Instalar

Claude Code (skills + hooks + comandos):

```bash
claude plugin marketplace add /caminho/para/bastion-agent/extras/bastionizer
claude plugin install bastionizer@bastionizer
```

Companion na status line (`~/.claude/settings.json`), depois da primeira sessão:

```json
{ "statusLine": { "type": "command", "command": "python3 ~/.bastion/bin/bastion.py buddy" } }
```

Outro agente: copie as seis pastas de `extras/bastionizer/skills/` para a pasta de
skills dele, lado a lado (elas acham o CLI em `../bastion/scripts/bastion.py`).

## Privacidade

Personas são `cloud-ok` ou `local-only`. Memórias de uma persona `local-only`
são privadas: os hooks nunca as injetam e a busca as esconde, a não ser que
você peça. O Bastionizer não escolhe o modelo do agente: se ele roda na nuvem, o que
o agente lê vai para lá. Não guarde segredos na memória, nas personas nem nas
skills.

Detalhes, comandos e limites: [extras/bastionizer/README.md](../../extras/bastionizer/README.md).
