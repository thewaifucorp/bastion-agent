# App Mobile

## O que é o app mobile do Bastion?

O app mobile é um cliente para iOS e Android que se conecta ao **seu** Bastion — o que está rodando no seu computador ou servidor. Ele não armazena nenhum dado: é só uma interface para conversar com o seu agente de qualquer lugar.

O modelo é parecido com apps como Jellyfin ou Nextcloud: você baixa o app, aponta para a sua instância, e pronto.

---

## Como conectar

A conexão é feita por um código temporário gerado pelo próprio Bastion. Você não precisa configurar nada manualmente no app.

### Passo 1 — Gere o código de conexão

No Telegram ou WhatsApp, envie:

```
/connect-app
```

O Bastion vai responder com um código no formato `BAST-XXXX-XXXX` e um QR code. Esse código é válido por **5 minutos** e pode ser usado **uma única vez**.

### Passo 2 — Abra o app e cole o código

No app mobile, na tela de configuração, cole o código `BAST-XXXX-XXXX` ou escaneie o QR code.

### Passo 3 — Pronto

O app se conecta ao seu Bastion e gera um token de acesso que fica salvo com segurança no seu celular. Você não precisa repetir esse processo — o token dura 90 dias.

---

## Gerenciando dispositivos conectados

Para ver quais dispositivos estão conectados ao seu Bastion:

```
/devices
```

O Bastion lista todos os dispositivos com nome e status.

Para desconectar um dispositivo específico:

```
/revoke meu-iphone
```

O acesso é revogado imediatamente. Se alguém tiver o app aberto naquele dispositivo, a próxima requisição vai ser rejeitada.

---

## Segurança

- O código de conexão expira em 5 minutos e só funciona uma vez — mesmo que não tenha sido usado, não pode ser reutilizado após expirar
- O token de acesso fica salvo no keychain (iOS) ou keystore (Android), nunca em armazenamento não seguro
- Se você perder o celular, use `/revoke` pelo Telegram ou WhatsApp para revogar o acesso imediatamente
- O app nunca armazena suas conversas — tudo fica no seu Bastion

---

## Onde baixar o app

O app está disponível na App Store (iOS) e Google Play (Android). Busque por "Bastion AI" ou acesse [bastion.ai/app](https://bastion.ai/app).
