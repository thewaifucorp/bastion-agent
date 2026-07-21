import { getCrypto } from "./crypto-provider.js";

/**
 * Verify an inbound webhook delivery's `X-Bastion-Signature` header,
 * matching `src/control_plane/webhook_delivery.rs`'s `sign_payload` exactly
 * (HMAC-SHA256 over the raw body bytes, `sha256=<hex>`).
 *
 * Uses the Web Crypto API (`crypto.subtle`, via `crypto-provider.ts`) — in a
 * real browser this never touches Node at all; see that file's doc comment
 * for why Node 18 specifically needs a resolver rather than the bare global.
 */

/**
 * @param secret The signing secret returned ONCE at subscription creation
 *   (`BastionClient.createWebhookSubscription`'s response) — store it
 *   yourself; the server never returns it again.
 * @param rawBody The EXACT bytes received on the wire. Passing a
 *   re-serialized/re-parsed copy of the JSON will fail verification even for
 *   a genuine delivery — whitespace/key-order can differ from what was
 *   signed. Framework body parsers often expose the raw bytes separately
 *   from the parsed object for exactly this reason (e.g. Express's
 *   `express.raw()`); use that, not `JSON.stringify(req.body)`.
 * @param signatureHeader The full `X-Bastion-Signature` header value,
 *   e.g. `"sha256=3f2a...".`
 */
export async function verifyWebhookSignature(
  secret: string,
  rawBody: Uint8Array | string,
  signatureHeader: string | null | undefined,
): Promise<boolean> {
  if (!signatureHeader) return false;
  const prefix = "sha256=";
  if (!signatureHeader.startsWith(prefix)) return false;
  const expectedHex = signatureHeader.slice(prefix.length);
  if (!/^[0-9a-f]+$/i.test(expectedHex) || expectedHex.length !== 64) return false;

  const bodyBytes =
    typeof rawBody === "string" ? new TextEncoder().encode(rawBody) : rawBody;

  const crypto = await getCrypto();
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  // Cast: this lib's `Uint8Array<ArrayBufferLike>` (from `TextEncoder.encode`/
  // a caller-supplied `Uint8Array`) is structurally a valid `BufferSource` at
  // runtime; the mismatch is a TS lib-generic quirk (SharedArrayBuffer-typed
  // backing array signature), not a real type hazard here.
  const signature = await crypto.subtle.sign("HMAC", key, bodyBytes as BufferSource);
  const actualHex = bytesToHex(new Uint8Array(signature));

  return constantTimeEqual(actualHex.toLowerCase(), expectedHex.toLowerCase());
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/** Constant-time string comparison — never a plain `===` on a signature. */
function constantTimeEqual(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) {
    diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  }
  return diff === 0;
}
