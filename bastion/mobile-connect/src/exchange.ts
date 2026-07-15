import jwt from "jsonwebtoken";
import { pendingTokens, isTokenValid } from "./token";

/** JWT expiry: 90 days in seconds */
const JWT_EXP_SECONDS = 90 * 24 * 60 * 60;

/** In-memory registry of connected devices: deviceName → JWT issued-at timestamp */
export const connectedDevices: Map<string, number> = new Map();

/**
 * Reads JWT_SECRET from environment. Throws if not set.
 */
function getJwtSecret(): string {
  const secret = process.env["JWT_SECRET"];
  if (!secret) {
    throw new Error("JWT_SECRET environment variable is not set");
  }
  return secret;
}

/**
 * Exchanges a one-time token for a long-lived JWT.
 *
 * - Validates the token TTL (5 min)
 * - Deletes the token immediately (one-time use)
 * - Generates a JWT with exp: 90 days
 * - Registers the device in connectedDevices
 *
 * Returns the signed JWT string, or null if the token is invalid/expired.
 */
export function exchangeToken(token: string, deviceName: string): string | null {
  if (!isTokenValid(token)) {
    return null;
  }

  // Invalidate immediately — one-time use
  pendingTokens.delete(token);

  const secret = getJwtSecret();
  const issuedAt = Math.floor(Date.now() / 1000);

  const payload = {
    device: deviceName,
    iat: issuedAt,
  };

  const signed = jwt.sign(payload, secret, {
    expiresIn: JWT_EXP_SECONDS,
    algorithm: "HS256",
  });

  connectedDevices.set(deviceName, issuedAt);

  return signed;
}

/**
 * Verifies a JWT and returns the decoded payload, or null if invalid/revoked.
 * Revocation check is delegated to the caller via the isRevoked callback.
 */
export function verifyDeviceJwt(
  token: string,
  isRevoked: (deviceName: string) => boolean
): { device: string; iat: number; exp: number } | null {
  const secret = getJwtSecret();
  try {
    const decoded = jwt.verify(token, secret, { algorithms: ["HS256"] }) as {
      device: string;
      iat: number;
      exp: number;
    };
    if (isRevoked(decoded.device)) {
      return null;
    }
    return decoded;
  } catch {
    return null;
  }
}
