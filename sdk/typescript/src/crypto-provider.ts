/**
 * Resolve a Web Crypto `Crypto` object, preferring the global (present in
 * every browser and in Node >= 20) and falling back to `node:crypto`'s
 * `webcrypto` export ONLY when the global is missing (Node 18 — this SDK's
 * documented minimum, `package.json`'s `engines.node`, exposes Web Crypto
 * behind a flag rather than as an unconditional global; see
 * https://nodejs.org/en/blog/announcements/nodejs-globalthis-webcrypto).
 *
 * The `node:crypto` import is dynamic and only ever evaluated when the
 * global is absent — in a real browser this branch is dead code, never
 * imported, so this file does not compromise `src/`'s "no Node-only APIs"
 * design goal (see `client.ts`'s module doc comment). It exists ONLY so
 * `idempotency.ts`/`webhook.ts` work out of the box on Node 18, not just 20+.
 */
export async function getCrypto(): Promise<Crypto> {
  const globalCrypto = (globalThis as { crypto?: Crypto }).crypto;
  if (globalCrypto) return globalCrypto;

  // Node < 20 (or a non-browser, non-Node environment with neither) — try
  // node:crypto's webcrypto export. If this import itself fails (a real
  // browser has no `node:crypto` module at all), that's fine: it means
  // `globalThis.crypto` should have been present and wasn't, which is a
  // genuine environment problem this function can't paper over further.
  const nodeCrypto = await import("node:crypto");
  return nodeCrypto.webcrypto as unknown as Crypto;
}
