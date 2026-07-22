import { useCallback, useEffect, useState } from "react";
import { ApiError, AttemptSummary, Task, tokens, v1 } from "../api";
import { Empty, useToast } from "../ui";

function money(v: number | null | undefined): string {
  return v == null ? "—" : `$${v.toFixed(2)}`;
}
function ts(nanos: number): string {
  return new Date(nanos / 1e6).toLocaleString();
}
function shortId(id: string): string {
  return id.length > 14 ? id.slice(0, 14) + "…" : id;
}

export default function Tasks() {
  const [items, setItems] = useState<Task[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<Task | null>(null);
  const [attempts, setAttempts] = useState<AttemptSummary[]>([]);
  const [steering, setSteering] = useState(false);
  const [guidance, setGuidance] = useState("");
  const toast = useToast();

  const refresh = useCallback(async () => {
    if (!tokens.cp) {
      setError("no credential");
      return;
    }
    try {
      const data = await v1.listTasks();
      setItems(data.items);
      setError(null);
    } catch (e) {
      setError(
        e instanceof ApiError && e.status === 401
          ? "credential rejected (401) — issue a new one with /credential issue"
          : `tasks unavailable: ${e instanceof Error ? e.message : e}`,
      );
    }
  }, []);

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 15000);
    return () => clearInterval(t);
  }, [refresh]);

  async function openDetail(id: string) {
    try {
      const [task, att] = await Promise.all([v1.getTask(id), v1.attempts(id)]);
      setOpen(task);
      setAttempts(att.items ?? att.attempts ?? []);
      setSteering(false);
    } catch (e) {
      toast(`detail unavailable: ${e instanceof Error ? e.message : e}`, true);
    }
  }

  async function act(
    action: "pause" | "resume" | "cancel" | "steer",
    extra?: Record<string, unknown>,
  ) {
    if (!open) return;
    try {
      await v1.action(open.id, action, open.revision, extra);
      toast(`${action} applied`);
      await openDetail(open.id);
      refresh();
    } catch (e) {
      const code = e instanceof ApiError ? e.code : String(e);
      toast(
        code === "stale_revision"
          ? "stale revision — detail reloaded, try again"
          : `${action} failed: ${code}`,
        true,
      );
      await openDetail(open.id);
    }
  }

  const can = open
    ? {
        pause: open.status === "running",
        resume: open.status === "paused",
        steer: open.status === "running",
        cancel: ["pending", "running", "paused"].includes(open.status),
      }
    : null;

  return (
    <>
      <div className="page-head">
        <h1>Tasks</h1>
        <span className="sub">
          durable Pursue tasks — attempts, verdicts, budget, control
        </span>
        <span className="spacer" />
        <button onClick={refresh}>refresh</button>
      </div>
      <div className="page-body flush">
        <div style={{ flex: 1, overflowY: "auto", padding: "12px 24px" }}>
          {!tokens.cp ? (
            <Empty start="NO CREDENTIAL">
              the task API needs a Control Plane credential — issue one on the
              daemon console and set it under Connection
            </Empty>
          ) : error ? (
            <div className="error-line">{error}</div>
          ) : items === null ? (
            <div className="empty">loading…</div>
          ) : items.length === 0 ? (
            <Empty start="NO ACTIVE MISSIONS">
              durable objectives become tasks here — start one from Chat
            </Empty>
          ) : (
            <table aria-label="Pursue tasks">
              <thead>
                <tr>
                  <th className="nowrap">id</th>
                  <th>status</th>
                  <th>objective</th>
                  <th className="nowrap">steps</th>
                  <th className="nowrap">cost</th>
                  <th className="nowrap">updated</th>
                </tr>
              </thead>
              <tbody>
                {items.map((t) => (
                  <tr
                    key={t.id}
                    className={"rowlink" + (open?.id === t.id ? " open" : "")}
                    onClick={() => openDetail(t.id)}
                  >
                    <td className="nowrap">{shortId(t.id)}</td>
                    <td>
                      <span className={`chip ${t.status}`}>{t.status}</span>
                    </td>
                    <td>{t.objective}</td>
                    <td className="nowrap">{t.budget_summary?.steps ?? "—"}</td>
                    <td className="nowrap">
                      {money(t.budget_summary?.cost_usd)}
                    </td>
                    <td className="nowrap">{ts(t.updated_at)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>

        {open && can && (
          <div className="drawer">
            <h2>{open.objective}</h2>
            <div className="meta">
              {open.id} · {open.status} · revision {open.revision}
              {open.stop_reason
                ? ` · stopped: ${open.stop_reason.kind}${open.stop_reason.detail ? ` (${open.stop_reason.detail})` : ""}`
                : ""}
              {open.budget_summary
                ? ` · ${open.budget_summary.llm_calls} llm calls · ${open.budget_summary.total_tokens} tokens`
                : ""}
            </div>
            <div className="actions">
              {can.pause && <button onClick={() => act("pause")}>pause</button>}
              {can.resume && (
                <button onClick={() => act("resume")}>resume</button>
              )}
              {can.steer && (
                <button onClick={() => setSteering((s) => !s)}>steer</button>
              )}
              {can.cancel && (
                <button className="danger" onClick={() => act("cancel")}>
                  cancel
                </button>
              )}
              <button
                onClick={() => {
                  setOpen(null);
                  refresh();
                }}
              >
                close
              </button>
            </div>
            {steering && (
              <div className="steer-row">
                <input
                  value={guidance}
                  onChange={(e) => setGuidance(e.target.value)}
                  placeholder="fresh guidance for the running task"
                  aria-label="guidance"
                />
                <button
                  className="primary"
                  onClick={() => {
                    const g = guidance.trim();
                    if (!g) return;
                    setGuidance("");
                    act("steer", { guidance: g });
                  }}
                >
                  send
                </button>
              </div>
            )}
            {attempts.map((a) => (
              <div className="attempt" key={a.id}>
                {a.id} · started {ts(a.started_at)}
                {a.ended_at ? ` · ended ${ts(a.ended_at)}` : " · in flight"}
                {a.verified ? (
                  <>
                    {" · verdict: "}
                    <span className={`chip ${a.verified.kind}`}>
                      {a.verified.kind}
                    </span>
                    {a.verified.detail ? ` ${a.verified.detail}` : ""}
                  </>
                ) : null}
              </div>
            ))}
          </div>
        )}
      </div>
    </>
  );
}
