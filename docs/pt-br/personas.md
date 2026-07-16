# Personas

## O que é uma persona?

Uma persona é um perfil de comportamento que o Bastion usa para se adaptar ao contexto da sua conversa. Cada persona tem um nome, um domínio de atuação, um tom de voz e palavras-chave que a ativam automaticamente.

Por exemplo: se você tem uma persona "Tech Lead" com a keyword "PR", toda vez que você mencionar "PR" ou "code review" numa mensagem, o Bastion vai responder com o comportamento e o tom dessa persona — mais técnico, focado em código e arquitetura.

Você pode ter quantas personas quiser. Elas podem estar ativas ao mesmo tempo se a mensagem tocar em múltiplos contextos.

---

## Como as personas são criadas

No onboarding (quando você envia `/start` pela primeira vez), o Bastion pergunta quais áreas da sua vida você quer que ele ajude. Para cada área que você informar, ele cria uma persona automaticamente.

Se você informou "trabalho", "estudos" e "saúde", o Bastion cria três personas — uma para cada área — e já sugere ferramentas relevantes para instalar em cada uma.

---

## Como criar uma nova persona

Basta pedir ao Bastion em linguagem natural:

> "Quero criar uma nova persona para minha banda"

O Bastion vai conduzir uma conversa curta perguntando:

1. **Nome** da persona (ex: "Músico")
2. **Domínio** — o que essa persona cobre (ex: ensaios, composição, shows)
3. **Tom de voz** — formal ou informal, direto ou detalhado
4. **Keywords** — palavras que ativam essa persona (ex: "ensaio", "música", "show", "acorde")
5. **Peso base** — a prioridade dessa persona em relação às outras (0.0 a 1.0)

Depois de confirmar, o Bastion cria a persona e ela já fica disponível.

---

## Como editar uma persona existente

Peça diretamente:

> "Quero editar a persona Tech Lead"
> "Adiciona a keyword 'deploy' na minha persona de trabalho"
> "Muda o tom da persona Estudante para mais informal"

O Bastion faz a alteração e confirma o que mudou.

---

## Como funciona a ativação automática

O Bastion analisa cada mensagem que você envia e verifica:

- Se alguma palavra da mensagem bate com as keywords de alguma persona
- O contexto geral da mensagem (mesmo sem keyword exata)
- O horário do dia, se você configurou horários para alguma persona

Se nenhuma persona for identificada, o Bastion usa a persona com maior peso atual como padrão.

---

## Pesos de persona

Cada persona tem um peso (de 0.0 a 1.0) que representa a prioridade dela. O peso pode aumentar temporariamente em situações de crise (veja [Modo Crise](crisis-mode.md)) e é ajustado automaticamente com base nos seus padrões de uso ao longo do tempo.

Você também pode ajustar manualmente:

> "Aumenta o peso da persona CEO para 0.8"
> "Diminui a prioridade da persona Inglês"

---

## Dicas

- Crie personas específicas, não genéricas. "Tech Lead" é melhor que "Trabalho".
- Escolha keywords que você realmente usa no dia a dia — palavras que aparecem naturalmente nas suas mensagens.
- Se o Bastion estiver usando a persona errada com frequência, adicione mais keywords à persona correta ou remova keywords ambíguas.
