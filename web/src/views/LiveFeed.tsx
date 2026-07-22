import { BastionEvent } from "../api";
import type { LedgerEntry } from "../App";
import { LANTERN_GLOW_MS } from "../App";
import { Empty } from "../ui";

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
  if (ev.mode === "cabinet") parts.push("cabinet");
  if (Array.isArray(ev.personas) && ev.personas.length)
    parts.push(ev.personas.join(" + "));
  if (ev.task) parts.push(String(ev.task));
  if (ev.attempt) parts.push(`attempt ${ev.attempt}`);
  if (ev.status) parts.push(String(ev.status).toLowerCase());
  if (typeof ev.latency_ms === "number") parts.push(`${ev.latency_ms} ms`);
  if (ev.event === "mesh_sync") parts.push("mesh sync");
  return parts.join(" · ");
}

export default function LiveFeed({
  ledger,
  personas,
}: {
  ledger: LedgerEntry[];
  personas: Map<string, number>;
}) {
  const lanterns = [...personas.entries()].sort((a, b) => b[1] - a[1]);
  return (
    <>
      <div className="page-head">
        <h1>Live feed</h1>
        <span className="sub">
          every turn, persona and task event, as it happens
        </span>
      </div>
      <div className="lanterns">
        {lanterns.length === 0 ? (
          <span className="hint">lanterns light up when a persona speaks</span>
        ) : (
          lanterns.map(([name, seen]) => (
            <span
              key={name}
              className={
                "lantern" + (Date.now() - seen < LANTERN_GLOW_MS ? " lit" : "")
              }
            >
              <span className="px" />
              {name}
            </span>
          ))
        )}
      </div>
      <div className="page-body flush">
        {ledger.length === 0 ? (
          <Empty start="AWAITING SIGNAL">
            set the owner token in Connection — turns, personas and tasks
            stream here live
          </Empty>
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
    </>
  );
}
