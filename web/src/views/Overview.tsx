import { useEffect, useState } from "react";
import {
  health,
  status,
  StatusSnapshot,
  tokens,
} from "../api";
import { Dot, Row, Section } from "../ui";

export default function Overview({ go }: { go: (key: string) => void }) {
  const [snap, setSnap] = useState<StatusSnapshot | null>(null);
  const [hz, setHz] = useState<{ healthz: boolean; readyz: boolean } | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const load = async () => {
      try {
        setSnap(await status());
        setHz(await health());
        setError(null);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
    };
    load();
    const t = setInterval(load, 20000);
    return () => clearInterval(t);
  }, []);

  const update = snap?.update ?? {};
  const updateLabel =
    typeof update === "object" && update !== null
      ? String(
          (update as Record<string, unknown>).state ??
            (update as Record<string, unknown>).status ??
            "unknown",
        )
      : "unknown";

  return (
    <>
      <div className="page-head">
        <h1>Overview</h1>
        <span className="sub">daemon health, runtimes, release state</span>
      </div>
      <div className="page-body">
        {error && (
          <div className="error-line">status unavailable: {error}</div>
        )}

        <div className="stats">
          <div className="stat">
            <div className="k">daemon</div>
            <div className="v">
              <Dot
                state={hz?.healthz ? "ok" : "bad"}
                label={hz?.healthz ? "alive" : "unreachable"}
              />
            </div>
          </div>
          <div className="stat">
            <div className="k">readiness</div>
            <div className="v">
              <Dot
                state={snap?.ready ? "ok" : "warn"}
                label={snap?.ready ? "ready" : "starting"}
              />
            </div>
          </div>
          <div className="stat">
            <div className="k">runtimes</div>
            <div className="v">
              {snap
                ? `${snap.runtimes.filter((r) => r.cli_present).length}/${snap.runtimes.length} present`
                : "—"}
            </div>
          </div>
          <div className="stat">
            <div className="k">update</div>
            <div className="v">{updateLabel}</div>
          </div>
        </div>

        <Section title="Coding runtimes">
          {snap === null ? (
            <Row title="loading…" />
          ) : snap.runtimes.length === 0 ? (
            <Row
              title="no runtimes registered"
              desc="Pursue coding tasks delegate to external runtimes (codex, claude, opencode) — none is wired in this build"
            />
          ) : (
            snap.runtimes.map((r) => (
              <Row
                key={r.id}
                title={r.id}
                desc={
                  r.cli_present
                    ? "CLI installed and answering"
                    : "CLI not found on the host"
                }
              >
                <Dot
                  state={
                    r.logged_in ? "ok" : r.cli_present ? "warn" : "bad"
                  }
                  label={
                    r.logged_in
                      ? "logged in"
                      : r.cli_present
                        ? "not logged in"
                        : "missing"
                  }
                />
              </Row>
            ))
          )}
        </Section>

        <Section title="Access">
          <Row
            title="Live feed & chat"
            desc="owner token — /events and /webhook"
          >
            <Dot
              state={tokens.owner ? "ok" : "off"}
              label={tokens.owner ? "configured" : "not set"}
            />
          </Row>
          <Row
            title="Tasks API"
            desc="Control Plane credential — /v1/*"
          >
            <Dot
              state={tokens.cp ? "ok" : "off"}
              label={tokens.cp ? "configured" : "not set"}
            />
          </Row>
          <Row title="Set up tokens" desc="both live only in this browser">
            <button onClick={() => go("connection")}>open connection</button>
          </Row>
        </Section>
      </div>
    </>
  );
}
