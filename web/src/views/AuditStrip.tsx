import { useCallback, useEffect, useState } from "react";
import { ConfigOverride, configApi, tokens } from "../api";
import { Row, Section } from "../ui";

// The audit strip: GET /config/overrides rendered small. Every applied
// runtime change — console command or approved web proposal — lands in the
// same audited table, and this strip makes that single write path visible.

const MAX_SHOWN = 8;

export function relTime(unixSecs: number): string {
  const delta = Math.max(0, Math.floor(Date.now() / 1000) - unixSecs);
  if (delta < 60) return `${delta}s ago`;
  if (delta < 3600) return `${Math.floor(delta / 60)}m ago`;
  if (delta < 86400) return `${Math.floor(delta / 3600)}h ago`;
  return `${Math.floor(delta / 86400)}d ago`;
}

export default function AuditStrip({ tick }: { tick: number }) {
  const [items, setItems] = useState<ConfigOverride[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!tokens.owner) return;
    try {
      const r = await configApi.overrides();
      setItems(
        [...r.items].sort((a, b) => b.applied_at - a.applied_at).slice(0, MAX_SHOWN),
      );
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh, tick]);

  return (
    <Section title="Config audit — the single write path">
      {error ? (
        <Row title="audit unavailable" desc={error}>
          <button onClick={refresh}>retry</button>
        </Row>
      ) : items === null ? (
        <Row title="loading audit trail…" />
      ) : items.length === 0 ? (
        <Row
          title="no applied overrides yet"
          desc="every applied change — console command or approved proposal — is recorded here"
        />
      ) : (
        items.map((o, i) => (
          <Row
            key={`${o.key}-${o.applied_at}-${i}`}
            title={o.key}
            desc={`origin: ${o.origin}`}
          >
            <span style={{ color: "var(--dim)", fontSize: 12 }}>
              {relTime(o.applied_at)}
            </span>
          </Row>
        ))
      )}
    </Section>
  );
}
