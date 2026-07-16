# App mobile

O diretório `mobile/` contém o aplicativo companion Flutter do Bastion. Ele conversa com a superfície webhook/mobile do core; não é um serviço Bastion hospedado e não substitui o runtime principal.

## Desenvolva o app

```bash
cd mobile
flutter pub get
flutter run
```

O repositório inclui diretórios de plataforma para Android, iOS, macOS, Linux, web e Windows. Use as ferramentas de plataforma exigidas pelo Flutter para o alvo escolhido.

## Conecte com segurança

A rota mobile do core é atendida pelo roteador webhook. No Compose fornecido, isso exige configurar `BASTION_WEBHOOK_ADDR` e um `APP_JWT_SECRET` forte e privado; por padrão, o core publica a porta `8080`.

Pareie apenas com uma instância que você controla. Durante desenvolvimento, mantenha-a em rede local ou privada; antes de expor na internet, adicione uma camada deliberada de controle de acesso. O pareamento usa troca por código de uso único; não a substitua por token compartilhado e duradouro.

Leia [Canais](canais.md) e [Segurança](seguranca.md) antes de levar o companion além de um teste local.
