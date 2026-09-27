import { useCallback, useEffect, useState } from "react";
import {
  ApiError,
  devicesApi,
  primaryInfo,
  tokens,
  type DevicesSnapshot,
  type PrimaryInfo,
} from "../api";
import { Dot, Empty, Row, Section, useToast } from "../ui";

/** The owner's devices (spec multi-device-brain-and-nodes): who the primary
 * is (public discovery, BMD-26), and — with the daemon operator token —
 * pending enrollment requests to approve (BMD-08), each device's grants and
 * revocation (BMD-33), and the reconciliation conflict queue (BMD-23). */
export default function Devices() {
  const [primary, setPrimary] = useState<PrimaryInfo | null | undefined>(undefined);
  const [snapshot, setSnapshot] = useState<DevicesSnapshot | null>(null);
  const [conflicts, setConflicts] = useState<
    { id: number; status: string }[]
  >([]);
  const [code, setCode] = useState<string | null>(null);
  const [daemonToken, setDaemonToken] = useState(tokens.daemon);
  const [adminError, setAdminError] = useState<string | null>(null);
  const toast = useToast();

  const loadPrimary = useCallback(() => {
    primaryInfo().then(setPrimary);
  }, []);

  const loadAdmin = useCallback(async () => {
    if (!tokens.daemon) {
      setSnapshot(null);
      return;
    }
    try {
      const [snap, conf] = await Promise.all([
        devicesApi.list(),
        devicesApi.conflicts(),
      ]);
      setSnapshot(snap);
      setConflicts(conf.conflicts);
      setAdminError(null);
    } catch (e) {
      setSnapshot(null);
      setAdminError(e instanceof ApiError ? e.code : String(e));
    }
  }, []);

  useEffect(() => {
    loadPrimary();
    loadAdmin();
    const t = setInterval(loadPrimary, 15_000);
    return () => clearInterval(t);
  }, [loadPrimary, loadAdmin]);

  function saveToken() {
    tokens.daemon = daemonToken.trim();
    loadAdmin();
  }

  async function guard(action: () => Promise<void>, ok: string) {
    try {
      await action();
      toast(ok);
      await loadAdmin();
    } catch (e) {
      toast(e instanceof ApiError ? e.code : String(e), true);
    }
  }

  const pending = snapshot?.requests.filter((r) => r.status === "pending") ?? [];

  return (
    <>
      <Section title="Primary">
        {primary === undefined ? (
          <Empty start="…">checking</Empty>
        ) : primary === null ? (
          <Row title="Primary offline" desc="No primary answered on this address.">
            <Dot state="bad" label="offline" />
          </Row>
        ) : (
          <>
            <Row
              title={primary.device ?? "no primary"}
              desc={primary.address ?? "address not published"}
            >
              <Dot
                state={primary.this_device_is_primary ? "ok" : "info"}
                label={
                  primary.this_device_is_primary
                    ? `this device · epoch ${primary.epoch}`
                    : `epoch ${primary.epoch}`
                }
              />
            </Row>
            <Row title="This device" desc={primary.this_device} />
          </>
        )}
      </Section>

      <Section title="Administration">
        <Row
          title="Daemon operator token"
          desc="Device administration is gated by BASTION_DAEMON_TOKEN, not the owner token."
        >
          <input
            type="password"
            value={daemonToken}
            placeholder="Bearer token"
            onChange={(e) => setDaemonToken(e.target.value)}
          />
          <button onClick={saveToken}>Save</button>
        </Row>
        {adminError && (
          <Row title="Cannot administer" desc={adminError}>
            <Dot state="bad" label="denied" />
          </Row>
        )}
        {snapshot && (
          <Row
            title="Pair a new device"
            desc="Generate a one-time code, then run `bastion node pair` on the other device."
          >
            <button
              onClick={() =>
                devicesApi
                  .newCode()
                  .then((r) => setCode(r.code))
                  .catch((e) =>
                    toast(e instanceof ApiError ? e.code : String(e), true),
                  )
              }
            >
              New code
            </button>
            {code && <code>{code}</code>}
          </Row>
        )}
      </Section>

      {pending.length > 0 && (
        <Section title="Waiting for your approval">
          {pending.map((r) => (
            <Row
              key={r.id}
              title={r.device}
              desc={`${r.platform}${r.holds_replica ? " · keeps a replica" : ""}`}
            >
              <button
                onClick={() =>
                  guard(() => devicesApi.approve(r.id), "device approved")
                }
              >
                Approve
              </button>
              <button
                onClick={() =>
                  guard(() => devicesApi.refuse(r.id), "device refused")
                }
              >
                Refuse
              </button>
            </Row>
          ))}
        </Section>
      )}

      {snapshot && (
        <Section title={`Devices · epoch ${snapshot.current_epoch}`}>
          {snapshot.devices.length === 0 ? (
            <Empty start="node pair">no devices yet</Empty>
          ) : (
            snapshot.devices.map((d) => (
              <Row
                key={d.device}
                title={d.device}
                desc={`${d.platform} · ${d.granted.length} grant(s)${
                  d.holds_replica ? " · replica" : ""
                }`}
              >
                <Dot
                  state={d.revoked ? "bad" : d.connected ? "ok" : "off"}
                  label={d.revoked ? "revoked" : d.connected ? "online" : "offline"}
                />
                {!d.revoked && (
                  <button
                    onClick={() => {
                      if (
                        confirm(
                          `Revoke ${d.device}? It can no longer connect; rotate any secrets it kept.`,
                        )
                      ) {
                        guard(async () => {
                          const r = await devicesApi.revoke(d.device);
                          if (r.rotate_secrets.length > 0) {
                            toast(
                              `revoked; rotate ${r.rotate_secrets.length} secret(s)`,
                            );
                          }
                        }, "device revoked");
                      }
                    }}
                  >
                    Revoke
                  </button>
                )}
              </Row>
            ))
          )}
        </Section>
      )}

      {conflicts.length > 0 && (
        <Section title="Reconciliation conflicts">
          {conflicts.map((c) => (
            <Row
              key={c.id}
              title={`Conflict #${c.id}`}
              desc="A belief was changed on both sides during a partition. Choose which change stands; the other is kept in the log."
            >
              <button
                onClick={() =>
                  guard(
                    () => devicesApi.resolveConflict(c.id, "keep_ours"),
                    "kept this side",
                  )
                }
              >
                Keep ours
              </button>
              <button
                onClick={() =>
                  guard(
                    () => devicesApi.resolveConflict(c.id, "take_theirs"),
                    "took theirs",
                  )
                }
              >
                Take theirs
              </button>
            </Row>
          ))}
        </Section>
      )}
    </>
  );
}
