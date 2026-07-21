import { useCallback, useEffect, useState } from "react";
import {
  ModelsResponse,
  Proposal,
  modelsApi,
  proposalsApi,
  tokens,
} from "../api";
import { Empty, Row, Section, useToast } from "../ui";
import AuditStrip from "./AuditStrip";
import RoutingSection from "./Routing";

// Models: the merged catalog grouped by provider, plus the EFFECTIVE
// default and fallback ladder. Selecting a different default or editing
// the ladder STAGES a model_config proposal — the web never applies it;
// the operator approves on the daemon console and the change lands in the
// audited config store (see the strip below).

const MAX_FALLBACKS = 16;

const KIND_NAME: Record<string, string> = {
  anthropic: "Anthropic",
  openai: "OpenAI",
  gemini: "Google Gemini",
  groq: "Groq",
  openrouter: "OpenRouter",
  ollama: "Ollama",
};

function sameList(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((x, i) => x === b[i]);
}

export default function Models({ configTick }: { configTick: number }) {
  const [data, setData] = useState<ModelsResponse | null>(null);
  const [proposals, setProposals] = useState<Proposal[]>([]);
  const [error, setError] = useState<string | null>(null);
  // drafts: null = untouched (mirror the effective value)
  const [draftDefault, setDraftDefault] = useState<string | null>(null);
  const [draftFallbacks, setDraftFallbacks] = useState<string[] | null>(null);
  const [busy, setBusy] = useState(false);
  const toast = useToast();

  const refresh = useCallback(async () => {
    if (!tokens.owner) return;
    try {
      const [m, props] = await Promise.all([
        modelsApi.get(),
        proposalsApi.list(),
      ]);
      setData(m);
      setProposals(props.items.filter((x) => x.payload.kind === "model_config"));
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh, configTick]);

  const effDefault = data?.default_model ?? "";
  const effFallbacks = data?.fallback_models ?? [];
  const curDefault = draftDefault ?? effDefault;
  const curFallbacks = draftFallbacks ?? effFallbacks;
  const defaultChanged = curDefault !== effDefault;
  const fallbacksChanged = !sameList(curFallbacks, effFallbacks);
  const dirty = defaultChanged || fallbacksChanged;

  function setFallbacks(next: string[]) {
    setDraftFallbacks(next);
  }

  function addFallback(id: string) {
    if (curFallbacks.includes(id)) return;
    if (curFallbacks.length >= MAX_FALLBACKS) {
      toast(`the fallback ladder caps at ${MAX_FALLBACKS} models`, true);
      return;
    }
    setFallbacks([...curFallbacks, id]);
  }

  function removeFallback(id: string) {
    setFallbacks(curFallbacks.filter((m) => m !== id));
  }

  function moveFallback(i: number, delta: -1 | 1) {
    const j = i + delta;
    if (j < 0 || j >= curFallbacks.length) return;
    const next = [...curFallbacks];
    [next[i], next[j]] = [next[j], next[i]];
    setFallbacks(next);
  }

  function discard() {
    setDraftDefault(null);
    setDraftFallbacks(null);
  }

  async function stage() {
    if (!dirty || busy) return;
    setBusy(true);
    try {
      const body: { default_model?: string; fallback_models?: string[] } = {};
      if (defaultChanged) body.default_model = curDefault;
      if (fallbacksChanged) body.fallback_models = curFallbacks;
      const p = await proposalsApi.createModelConfig(body);
      toast(
        `staged as ${p.id} — approve on the daemon console: /proposal approve ${p.id}`,
      );
      discard();
      refresh();
    } catch (e) {
      toast(`staging failed: ${e instanceof Error ? e.message : e}`, true);
    } finally {
      setBusy(false);
    }
  }

  const groups = (data?.providers ?? []).filter((g) => g.models.length > 0);

  return (
    <>
      <div className="page-head">
        <h1>Models</h1>
        <span className="sub">
          catalog and the effective default/fallback ladder — changes are
          staged here, applied only on the console
        </span>
        <span className="spacer" />
        <button onClick={refresh}>refresh</button>
      </div>
      <div className="page-body">
        {!tokens.owner ? (
          <Empty start="NO TOKEN">
            model selection is for the operator — set the owner token under
            Connection first
          </Empty>
        ) : error ? (
          <div className="error-line">
            models unavailable: {error}{" "}
            <button onClick={refresh} style={{ marginLeft: 8 }}>
              retry
            </button>
          </div>
        ) : data === null ? (
          <Section title="Effective configuration">
            <Row title="loading model catalog…" />
          </Section>
        ) : (
          <>
            <Section title="Effective configuration">
              <Row
                title={curDefault || "no default model"}
                desc={
                  defaultChanged
                    ? `draft — the daemon still runs ${effDefault}`
                    : "the model every new turn starts on"
                }
              >
                <span className={`chip ${defaultChanged ? "escalated" : "succeeded"}`}>
                  {defaultChanged ? "draft default" : "default"}
                </span>
              </Row>
              {curFallbacks.length === 0 ? (
                <Row
                  title="no fallback models"
                  desc="add catalog models below to build the ladder tried when the default fails"
                />
              ) : (
                curFallbacks.map((m, i) => (
                  <Row key={m} title={`${i + 1}. ${m}`} desc="fallback">
                    <button
                      onClick={() => moveFallback(i, -1)}
                      disabled={i === 0}
                      aria-label={`move ${m} up`}
                    >
                      ↑
                    </button>
                    <button
                      onClick={() => moveFallback(i, 1)}
                      disabled={i === curFallbacks.length - 1}
                      aria-label={`move ${m} down`}
                    >
                      ↓
                    </button>
                    <button className="danger" onClick={() => removeFallback(m)}>
                      remove
                    </button>
                  </Row>
                ))
              )}
              {dirty && (
                <Row
                  title="Stage this change"
                  desc="creates a pending model_config proposal; apply it on the daemon console with /proposal approve <id>"
                >
                  <button onClick={discard} disabled={busy}>
                    discard
                  </button>
                  <button className="primary" onClick={stage} disabled={busy}>
                    {busy ? "staging…" : "stage proposal"}
                  </button>
                </Row>
              )}
            </Section>

            <RoutingSection catalog={data} tick={configTick} />

            {groups.map((g) => (
              <Section
                key={g.provider_kind}
                title={`${KIND_NAME[g.provider_kind] ?? g.provider_kind} — ${g.models.length} model${g.models.length === 1 ? "" : "s"}`}
              >
                {g.models.map((m) => {
                  const isDefault = m.id === curDefault;
                  const fbIndex = curFallbacks.indexOf(m.id);
                  return (
                    <Row key={m.id} title={m.display_name} desc={m.id}>
                      {isDefault ? (
                        <span className="chip succeeded">default</span>
                      ) : (
                        <button onClick={() => setDraftDefault(m.id)}>
                          set default
                        </button>
                      )}
                      {fbIndex >= 0 ? (
                        <>
                          <span className="chip paused">
                            fallback #{fbIndex + 1}
                          </span>
                          <button
                            className="danger"
                            onClick={() => removeFallback(m.id)}
                          >
                            remove
                          </button>
                        </>
                      ) : (
                        !isDefault && (
                          <button onClick={() => addFallback(m.id)}>
                            + fallback
                          </button>
                        )
                      )}
                    </Row>
                  );
                })}
              </Section>
            ))}

            <Section title="Staged model proposals">
              {proposals.length === 0 ? (
                <Row
                  title="none yet"
                  desc="staged default/fallback changes appear here with their status"
                />
              ) : (
                proposals.map((p) => (
                  <Row
                    key={p.id}
                    title={`${
                      p.payload.kind === "model_config"
                        ? [
                            p.payload.default_model
                              ? `default → ${p.payload.default_model}`
                              : null,
                            p.payload.fallback_models
                              ? `fallbacks → [${p.payload.fallback_models.join(", ")}]`
                              : null,
                          ]
                            .filter(Boolean)
                            .join(" · ")
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
                ))
              )}
            </Section>

            <AuditStrip tick={configTick} />
          </>
        )}
      </div>
    </>
  );
}
