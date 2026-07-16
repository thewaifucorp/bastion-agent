# Modo Crise

## O que é o Modo Crise?

O Modo Crise é uma funcionalidade do Bastion para situações de urgência real — quando algo importante quebrou, um prazo está chegando, ou você precisa liberar tempo imediatamente para resolver um problema.

Quando ativado, o Bastion:

1. Aumenta a prioridade da persona afetada
2. Analisa sua agenda e identifica compromissos que podem ser movidos ou cancelados
3. Libera pelo menos 2 horas de tempo para você focar no problema
4. Te mostra um resumo do que foi reorganizado

---

## Como ativar

Você pode ativar o Modo Crise de duas formas:

**Comando direto:**
```
/crise Servidor de produção caiu
/crise Apresentação amanhã e ainda não terminei o deck
```

**Linguagem natural de urgência:**
```
"Tá tudo pegando fogo aqui, o cliente principal cancelou o contrato"
"Emergência — preciso entregar isso hoje e não tenho tempo"
```

O Bastion detecta automaticamente quando a mensagem indica uma crise real (com alta confiança) e ativa o modo sem precisar do comando `/crise`.

---

## O que acontece depois

Quando o Modo Crise é ativado, o Bastion executa o **sacrifice algorithm**:

1. Identifica qual persona está sendo afetada pela crise
2. Aumenta o peso dessa persona em +0.3 (até o máximo de 1.0)
3. Busca na sua agenda tarefas que podem ser movidas — compromissos de baixa prioridade que não são urgentes
4. Cancela ou reagenda essas tarefas para liberar no mínimo 2 horas
5. Te envia um resumo: o que foi movido, para quando, e quanto tempo foi liberado

Exemplo de resposta:

```
🚨 Modo Crise ativado — persona: Tech Lead

Liberei 2h30 na sua agenda:
• Reunião de alinhamento semanal → movida para quinta às 14h
• Review de documentação → movida para sexta de manhã

Você tem das 14h às 16h30 livre hoje. Pode focar no servidor.
```

---

## E se não houver tarefas para mover?

Se o Bastion não encontrar compromissos suficientes para liberar 2 horas, ele te avisa e mostra as opções disponíveis — sem fazer nada sem a sua confirmação.

---

## Desativando o Modo Crise

O Modo Crise não tem duração fixa. Quando a situação se resolver, o peso da persona volta ao normal automaticamente na próxima revisão semanal. Você também pode pedir manualmente:

> "A crise passou, pode normalizar minha agenda"

---

## Dica

O Modo Crise funciona melhor quando você tem o Google Calendar conectado via Maton. Sem integração de agenda, o Bastion ainda pode ajudar a priorizar tarefas, mas não consegue reorganizar compromissos automaticamente.
