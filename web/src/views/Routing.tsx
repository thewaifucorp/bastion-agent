import { useCallback, useEffect, useState } from "react";
import {
  ModelsResponse,
  Proposal,
  RoutingItem,
  proposalsApi,
  routingApi,
  tokens,
} from "../api";
import { Row, Section, useToast } from "../ui";

// Routing (A4.5): one model rule per deterministic call-site class. The
// matrix below stages a routing_config proposal — the web never applies;
// the operator approves on the daemon console and GET /routing reflects
// the audited override. Classes without an agent-reachable knob on the
// pinned core rev are shown disabled and honestly labeled.

const UNSUPPORTED_TOOLTIP = "requires core support — persisted for future";

const CLASS_LABEL: Record<string, string> = {
  chat_turn: "Chat turns",
  pursue_task: "Pursue tasks",
  cabinet: "Cabinet",
  reflection: "Reflection",
  compaction: "Compaction",
};

const CLASS_DESC: Record<string, string> = {
  chat_turn: "interactive conversation turns — applies live, like /model",
  pursue_task: "delegated coding tasks run inside an external runtime",
  cabinet: "multi-persona deliberation legs",
  reflection: "the offline Reflector — applies on the next restart",
  compaction: "history summarization before long turns",
};

function sameRules(a: Record<string, string>, b: Record<string, string>): boolean {
  const ka = Object.keys(a);
  const kb = Object.keys(b);
  return ka.length === kb.length && ka.every((k) => a[k] === b[k]);
}

export default function RoutingSection({
  catalog,
  tick,
}: {
  catalog: ModelsResponse | null;
  tick: number;
}) {
  const [items, setItems] = useState<RoutingItem[] | null>(null);
  const [proposals, setProposals] = useState<Proposal[]>([]);
  const [error, setError] = useState<string | null>(null);
  // null = untouched (mirror the current override rules)
  const [draft, setDraft] = useState<Record<string, string> | null>(null);
  const [busy, setBusy] = useState(false);
  const toast = useToast();

  const refresh = useCallback(async () => {
    if (!tokens.owner) return;
    try {
      const [r, props] = await Promise.all([
        routingApi.get(),
        proposalsApi.list(),
      ]);
      setItems(r.items);
      setProposals(
        props.items.filter((x) => x.payload.kind === "routing_config"),
      );
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh, tick]);

  // The staged map replaces the WHOLE override, so the draft starts from
  // the override-sourced rules only — toml rules stay declarative unless
  // the operator explicitly picks a model for that class.
  const baseRules: Record<string, string> = Object.fromEntries(
    (items ?? [])
      .filter((i) => i.source === "override" && i.model)
      .map((i) => [i.class, i.model as string]),
  );
  const curRules = draft ?? baseRules;
  const dirty = !sameRules(curRules, baseRules);

  function setRule(cls: string, model: string) {
    const next = { ...curRules };
    if (model) next[cls] = model;
    else delete next[cls];
    setDraft(next);
  }

  async function stage() {
    if (!dirty || busy) return;
    setBusy(true);
    try {
      const p = await proposalsApi.createRoutingConfig(curRules);
      toast(
        `staged as ${p.id} — approve on the daemon console: /proposal approve ${p.id}`,
      );
      setDraft(null);
      refresh();
    } catch (e) {
      toast(`staging failed: ${e instanceof Error ? e.message : e}`, true);
    } finally {
      setBusy(false);
    }
  }

  const catalogModels = (catalog?.providers ?? []).flatMap((g) => g.models);

  if (error) {
    return (
      <Section title="Routing — model per call-site class">
        <Row title="routing unavailable" desc={error}>
          <button onClick={refresh}>retry</button>
        </Row>
      </Section>
    );
  }

  return (
    <Section title="Routing — model per call-site class">
      {items === null ? (
        <Row title="loading routing table…" />
      ) : (
        <>
          {items.map((item) => {
            const chosen = curRules[item.class] ?? "";
            const drafted = chosen !== (baseRules[item.class] ?? "");
            // A custom id (from toml or a console write) must stay
            // selectable even when the catalog doesn't list it.
            const extra =
              chosen && !catalogModels.some((m) => m.id === chosen)
                ? [chosen]
                : [];
            const emptyLabel =
              !chosen && item.source === "toml" && item.model
                ? `no override — toml: ${item.model}`
                : "no rule — follows the default model";
            return (
              <Row
                key={item.class}
                title={CLASS_LABEL[item.class] ?? item.class}
                desc={
                  item.supported
                    ? CLASS_DESC[item.class] ?? item.class
                    : `${CLASS_DESC[item.class] ?? item.class} — ${UNSUPPORTED_TOOLTIP}`
                }
              >
                {drafted && <span className="chip escalated">draft</span>}
                {!drafted && item.source === "override" && item.model && (
                  <span className="chip succeeded">override</span>
                )}
                {!drafted && item.source === "toml" && item.model && (
                  <span className="chip paused">toml</span>
                )}
                {!item.supported && (
                  <span className="chip" title={UNSUPPORTED_TOOLTIP}>
                    unsupported
                  </span>
                )}
                <select
                  value={chosen}
                  onChange={(e) => setRule(item.class, e.target.value)}
                  disabled={!item.supported || busy}
                  title={item.supported ? undefined : UNSUPPORTED_TOOLTIP}
                  aria-label={`${CLASS_LABEL[item.class] ?? item.class} model`}
                >
                  <option value="">{emptyLabel}</option>
                  {extra.map((id) => (
                    <option key={id} value={id}>
                      {id}
                    </option>
                  ))}
                  {catalogModels.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.display_name} ({m.id})
                    </option>
                  ))}
                </select>
                <button
                  onClick={() => setRule(item.class, "")}
                  disabled={!item.supported || busy || !chosen}
                >
                  clear
                </button>
              </Row>
            );
          })}

          {dirty && (
            <Row
              title="Stage this routing change"
              desc="creates a pending routing_config proposal; apply it on the daemon console with /proposal approve <id>"
            >
              <button onClick={() => setDraft(null)} disabled={busy}>
                discard
              </button>
              <button className="primary" onClick={stage} disabled={busy}>
                {busy ? "staging…" : "stage proposal"}
              </button>
            </Row>
          )}

          {proposals.length > 0 &&
            proposals.map((p) => (
              <Row
                key={p.id}
                title={`${
                  p.payload.kind === "routing_config"
                    ? Object.entries(p.payload.rules)
                        .map(([c, m]) => `${c} → ${m}`)
                        .join(" · ") || "clear the override"
                    : p.payload.kind
                } — ${p.id}`}
                desc={`${p.origin} · ${new Date(p.created_at / 1e6).toLocaleString()}${
                  p.status === "pending"
                    ? ` · approve on console: /proposal approve ${p.id}`
                    : ""
                }`}
              >
                <span
                  className={`chip ${
                    p.status === "pending"
                      ? "running"
                      : p.status === "approved"
                        ? "succeeded"
                        : "failed"
                  }`}
                >
                  {p.status}
                </span>
              </Row>
            ))}
        </>
      )}
    </Section>
  );
}
