import { connectedDevices } from "./exchange";

/**
 * In-memory blocklist of revoked device names.
 * Once a device is revoked, its JWT is rejected on every subsequent request.
 */
export const revokedDevices: Set<string> = new Set();

/**
 * Revokes a device immediately by adding it to the in-memory blocklist
 * and removing it from the connected devices registry.
 */
export function revokeDevice(deviceName: string): void {
  revokedDevices.add(deviceName);
  connectedDevices.delete(deviceName);
}

/**
 * Returns true if the device has been revoked.
 */
export function isRevoked(deviceName: string): boolean {
  return revokedDevices.has(deviceName);
}
