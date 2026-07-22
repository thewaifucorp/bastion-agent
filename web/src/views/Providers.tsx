import { FormEvent, useCallback, useEffect, useState } from "react";
import {
  Proposal,
  ProviderItem,
  proposalsApi,
  providersApi,
  tokens,
} from "../api";
import { Dot, Empty, Row, Section, useToast } from "../ui";
import AuditStrip from "./AuditStrip";

// Providers: connection status per provider, booleans only — the daemon
// never reveals key material and neither does this view. Adding an API key
// STAGES a secret_set proposal: the value travels once in the POST body,
// is penned in daemon memory, and only console approval writes it to the
// secrets dir. If the daemon restarts before approval the value expires
// and must be re-submitted.

// S4 cleanup: names and env keys come from GET /providers itself
// (`display_name` / `env_key` — the daemon's src/model_catalog.rs whitelist
// is the single source), so the old frontend mirror table is gone.

const KIND_LABEL: Record<ProviderItem["kind"], string> = {
  api_key: "API key",
  subscription_cli: "Subscription",
  local: "Local",
};

const SOURCE_LABEL: Record<string, string> = {
  env: "environment variable",
  secrets_dir: "secrets dir",
  auth_profile: "auth profile",
};

export default function Providers({ configTick }: { configTick: number }) {
  const [items, setItems] = useState<ProviderItem[] | null>(null);
  const [proposals, setProposals] = useState<Proposal[]>([]);
  const [error, setError] = useState<string | null>(null);
  // key drafts live here ONLY until the POST fires — cleared before await
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [busyId, setBusyId] = useState<string | null>(null);
  const toast = useToast();

  const refresh = useCallback(async () => {
    if (!tokens.owner) return;
    try {
      const [p, props] = await Promise.all([
        providersApi.list(),
        proposalsApi.list(),
      ]);
      setItems(p.items);
      setProposals(props.items.filter((x) => x.payload.kind === "secret_set"));
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh, configTick]);

  function pendingFor(providerId: string): Proposal | undefined {
    return proposals.find(
      (p) =>
        p.status === "pending" &&
        p.payload.kind === "secret_set" &&
        p.payload.provider_id === providerId,
    );
  }

  async function stageKey(e: FormEvent, provider: ProviderItem) {
    e.preventDefault();
    const envKey = provider.env_key;
    const value = (drafts[provider.id] ?? "").trim();
    if (!envKey || !value || busyId) return;
    // drop the secret from state BEFORE the request — it lives only in the
    // POST body from here on
    setDrafts((d) => ({ ...d, [provider.id]: "" }));
    setBusyId(provider.id);
    try {
      const p = await proposalsApi.createSecretSet(provider.id, envKey, value);
      toast(
        `key staged as ${p.id} — approve on the daemon console: /proposal approve ${p.id}`,
      );
      refresh();
    } catch (err) {
      toast(
        `staging failed: ${err instanceof Error ? err.message : err} — re-enter the key`,
        true,
      );
    } finally {
      setBusyId(null);
    }
  }

  return (
    <>
      <div className="page-head">
        <h1>Providers</h1>
        <span className="sub">
          connection status per provider — keys are staged here, applied only
          on the console
        </span>
        <span className="spacer" />
        <button onClick={refresh}>refresh</button>
      </div>
      <div className="page-body">
        {!tokens.owner ? (
          <Empty start="NO TOKEN">
            provider status is for the operator — set the owner token under
            Connection first
          </Empty>
        ) : error ? (
          <div className="error-line">
            providers unavailable: {error}{" "}
            <button onClick={refresh} style={{ marginLeft: 8 }}>
              retry
            </button>
          </div>
        ) : items === null ? (
          <div className="pgrid" aria-hidden="true">
            {[0, 1, 2, 3, 4, 5].map((i) => (
              <div className="pcard skeleton" key={i}>
                <div className="phead">
                  <span className="pname">loading…</span>
                </div>
                <div className="pmeta">probing connection status</div>
              </div>
            ))}
          </div>
        ) : (
          <>
            <div className="pgrid">
              {items.map((p) => (
                <ProviderCard
                  key={p.id}
                  provider={p}
                  pending={pendingFor(p.id)}
                  draft={drafts[p.id] ?? ""}
                  onDraft={(v) => setDrafts((d) => ({ ...d, [p.id]: v }))}
                  onSubmit={(e) => stageKey(e, p)}
                  busy={busyId === p.id}
                />
              ))}
            </div>

            <Section title="Staged key proposals">
              {proposals.length === 0 ? (
                <Row
                  title="none yet"
                  desc="staged API keys appear here with their status — values are never shown"
                />
              ) : (
                proposals.map((p) => {
                  const providerId =
                    p.payload.kind === "secret_set"
                      ? p.payload.provider_id
                      : null;
                  const name = providerId
                    ? (items.find((i) => i.id === providerId)?.display_name ??
                      providerId)
                    : p.payload.kind;
                  return (
                  <Row
                    key={p.id}
                    title={`${name} — ${p.id}`}
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
                  );
                })
              )}
            </Section>

            <AuditStrip tick={configTick} />
          </>
        )}
      </div>
    </>
  );
}

function ProviderCard({
  provider,
  pending,
  draft,
  onDraft,
  onSubmit,
  busy,
}: {
  provider: ProviderItem;
  pending: Proposal | undefined;
  draft: string;
  onDraft: (v: string) => void;
  onSubmit: (e: FormEvent) => void;
  busy: boolean;
}) {
  const status =
    provider.kind === "local"
      ? provider.connected
        ? "in the effective model set"
        : "not in the effective model set"
      : provider.connected
        ? `connected · ${SOURCE_LABEL[provider.source ?? ""] ?? provider.source}`
        : "not connected";

  return (
    <div className="pcard">
      <div className="phead">
        <span className="pname">{provider.display_name}</span>
        <span className="chip">{KIND_LABEL[provider.kind]}</span>
      </div>
      <div className="pmeta">
        <Dot
          state={
            provider.connected ? "ok" : provider.kind === "local" ? "off" : "warn"
          }
          label={status}
        />
      </div>
      <div className="pmeta">
        {provider.kind === "subscription_cli"
          ? "brings its own entitlement — select it as a runtime backend"
          : `${provider.models_count} catalog model${provider.models_count === 1 ? "" : "s"}`}
      </div>

      {provider.kind === "api_key" && !provider.connected && (
        pending ? (
          <div className="pmeta pending-note">
            key staged as {pending.id} — approve on the daemon console:{" "}
            <span className="cmd">/proposal approve {pending.id}</span>. If the
            daemon restarts first, the value expires and must be re-entered.
          </div>
        ) : (
          <form onSubmit={onSubmit}>
            <input
              type="password"
              value={draft}
              onChange={(e) => onDraft(e.target.value)}
              placeholder={provider.env_key ?? "API key"}
              autoComplete="off"
              aria-label={`${provider.display_name} API key`}
            />
            <button
              type="submit"
              className="primary"
              disabled={busy || !draft.trim()}
            >
              {busy ? "staging…" : "add key"}
            </button>
          </form>
        )
      )}

      {provider.kind === "subscription_cli" && !provider.connected && (
        <div className="pmeta">
          connect on the daemon console with{" "}
          <span className="cmd">/connect</span> — the CLI login never passes
          through the web
        </div>
      )}
    </div>
  );
}
