---
name: bastion/mobile-connect
version: 1.0.0
description: >
  Generates one-time connection tokens for the Bastion mobile app, manages
  device JWT lifecycle, and handles device revocation.
triggers:
  - /connect-app
  - /devices
  - /revoke
---

# bastion/mobile-connect

Connects the Bastion mobile app (iOS/Android) to the user's self-hosted Bastion instance using a secure one-time token exchange flow.

## Triggers

| Trigger | Action |
|---------|--------|
| `/connect-app` | Generate a one-time token (`BAST-XXXX-XXXX`) valid for 5 minutes and display it (plus QR code) for the user to enter in the mobile app |
| `/devices` | List all currently connected devices with name and connection timestamp |
| `/revoke {device_name}` | Immediately revoke the JWT for the specified device |

## Flow: /connect-app

```
1. User types /connect-app in Telegram/WhatsApp
2. Bastion calls generateConnectToken() → BAST-XXXX-XXXX
3. Token is stored in pendingTokens with 5-minute TTL
4. Bastion displays the token as text + QR code
5. User opens the mobile app and enters the token
6. App sends POST /auth/exchange { token, device_name }
7. Bastion validates TTL, deletes token immediately (one-time use)
8. Bastion generates JWT (exp: 90 days) and returns it
9. App stores JWT in keychain (iOS) / keystore (Android)
10. Connection established — app uses JWT for all subsequent requests
```

## Flow: /devices

```
1. User types /devices
2. Bastion calls GET /devices
3. Returns list of connected (non-revoked) devices with name and connected_at
4. Bastion formats and displays the list to the user
```

## Flow: /revoke {device_name}

```
1. User types /revoke my-iphone
2. Bastion calls DELETE /devices/my-iphone
3. Device is added to the in-memory revocation blocklist immediately
4. Any subsequent request using that device's JWT is rejected with 401
5. Bastion confirms revocation to the user
```

## Security Notes

- Tokens are one-time use: deleted immediately upon successful exchange
- Token TTL is 5 minutes — expired tokens are rejected
- JWT_SECRET is read from environment variable, never hardcoded
- JWTs expire after 90 days; users must reconnect after expiry
- Revocation is immediate and in-memory (survives until process restart)
- The mobile app must store the JWT in the platform secure storage (keychain/keystore)

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/auth/exchange` | Exchange one-time token for JWT |
| `GET` | `/devices` | List connected devices |
| `DELETE` | `/devices/:device` | Revoke a device |

## Edge Cases

- If the user types `/connect-app` multiple times, each call generates a new independent token
- If the token expires before the user enters it, they must run `/connect-app` again
- If a device name is already connected, exchanging a new token with the same name overwrites the previous entry
- Revocation persists only in memory — a process restart clears the blocklist (production deployments should use Redis)
