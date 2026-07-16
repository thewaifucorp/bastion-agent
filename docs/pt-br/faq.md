# FAQ

## Bastion é um serviço hospedado?

Não. Bastion é um runtime de produto self-hosted construído sobre `bastion-core`. Você escolhe host, canais, configuração de provider e controles operacionais.

## Onde fica o estado?

Sessões usam o caminho SQLite configurado em `session.db_path`. O deploy Compose também cria volumes nomeados para core e sidecars locais. Faça backup desses volumes como dados da aplicação.

## Por que um canal ignora uma mensagem?

O remetente pode não estar mapeado em `[[identity]]`, o canal pode estar desabilitado ou a credencial pode estar ausente. Verifique logs estruturados sem expor tokens. Veja [Canais](canais.md).

## Posso usar o app mobile remotamente?

Sim, mas apenas depois de tornar a superfície webhook/mobile acessível por um deploy que você controla e protege. Não exponha a porta padrão sem plano explícito de rede e controle de acesso. Veja [App mobile](app-mobile.md).

## Bastion suporta MCP?

Sim. Ele compõe clientes MCP e um build com `mcp-server` pode expor `bastion mcp-stdio` para transporte stdio local.

## Como o Bastion melhora?

O caminho de escrita de skills pode identificar padrões de trabalho concluído como candidatos a skills reutilizáveis. Os candidatos entram em fila de aprovação; não são aplicados silenciosamente.
