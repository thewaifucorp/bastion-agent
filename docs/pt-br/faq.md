# Perguntas Frequentes

---

## Instalação e configuração

**Preciso saber programar para instalar o Bastion?**
Não. O instalador faz tudo automaticamente. Você só precisa saber abrir o terminal e editar um arquivo de texto.

**Funciona no Windows?**
Sim, com WSL2 (Windows Subsystem for Linux). Instale o WSL2, depois siga o guia de instalação normalmente dentro do ambiente Linux.

**Qual LLM devo usar?**
Qualquer um funciona. Para começar, Groq é gratuito e rápido. Para respostas mais elaboradas, Claude (Anthropic) ou GPT-4o (OpenAI) são melhores. Você pode configurar múltiplos e o OpenClaw faz o fallback automaticamente.

**O que é o Maton e por que preciso dele?**
O Maton é um gateway de integrações — ele gerencia o OAuth com serviços como Google Calendar, Notion, GitHub, etc. Sem ele, o Bastion não consegue acessar esses serviços. A conta gratuita do Maton é suficiente para uso pessoal.

**Posso usar sem Telegram? Só pelo app mobile?**
O Telegram (ou WhatsApp) é necessário para o onboarding inicial e para algumas configurações como `/connect-app`. Depois de configurado, você pode usar principalmente pelo app mobile.

---

## Personas e comportamento

**Quantas personas posso ter?**
Não há limite. Na prática, 3 a 6 personas é o que a maioria das pessoas usa.

**O Bastion pode usar a persona errada?**
Sim, especialmente no começo. Se isso acontecer com frequência, adicione mais keywords à persona correta ou remova keywords ambíguas. Você pode pedir diretamente: "Adiciona a keyword 'deploy' na minha persona Tech Lead".

**Posso ter duas personas ativas ao mesmo tempo?**
Sim. Se uma mensagem tocar em múltiplos contextos, o Bastion ativa todas as personas relevantes simultaneamente, cada uma com seu peso.

**O que acontece se eu não tiver nenhuma persona configurada?**
O Bastion usa um comportamento padrão neutro até você criar personas. O onboarding cria as primeiras automaticamente.

---

## Dados e privacidade

**Onde ficam meus dados?**
No seu computador ou VPS. O banco de dados SQLite fica em `~/bastion/db/life-log.db`. Suas personas ficam em `~/bastion/personas/`. Nada vai para servidores externos além das chamadas ao LLM.

**O LLM vê minhas conversas?**
Sim — as mensagens são enviadas ao LLM para processamento. Isso é inevitável. Se isso for uma preocupação, use um LLM local via Ollama (suportado pelo OpenClaw).

**Posso exportar meus dados?**
Sim. Tudo está em arquivos de texto (Markdown) e SQLite — formatos abertos que você pode ler e exportar com qualquer ferramenta.

**O que acontece se eu deletar o banco de dados?**
O Bastion perde o histórico de interações (life log), mas as personas e configurações ficam intactas — elas ficam nos arquivos Markdown em `~/bastion/personas/`.

---

## Segurança e autenticação

**O que é o TOTP e por que preciso dele?**
TOTP é o código de 6 dígitos que muda a cada 30 segundos no app Authy. Ele protege o Bastion mesmo que alguém tenha acesso ao seu Telegram — sem o código, não conseguem usar o agente.

**Esqueci de configurar o Authy durante o onboarding. O que faço?**
Envie `/start` novamente. O onboarding é idempotente — você pode refazer sem perder suas personas.

**Posso desativar o TOTP?**
Não é recomendado, mas é possível editando o `USER.md` e definindo `totp_configured: false`. Sem TOTP, qualquer pessoa com acesso ao seu Telegram pode usar o Bastion.

**Alguém pode usar o Bastion se tiver acesso ao meu Telegram?**
Não, se o TOTP estiver configurado. Eles precisariam também do código do Authy, que só existe no seu celular.

---

## App mobile

**O app funciona sem internet?**
Não. O app precisa se comunicar com o seu Bastion, que por sua vez precisa chamar o LLM. Sem internet, nada funciona.

**O token do app expira?**
Sim, após 90 dias. Quando expirar, gere um novo código com `/connect-app` e reconecte.

**Posso conectar vários celulares?**
Sim. Cada dispositivo tem seu próprio token. Use `/devices` para ver todos e `/revoke` para desconectar qualquer um.

---

## Problemas comuns

**O Bastion não responde no Telegram**
1. Verifique se os containers estão rodando: `docker compose ps`
2. Veja os logs: `docker compose logs -f openclaw`
3. Confirme que o `TELEGRAM_BOT_TOKEN` no `.env` está correto

**"Código TOTP inválido"**
Certifique-se de que o horário do seu celular está sincronizado. O TOTP depende do horário exato — uma diferença de mais de 30 segundos invalida o código.

**O Bastion está lento**
Provavelmente é latência do LLM. Tente trocar para um modelo mais rápido (Groq é o mais rápido). Verifique também se a VPS está sobrecarregada: `docker stats`.

**Perdi acesso ao Authy e não consigo autenticar**
1. Tente recuperar o Authy pelo backup na nuvem (se estava ativado)
2. Se não tiver backup, acesse o servidor diretamente via SSH, edite `~/bastion/USER.md` e defina `totp_configured: false`, depois reinicie: `docker compose restart openclaw`
3. Envie `/start` no Telegram para reconfigurar o TOTP
