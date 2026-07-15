import crypto from "crypto";

/** Format: BAST-XXXX-XXXX */
const TOKEN_PATTERN = /^BAST-[A-F0-9]{4}-[A-F0-9]{4}$/;

/** TTL for pending tokens: 5 minutes in milliseconds */
const TOKEN_TTL_MS = 5 * 60 * 1000;

/**
 * In-memory store of pending (unused) tokens.
 * Key: token string, Value: expiry Date.
 */
export const pendingTokens: Map<string, Date> = new Map();

/**
 * Generates a one-time connect token in the format BAST-XXXX-XXXX.
 * Uses crypto.randomBytes for cryptographic randomness.
 */
export function generateConnectToken(): string {
  const bytes = crypto.randomBytes(4);
  const hex = bytes.toString("hex").toUpperCase();
  return `BAST-${hex.slice(0, 4)}-${hex.slice(4, 8)}`;
}

/**
 * Adds a newly generated token to the pending store with a 5-minute TTL.
 * Returns the token string.
 */
export function addPendingToken(token: string): void {
  const expiry = new Date(Date.now() + TOKEN_TTL_MS);
  pendingTokens.set(token, expiry);
}

/**
 * Checks whether a token is still valid (exists and not expired).
 * Does NOT consume the token.
 */
export function isTokenValid(token: string): boolean {
  const expiry = pendingTokens.get(token);
  if (!expiry) return false;
  return Date.now() < expiry.getTime();
}

/**
 * Validates the token format matches BAST-XXXX-XXXX.
 */
export function isValidTokenFormat(token: string): boolean {
  return TOKEN_PATTERN.test(token);
}

/**
 * Removes expired tokens from the pending store (housekeeping).
 */
export function purgeExpiredTokens(): void {
  const now = Date.now();
  for (const [token, expiry] of pendingTokens.entries()) {
    if (now >= expiry.getTime()) {
      pendingTokens.delete(token);
    }
  }
}
