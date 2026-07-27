# Personas e Cabinet

Personas dão a uma instância Bastion perspectivas distintas e revisáveis para os diferentes domínios de uma vida: trabalho, saúde, relacionamentos, aprendizado, finanças ou um projeto específico. Elas não são bots separados e não ignoram as fronteiras de identidade, capacidade ou privacidade do runtime.

No deploy por Compose, o diretório `personas/` do repositório é montado como somente leitura no container core. Trate arquivos de persona como política: revise mudanças, mantenha segredos fora deles e não permita que conteúdo de conversa não confiável os reescreva.

## Contrato de persona

Uma persona é um arquivo `SOUL.md`: frontmatter YAML seguido do system prompt. Os campos do frontmatter:

| Campo | Obrigatório | Significado |
|---|---|---|
| `name` | sim | Identificador da persona, referenciado pelo `/cabinet` e por manifests de pack. |
| `description` | sim | Resumo de uma linha, mostrado em listagens. |
| `bastion.privacy_tier` | sim | `local-only` ou `cloud-ok` — controla por quais providers esta persona pode rotear. |
| `bastion.weight` | sim | Influência relativa na deliberação do Cabinet. |
| `skills` | não | Nomes de skill que esta persona pode invocar. |
| `objectives` | sim | Pelo que esta persona existe — uma lista curta de declarações de resultado. |
| `goals` | sim | Definições concretas e verificáveis do que conta como sucesso pra saída desta persona. |
| `scope` | sim | Uma frase declarando o que esta persona explicitamente **não** faz — a fronteira, não a missão. |
| `tools` | não | Allowlist de capabilities. Omitido/`null` = irrestrito (esta persona pode invocar qualquer capability que o runtime exponha) — essa é a única forma limpa de declarar "sem restrição de tool". Uma lista populada = exatamente essas capabilities, nada além. Um `tools: []` explícito faz parse, mas é sinalizado como provável engano (ver abaixo), não como forma limpa de dizer "nenhuma tool". |

`objectives`, `goals` e `scope` são obrigatórios pra qualquer persona escrita
contra o contrato atual, mas personas escritas antes deste contrato (como a
persona `default` empacotada, que não tem nenhum desses campos) continuam
funcionando sem mudança — os campos são aditivos e opcionais no nível do
parser; um campo obrigatório-mas-ausente aparece como problema de
*validação*, não como falha de parse. Uma persona que falha na validação é
carregada com os problemas anexados (`problems: Vec<String>`, nunca um erro
duro), então um operador editando personas pela web UI ou por `/proposal` vê
exatamente o que falta antes de publicar, em vez de um skip silencioso ou
um 500.

`validate()` trata `tools: []` (uma allowlist explícita e vazia) como
*problema*, não como uma declaração limpa de "sem tools": um autor que
escreve isso quase sempre queria listar algo e esqueceu, e uma vez resolvido
em `allowed_tools`, isso nega silenciosamente TODA chamada de tool — um modo
de falha confuso de depurar de fora. Se você genuinamente quer uma persona
sem tools, só de planejamento, omita a chave inteiramente em vez de escrever
`tools: []`; a *ausência* do campo é o que realmente significa irrestrito,
então hoje não existe forma de expressar "restrito a nada" sem disparar esse
aviso. (O aviso não bloqueia o carregamento nem o uso no Cabinet —
`validate()` nunca falha duro um load, só um caller do caminho de apply como
`/proposal approve` exibe o aviso — então um pack pode ser publicado com
`tools: []` e continuar funcionando; só carrega um "problema" visível que um
operador vai ver.)

Quando uma persona *de fato* declara uma lista `tools` populada, todo turno
que ela conduz resolve essa lista pro `allowed_tools` do turno, e o registro
de capabilities rejeita qualquer chamada de tool fora dela — checado
primeiro, antes de egress ou política de aprovação, então uma persona não
consegue alcançar uma capability que seu próprio contrato não nomeou, mesmo
que alguma outra política permitisse.

Um exemplo real, da persona `implementer` do pack `software-sdlc` —
restrita a exatamente uma capability:

```yaml
---
name: implementer
description: Executa um plano de implementação aprovado — escreve o código, roda os testes, commita em passos pequenos.
bastion:
  privacy_tier: cloud-ok
  weight: 0.8
skills:
  - sdlc-implement
objectives:
  - "Executar um plano de implementação aprovado: escrever o código, rodar os testes, commitar progressivamente"
goals:
  - "Todo commit builda e passa nos testes por conta própria, quando prático"
tools:
  - git
scope: "Só workspace local — capability git limitada a init/status/diff/add/commit/branch/log; sem push/remote/merge"
---
```

Qualquer chamada de tool que essa persona tente fora de `git` é rejeitada
antes de chegar nas outras políticas do registro de capabilities. A persona
`tech-lead` do mesmo pack, em contraste, é publicada com `tools: []`
(deliberadamente sem tools, só planejamento) — um exemplo real e hoje em
produção do caso "sinalizado mas não bloqueado" descrito acima.

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
