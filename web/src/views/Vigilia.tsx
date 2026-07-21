import { BastionEvent } from "../api";
import type { LedgerEntry } from "../App";

function kindClass(event: string): string {
  if (event === "turn.failed") return "k-fail";
  if (event === "turn.completed" || event === "task.verified") return "k-ok";
  if (event === "cabinet.started") return "k-cabinet";
  if (event.startsWith("turn.")) return "k-turn";
  if (event.startsWith("task.")) return "k-task";
  return "";
}

function describe(ev: BastionEvent): string {
  const parts: string[] = [];
  if (ev.owner) parts.push(String(ev.owner));
  if (ev.mode === "cabinet") parts.push("gabinete");
  if (Array.isArray(ev.personas) && ev.personas.length)
    parts.push(ev.personas.join(" + "));
  if (ev.task) parts.push(String(ev.task));
  if (ev.attempt) parts.push(`tentativa ${ev.attempt}`);
  if (ev.status) parts.push(String(ev.status).toLowerCase());
  if (typeof ev.latency_ms === "number") parts.push(`${ev.latency_ms} ms`);
  if (ev.event === "mesh_sync") parts.push("sincronia de malha");
  return parts.join(" · ");
}

export default function Vigilia({ ledger }: { ledger: LedgerEntry[] }) {
  return (
    <main className="view">
      <section className="pane">
        <div className="pane-head">livro de vigília</div>
        <div className="scroll">
          {ledger.length === 0 ? (
            <div className="empty">
              <span className="start">▶ AGUARDANDO SINAL</span>
              <br />
              conecte o token de owner em 4: config — cada turno, persona e
              tarefa aparece aqui ao vivo
            </div>
          ) : (
            <div className="ledger">
              {ledger.map((e, i) => (
                <div className="entry" key={ledger.length - i}>
                  <time>{new Date(e.at).toLocaleTimeString()}</time>
                  <span className={`kind ${kindClass(e.ev.event)}`}>
                    {e.ev.event}
                  </span>
                  <span className="body">{describe(e.ev)}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      </section>
    </main>
  );
}
