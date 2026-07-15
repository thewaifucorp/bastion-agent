import express, { Request, Response, Router } from "express";
import { generateConnectToken, addPendingToken } from "./token";
import { exchangeToken, connectedDevices, verifyDeviceJwt } from "./exchange";
import { revokeDevice, isRevoked } from "./revoke";

export const router: Router = express.Router();

/**
 * POST /auth/exchange
 *
 * Body: { token: string, device_name: string }
 * Response: { jwt: string } on success, or 400/401 on failure.
 */
router.post("/auth/exchange", (req: Request, res: Response): void => {
  const { token, device_name } = req.body as {
    token?: string;
    device_name?: string;
  };

  if (!token || !device_name) {
    res.status(400).json({ error: "token and device_name are required" });
    return;
  }

  const signed = exchangeToken(token, device_name);
  if (!signed) {
    res.status(401).json({ error: "Token is invalid, expired, or already used" });
    return;
  }

  res.status(200).json({ jwt: signed });
});

/**
 * GET /devices
 *
 * Lists all currently connected (non-revoked) devices.
 * Response: { devices: Array<{ name: string, connected_at: number }> }
 */
router.get("/devices", (_req: Request, res: Response): void => {
  const devices = Array.from(connectedDevices.entries())
    .filter(([name]) => !isRevoked(name))
    .map(([name, iat]) => ({ name, connected_at: iat }));

  res.status(200).json({ devices });
});

/**
 * DELETE /devices/:device
 *
 * Revokes the JWT for the specified device immediately.
 * Response: 204 on success, 404 if device not found.
 */
router.delete("/devices/:device", (req: Request, res: Response): void => {
  const { device } = req.params as { device: string };

  if (!connectedDevices.has(device)) {
    res.status(404).json({ error: `Device '${device}' not found` });
    return;
  }

  revokeDevice(device);
  res.status(204).send();
});

/**
 * Creates and returns the Express app with all mobile-connect routes mounted.
 */
export function createApp(): express.Application {
  const app = express();
  app.use(express.json());
  app.use(router);
  return app;
}

/**
 * Generates a new connect token, stores it as pending, and returns it.
 * This is called when the user types /connect-app.
 */
export function handleConnectApp(): string {
  const token = generateConnectToken();
  addPendingToken(token);
  return token;
}

/**
 * Middleware to authenticate requests using the device JWT.
 * Attaches decoded payload to req.body._device on success.
 */
export function jwtAuthMiddleware(
  req: Request,
  res: Response,
  next: express.NextFunction
): void {
  const authHeader = req.headers["authorization"];
  if (!authHeader || !authHeader.startsWith("Bearer ")) {
    res.status(401).json({ error: "Missing or invalid Authorization header" });
    return;
  }

  const token = authHeader.slice(7);
  const decoded = verifyDeviceJwt(token, isRevoked);
  if (!decoded) {
    res.status(401).json({ error: "JWT is invalid, expired, or device is revoked" });
    return;
  }

  // Attach device info for downstream handlers
  (req as Request & { devicePayload?: typeof decoded }).devicePayload = decoded;
  next();
}
