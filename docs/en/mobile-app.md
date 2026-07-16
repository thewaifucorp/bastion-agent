# Mobile companion

The `mobile/` directory is Bastion’s Flutter companion application. It talks to the core’s webhook/mobile surface; it is not a hosted Bastion service and it does not replace the core runtime.

## Develop the app

```bash
cd mobile
flutter pub get
flutter run
```

The repository includes Android, iOS, macOS, Linux, web, and Windows platform directories. Use the platform tooling required by Flutter for the target you select.

## Connect it safely

The core’s mobile route is served by the webhook router. In the supplied Compose configuration, that means configuring `BASTION_WEBHOOK_ADDR` and a private, strong `APP_JWT_SECRET`; the core publishes port `8080` by default.

Pair against an instance you control. Keep it on a trusted local or private network during development, and add an intentional access-control layer before exposing it through the internet. The pairing path uses one-time-code exchange; do not replace it with a shared, long-lived token.

See [Channels](channels.md) and [Security](security.md) before deploying the companion beyond a local test environment.
