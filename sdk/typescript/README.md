# @bastion/control-plane-sdk

TypeScript client for Bastion's External Control Plane API (`/v1/*`). See
`docs/en/contracts/control-plane-v1.openapi.yaml` (repo root) for the frozen
wire contract this SDK is a typed client for, and
`docs/en/control-plane-security.md` for the threat model.

## Install (from a local checkout)

```bash
cd sdk/typescript
npm install
npm run build
```

## Usage

```ts
import { BastionClient } from "@bastion/control-plane-sdk";

const client = new BastionClient({
  baseUrl: "http://127.0.0.1:8080",
  token: process.env.BASTION_TOKEN!, // bcp_<opaque> — never ship this to a browser bundle
});

const task = await client.createTask({ objective: "Fix the auth bug" });
await client.pauseTask(task.id, task.revision);

for await (const t of client.tasks({ status: "running" })) {
  console.log(t.id, t.objective);
}
```

## Browser vs Node

One class, `BastionClient`, works in both — it's built on the global
`fetch`/`crypto`, no Node-only imports anywhere in `src/`. The split the
planning doc asks for ("browser-safe read client... no secret accepted in
browser bundles by default") is a **usage discipline**, not two classes: a
token is required for every `/v1/tasks*`/`/v1/webhook-subscriptions` call (a
scoped Control Plane credential, `src/control_plane/credential.rs`), so a
truly browser-safe deployment proxies authenticated calls through your own
backend rather than embedding the token client-side. The constructor warns
(`console.warn`) if it detects both a browser-like `window` global and a
token, to catch that mistake early — it does not hard-block it, since a
short-lived, narrowly-scoped token deliberately handed to a browser is a
legitimate (if unusual) choice only the caller can make.

## Verifying inbound webhook deliveries

```ts
import { verifyWebhookSignature } from "@bastion/control-plane-sdk";

// `rawBody` MUST be the exact bytes received — see verifyWebhookSignature's
// doc comment on why a re-serialized copy fails verification.
const ok = await verifyWebhookSignature(subscriptionSecret, rawBody, req.headers["x-bastion-signature"]);
```

## Testing

```bash
npm test
```

Uses Node's built-in test runner (`node:test`) via `tsx` — no Jest/Vitest
dependency. `test/client.test.ts` spins up a real local HTTP server as a
mock Bastion API (`node:http`), so requests are exercised against actual
`fetch` calls, not a mocked `fetch` implementation.
