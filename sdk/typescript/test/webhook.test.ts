import { test } from "node:test";
import assert from "node:assert/strict";
import { createHmac } from "node:crypto";
import { verifyWebhookSignature } from "../src/webhook.js";

// Independent reference implementation (Node's own `node:crypto`, not the
// Web Crypto API `verifyWebhookSignature` itself uses) — this is what makes
// the test meaningful rather than circular: if `verifyWebhookSignature`'s
// Web Crypto HMAC ever disagreed with the standard HMAC-SHA256 Rust's
// `hmac`/`sha2` crates also implement (src/control_plane/webhook_delivery.rs's
// `sign_payload`), this catches it via a THIRD, independent computation.
function referenceSignature(secret: string, body: string): string {
  const hex = createHmac("sha256", secret).update(body).digest("hex");
  return `sha256=${hex}`;
}

test("verifyWebhookSignature accepts a signature computed by an independent HMAC-SHA256 implementation", async () => {
  const secret = "test-secret-123";
  const body = JSON.stringify({ event_type: "task.created", task_id: "t1" });
  const signature = referenceSignature(secret, body);

  const ok = await verifyWebhookSignature(secret, body, signature);
  assert.equal(ok, true);
});

test("verifyWebhookSignature rejects a tampered body", async () => {
  const secret = "test-secret-123";
  const body = JSON.stringify({ event_type: "task.created", task_id: "t1" });
  const signature = referenceSignature(secret, body);

  const tamperedBody = JSON.stringify({ event_type: "task.created", task_id: "t2" });
  const ok = await verifyWebhookSignature(secret, tamperedBody, signature);
  assert.equal(ok, false);
});

test("verifyWebhookSignature rejects the wrong secret", async () => {
  const body = "same body";
  const signature = referenceSignature("secret-a", body);
  const ok = await verifyWebhookSignature("secret-b", body, signature);
  assert.equal(ok, false);
});

test("verifyWebhookSignature rejects a missing header", async () => {
  const ok = await verifyWebhookSignature("secret", "body", undefined);
  assert.equal(ok, false);
});

test("verifyWebhookSignature rejects a header without the sha256= prefix", async () => {
  const ok = await verifyWebhookSignature("secret", "body", "deadbeef");
  assert.equal(ok, false);
});

test("verifyWebhookSignature accepts Uint8Array bodies identically to the equivalent string", async () => {
  const secret = "test-secret-123";
  const bodyStr = "raw bytes test";
  const bodyBytes = new TextEncoder().encode(bodyStr);
  const signature = referenceSignature(secret, bodyStr);

  const okFromBytes = await verifyWebhookSignature(secret, bodyBytes, signature);
  const okFromString = await verifyWebhookSignature(secret, bodyStr, signature);
  assert.equal(okFromBytes, true);
  assert.equal(okFromString, true);
});
