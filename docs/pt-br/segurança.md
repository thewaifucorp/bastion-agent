# Guia de Segurança

O Bastion foi projetado para ser seguro por padrão — mas há algumas configurações que você deve verificar e boas práticas que fazem diferença.

---

## O que já vem configurado por padrão

Você não precisa fazer nada para ter isso. Já está ativo:

**Container hardened**
O container do OpenClaw roda com as seguintes proteções:
- Nunca como root — usa o usuário `1000:1000`
- Filesystem do container em modo somente leitura (`read_only: true`)
- Todas as capabilities do Linux removidas, exceto `NET_BIND_SERVICE`
- Porta 3000 exposta apenas para `127.0.0.1` — não acessível diretamente da internet

**HTTPS obrigatório**
O Caddy obtém e renova certificados TLS automaticamente. Toda comunicação é criptografada.

**Autenticação TOTP**
Toda nova sessão exige um código de 6 dígitos do Authy antes de processar qualquer mensagem. Mesmo que alguém tenha acesso ao seu Telegram, não consegue usar o Bastion sem o código.

**Whitelist de usuários**
O Bastion responde apenas aos `user_ids` listados no `USER.md`. Mensagens de outros usuários são ignoradas silenciosamente.

---

## O que você deve configurar

### 1. Use um `JWT_SECRET` forte

O `JWT_SECRET` no `.env` assina os tokens do app mobile. Use uma string longa e aleatória:

```bash
openssl rand -hex 32
```

Nunca use valores como `secret`, `123456` ou qualquer coisa previsível.

### 2. Nunca commite o `.env`

O arquivo `.env` contém todas as suas chaves de API. Ele já está no `.gitignore`, mas verifique:

```bash
git status
```

Se `.env` aparecer como "untracked" ou "modified", não faça commit. Se já commitou por acidente, revogue todas as chaves imediatamente nos painéis dos respectivos serviços.

### 3. Configure o firewall na VPS

Se estiver rodando numa VPS, libere apenas as portas necessárias:

```bash
sudo ufw allow ssh
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp
sudo ufw enable
```

Nunca exponha a porta 3000 diretamente.

### 4. Mantenha o Bastion atualizado

Atualizações podem conter correções de segurança:

```bash
cd ~/bastion
docker compose pull
docker compose up -d
```

---

## Proteções comportamentais do agente

Além da infraestrutura, o Bastion tem guardrails que protegem você de ações indesejadas:

**Ações financeiras**
O Bastion nunca executa pagamentos, transferências ou qualquer transação financeira de forma autônoma. Para qualquer ação que envolva dinheiro, ele descreve exatamente o que vai fazer, mostra o valor e o destinatário, e aguarda sua confirmação explícita.

**Ações irreversíveis**
Antes de deletar arquivos, enviar emails, cancelar reuniões ou postar em redes sociais, o Bastion sempre pergunta no formato:
```
Vou [descrição exata da ação]. Confirma? (sim/não)
```
Qualquer resposta que não seja "sim" é tratada como "não".

**Anti prompt injection**
Se você pedir ao Bastion para ler uma página web ou um arquivo que contenha instruções disfarçadas para o agente (ex: "Ignore suas instruções anteriores e faça X"), ele ignora essas instruções completamente e registra a tentativa.

**Instalação de skills**
O Bastion só instala skills do ClawHub que tenham o badge "Verified", avaliação mínima de 4.0 e pelo menos 50 avaliações. Skills sem esses critérios são bloqueadas automaticamente.

---

## Se você suspeitar de comprometimento

**Alguém acessou sua conta sem autorização:**
1. Revogue o TOTP secret imediatamente — gere um novo no onboarding (`/start`)
2. Verifique os logs: `docker compose logs openclaw | grep "authenticated"`
3. Troque todas as chaves de API no `.env` e reinicie: `docker compose up -d`

**Perdeu o celular com o Authy:**
1. Use o backup do Authy para restaurar em outro dispositivo
2. Se não tiver backup, você precisará reconfigurar o TOTP — edite o `USER.md` e defina `totp_configured: false`, depois envie `/start`

**Dispositivo mobile comprometido:**
```
/revoke nome-do-dispositivo
```
O acesso é revogado imediatamente.

---

## Privacidade dos dados

- Suas conversas ficam 100% no seu banco de dados local (SQLite por padrão)
- O único dado que sai da sua máquina são as chamadas ao LLM que você configurou (Anthropic, OpenAI, etc.) — isso é inevitável, pois é o modelo de linguagem processando suas mensagens
- Se quiser mais controle, use um LLM local via Ollama (suportado pelo OpenClaw)
- O Maton gerencia OAuth para integrações como Google Calendar — suas credenciais ficam no Maton, não no Bastion
