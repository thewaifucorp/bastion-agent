import { FormEvent, useState } from "react";
import { tokens } from "../api";
import { Empty, Term, useCommand, useToast } from "../ui";

/** Schedules cockpit over the daemon's /schedule command: list output as a
 * terminal block, plus structured forms that build the exact command the
 * console would take (`/schedule add every <secs> <intent>` etc.). */
export default function Schedules() {
  const { out, cmd, busy, error, run } = useCommand(
    tokens.owner ? "/schedule list" : undefined,
  );
  const [kind, setKind] = useState<"every" | "once">("every");
  const [secs, setSecs] = useState("3600");
  const [intent, setIntent] = useState("");
  const [cancelId, setCancelId] = useState("");
  const toast = useToast();

  async function add(e: FormEvent) {
    e.preventDefault();
    const s = Number(secs);
    const i = intent.trim();
    if (!i || !Number.isFinite(s) || s <= 0) {
      toast("schedule needs a positive interval and an intent", true);
      return;
    }
    setIntent("");
    await run(`/schedule add ${kind} ${s} ${i}`);
    await run("/schedule list");
    toast("schedule added");
  }

  async function cancel(e: FormEvent) {
    e.preventDefault();
    const id = cancelId.trim();
    if (!id) return;
    setCancelId("");
    await run(`/schedule cancel ${id}`);
    await run("/schedule list");
    toast("cancel sent");
  }

  return (
    <>
      <div className="page-head">
        <h1>Schedules</h1>
        <span className="sub">
          authorized intents that fire once or on a recurrence (owner-scoped)
        </span>
        <span className="spacer" />
        <button onClick={() => run("/schedule list")} disabled={busy}>
          refresh
        </button>
      </div>
      <div className="page-body">
        {!tokens.owner ? (
          <Empty start="NO TOKEN">
            schedules ride the daemon's /schedule command — set the owner
            token under Connection
          </Empty>
        ) : (
          <>
            {error && <div className="error-line">{error}</div>}
            {out !== null && <Term cmd={cmd ?? "/schedule list"} out={out} />}

            <div className="section" style={{ marginTop: 18 }}>
              <h2>New schedule</h2>
              <div className="card">
                <form onSubmit={add}>
                  <div className="row">
                    <div className="txt">
                      <div className="t">Recurrence</div>
                      <div className="d">
                        "every" repeats; "once" fires a single time after the
                        interval
                      </div>
                    </div>
                    <div className="ctl">
                      <button
                        type="button"
                        className={kind === "every" ? "primary" : ""}
                        onClick={() => setKind("every")}
                      >
                        every
                      </button>
                      <button
                        type="button"
                        className={kind === "once" ? "primary" : ""}
                        onClick={() => setKind("once")}
                      >
                        once
                      </button>
                    </div>
                  </div>
                  <div className="row">
                    <div className="txt">
                      <div className="t">Interval (seconds)</div>
                      <div className="d">3600 = hourly, 86400 = daily</div>
                    </div>
                    <div className="ctl">
                      <input
                        style={{ width: "12ch" }}
                        value={secs}
                        onChange={(e) => setSecs(e.target.value)}
                        inputMode="numeric"
                        aria-label="interval in seconds"
                      />
                    </div>
                  </div>
                  <div className="row">
                    <div className="txt">
                      <div className="t">Intent</div>
                      <div className="d">
                        what the daemon should do when it fires
                      </div>
                    </div>
                    <div className="ctl" style={{ flex: 2 }}>
                      <input
                        value={intent}
                        onChange={(e) => setIntent(e.target.value)}
                        placeholder="e.g. check the repo for new issues and summarize"
                        aria-label="intent"
                      />
                      <button type="submit" className="primary" disabled={busy}>
                        add
                      </button>
                    </div>
                  </div>
                </form>
              </div>
            </div>

            <div className="section">
              <h2>Cancel</h2>
              <div className="card">
                <form onSubmit={cancel}>
                  <div className="row">
                    <div className="txt">
                      <div className="t">Cancel a schedule</div>
                      <div className="d">id from the list above</div>
                    </div>
                    <div className="ctl">
                      <input
                        style={{ width: "16ch" }}
                        value={cancelId}
                        onChange={(e) => setCancelId(e.target.value)}
                        placeholder="schedule id"
                        aria-label="schedule id"
                      />
                      <button type="submit" className="danger" disabled={busy}>
                        cancel
                      </button>
                    </div>
                  </div>
                </form>
              </div>
            </div>
          </>
        )}
      </div>
    </>
  );
}
