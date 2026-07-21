import { getCrypto } from "./crypto-provider.js";

/**
 * Generate an opaque idempotency key for `BastionClient.createTask`, via
 * `crypto.randomUUID()` (see `crypto-provider.ts` for why this goes through
 * a resolver instead of the bare global — Node 18 compatibility).
 *
 * This is a CONVENIENCE, not a requirement — any caller-chosen unique string
 * works (the server derives its own storage key from
 * `sha256(owner || idempotency_key)`; see
 * `src/control_plane/routes.rs`'s `deterministic_task_id`). Prefer your own
 * stable id (e.g. an upstream issue id) over this generator whenever one
 * already exists — a random key defeats the point of idempotent retry across
 * process restarts, since a NEW key on retry after a crash creates a second
 * task instead of returning the first.
 */
export async function generateIdempotencyKey(): Promise<string> {
  const crypto = await getCrypto();
  return crypto.randomUUID();
}
