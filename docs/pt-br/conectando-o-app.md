# Conectando o App Mobile

O app mobile do Bastion é um cliente para iOS e Android que se conecta diretamente à **sua** instância — seja rodando no seu computador ou numa VPS. Ele não armazena nenhum dado: é só uma interface para conversar com o seu agente de qualquer lugar.

---

## Antes de começar

Para conectar o app, o Bastion precisa estar acessível pela internet. Isso significa:

- **Rodando numa VPS com domínio configurado** — recomendado. Veja [vps-setup.md](vps-setup.md).
- **Rodando localmente com túnel** — use [ngrok](https://ngrok.com) ou [Cloudflare Tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/) para expor temporariamente.

Se o Bastion só está acessível na sua rede local (Wi-Fi de casa), o app só vai funcionar quando você estiver conectado a essa rede.

---

## Como conectar

### Passo 1 — Gere o código de conexão

No Telegram ou WhatsApp, envie:

```
/connect-app
```

O Bastion responde com:
- Um código no formato `BAST-XXXX-XXXX`
- Um QR code para escanear diretamente pelo app

O código é válido por **5 minutos** e pode ser usado **uma única vez**.

### Passo 2 — Abra o app

Baixe o app na [App Store](https://apps.apple.com) ou [Google Play](https://play.google.com) — busque por "Bastion AI".

Na tela inicial, toque em "Conectar ao meu Bastion".

### Passo 3 — Cole o código ou escaneie o QR

Você tem duas opções:
- **Digitar o código**: cole o `BAST-XXXX-XXXX` no campo indicado
- **Escanear o QR**: toque no ícone de câmera e aponte para o QR code

### Passo 4 — Informe o endereço do seu Bastion

O app vai pedir o endereço da sua instância. Use o domínio que você configurou:

```
https://bastion.seudominio.com
```

Se estiver usando localmente com ngrok, use o endereço gerado pelo ngrok.

### Passo 5 — Pronto

O app se conecta, valida o código e salva um token de acesso no keychain (iOS) ou keystore (Android). Você não precisa repetir esse processo — o token dura **90 dias**.

---

## Gerenciando dispositivos conectados

Para ver todos os dispositivos com acesso ao seu Bastion:

```
/devices
```

Exemplo de resposta:
```
Dispositivos conectados:
• meu-iphone — conectado há 3 dias
• ipad-trabalho — conectado há 12 dias
```

Para revogar o acesso de um dispositivo:

```
/revoke meu-iphone
```

O acesso é revogado imediatamente. Se o app estiver aberto naquele dispositivo, a próxima mensagem vai ser rejeitada.

---

## Segurança

- O código `BAST-XXXX-XXXX` expira em 5 minutos e só funciona uma vez — mesmo que não tenha sido usado, não pode ser reutilizado após expirar
- O token de acesso fica salvo no keychain (iOS) ou keystore (Android), nunca em armazenamento não seguro
- Se perder o celular, use `/revoke` pelo Telegram ou WhatsApp para revogar o acesso imediatamente
- O app nunca armazena suas conversas — tudo fica no seu Bastion

---

## Problemas comuns

**"Código inválido ou expirado"**
O código dura 5 minutos. Gere um novo com `/connect-app` e tente novamente.

**"Não foi possível conectar ao servidor"**
Verifique se o endereço está correto e se o Bastion está acessível. Teste no navegador: `https://bastion.seudominio.com` deve responder.

**O app conectou mas não responde**
Verifique se a sessão TOTP está ativa. O app mobile também precisa de autenticação TOTP em sessões novas — o app vai solicitar o código automaticamente.
