import { useCallback, useEffect, useState } from "react";
import { ApiError, AttemptSummary, Task, tokens, v1 } from "../api";

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
  const [toast, setToast] = useState<{ text: string; warn: boolean } | null>(
    null,
  );

  const refresh = useCallback(async () => {
    if (!tokens.cp) {
      setError("credencial bcp_ não configurada — vá em 4: config");
      return;
    }
    try {
      const data = await v1.listTasks();
      setItems(data.items);
      setError(null);
    } catch (e) {
      setError(
        e instanceof ApiError && e.status === 401
          ? "credencial recusada (401) — emita outra com /credential issue"
          : `tarefas indisponíveis: ${e instanceof Error ? e.message : e}`,
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
      flash(`detalhe indisponível: ${e instanceof Error ? e.message : e}`, true);
    }
  }

  function flash(text: string, warn = false) {
    setToast({ text, warn });
    setTimeout(() => setToast(null), 5000);
  }

  async function act(
    action: "pause" | "resume" | "cancel" | "steer",
    extra?: Record<string, unknown>,
  ) {
    if (!open) return;
    try {
      await v1.action(open.id, action, open.revision, extra);
      flash(`${action} aplicado`);
      await openDetail(open.id);
      refresh();
    } catch (e) {
      const code = e instanceof ApiError ? e.code : String(e);
      flash(
        code === "stale_revision"
          ? "revisão velha — detalhe recarregado, tente de novo"
          : `${action} falhou: ${code}`,
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
    <main className="view">
      <section className="pane">
        <div className="pane-head">
          tarefas duráveis (pursue)
          <span className="spacer" />
          <button onClick={refresh}>atualizar</button>
        </div>
        <div className="scroll">
          {error ? (
            <div className="empty">{error}</div>
          ) : items === null ? (
            <div className="empty">carregando…</div>
          ) : items.length === 0 ? (
            <div className="empty">
              <span className="start">▶ NENHUMA MISSÃO ATIVA</span>
              <br />
              objetivos duráveis viram tarefas aqui — dispare um pelo chat
            </div>
          ) : (
            <table aria-label="tarefas Pursue">
              <thead>
                <tr>
                  <th className="nowrap">id</th>
                  <th>status</th>
                  <th>objetivo</th>
                  <th className="nowrap">passos</th>
                  <th className="nowrap">custo</th>
                </tr>
              </thead>
              <tbody>
                {items.map((t) => (
                  <tr
                    key={t.id}
                    className={"task" + (open?.id === t.id ? " open" : "")}
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
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>

        {open && can && (
          <div className="detail">
            <h2>{open.objective}</h2>
            <div className="meta">
              {open.id} · {open.status} · revisão {open.revision}
              {open.stop_reason
                ? ` · parada: ${open.stop_reason.kind}${open.stop_reason.detail ? ` (${open.stop_reason.detail})` : ""}`
                : ""}
            </div>
            <div className="actions">
              {can.pause && (
                <button onClick={() => act("pause")}>pausar</button>
              )}
              {can.resume && (
                <button onClick={() => act("resume")}>retomar</button>
              )}
              {can.steer && (
                <button onClick={() => setSteering((s) => !s)}>
                  direcionar
                </button>
              )}
              {can.cancel && (
                <button className="danger" onClick={() => act("cancel")}>
                  cancelar
                </button>
              )}
              <button
                onClick={() => {
                  setOpen(null);
                  refresh();
                }}
              >
                fechar
              </button>
            </div>
            {steering && (
              <div className="steer-row">
                <input
                  value={guidance}
                  onChange={(e) => setGuidance(e.target.value)}
                  placeholder="nova orientação para a tarefa"
                  aria-label="orientação"
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
                  enviar
                </button>
              </div>
            )}
            {attempts.map((a) => (
              <div className="attempt" key={a.id}>
                {a.id} · início {ts(a.started_at)}
                {a.ended_at ? ` · fim ${ts(a.ended_at)}` : " · em andamento"}
                {a.verified ? (
                  <>
                    {" · veredicto: "}
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
      </section>
      {toast && (
        <div className={"toast" + (toast.warn ? " warn" : "")} role="status">
          {toast.text}
        </div>
      )}
    </main>
  );
}
