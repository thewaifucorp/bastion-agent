# Modelo de segurança

O Bastion torna confiança, autoridade e egress explícitos. Isso não transforma um agente com credenciais amplas em algo sem risco: as escolhas de deploy ainda determinam o que o processo consegue alcançar.

## Salvaguardas do produto

- **Canais restritos por identidade:** adaptadores mapeiam o remetente a um proprietário explícito; desconhecidos são rejeitados.
- **Classificação de confiança:** mensagens públicas de Discord/Slack e todo e-mail recebido são conteúdo não confiável.
- **Entrada WhatsApp assinada:** o caminho valida HMAC do corpo bruto antes do JSON.
- **Limite de capacidades:** ferramentas passam pelo registro de capacidades do runtime.
- **Isolamento de sidecars:** no Compose, sidecars Python vivem na rede interna; apenas o core participa da rede com egress.
- **Higiene de segredos:** construtores de canal evitam logar tokens, e `.env` é ignorado pelo Git.

## Responsabilidades de quem opera

1. Use credenciais distintas e revogáveis para cada integração.
2. Mantenha `APP_JWT_SECRET` forte e privado quando webhook/mobile estiver ativo.
3. Restrinja a porta `8080` por bind local, firewall, rede privada ou proxy autenticado.
4. Mapeie apenas proprietários conhecidos em `bastion.toml`.
5. Revise toda skill ou extensão de terceiros como código executável.
6. Avalie o impacto de privacidade de providers e telemetria.

Se um segredo vazar, revogue-o no provider, substitua-o no cofre de segredos, reinicie o serviço afetado e revise logs sem copiar conteúdo sensível para uma issue. Para vulnerabilidades do produto, siga o canal privado indicado em [CONTRIBUTING.md](../../CONTRIBUTING.md).

Nenhuma configuração garante segurança para uma instância pública e com privilégios excessivos. Comece com permissões estreitas e amplie apenas depois de observar o fluxo exato que você quer automatizar.
