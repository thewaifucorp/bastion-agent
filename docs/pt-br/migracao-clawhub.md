# Migração de skills do ClawHub

Uma skill importada é código e configuração de terceiros, não apenas texto para o agente. Antes de trazê-la para um ambiente Bastion, leia a fonte, identifique credenciais, rede, escrita em disco e subprocessos, e confirme que ela cabe no modelo de capacidades que você quer operar.

## Processo recomendado

1. Instale ou copie a skill em um ambiente descartável.
2. Revise `SKILL.md`, manifestos, dependências e scripts antes de habilitá-la.
3. Remova credenciais embutidas e guarde segredos somente no cofre do deploy.
4. Execute testes ou uma interação sem privilégios contra dados não sensíveis.
5. Promova-a para a instância principal somente após revisar permissões e comportamento de egress.

O diretório `skills/` é montado como leitura/escrita apenas para o serviço que precisa criar skills; outros serviços recebem acesso mais restrito no Compose. Preserve essa separação ao adaptar uma skill.
