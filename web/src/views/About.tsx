import { useEffect, useState } from "react";
import { agentCard } from "../api";
import { Dot, Row, Section } from "../ui";

export default function About() {
  const [card, setCard] = useState<Record<string, unknown> | null>(null);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    agentCard().then((c) => {
      setCard(c);
      setLoaded(true);
    });
  }, []);

  return (
    <>
      <div className="page-head">
        <h1>About</h1>
        <span className="sub">this daemon, its surfaces, its identity</span>
      </div>
      <div className="page-body">
        <Section title="Surfaces served by this daemon">
          <Row title="GET /app" desc="this web app (embedded at build time)" />
          <Row
            title="GET /ui"
            desc="zero-build fallback dashboard, always available"
          />
          <Row
            title="GET /events"
            desc="live SSE feed — turns, personas, task lifecycle (owner token)"
          />
          <Row
            title="POST /webhook"
            desc="one turn per call; Remote slash commands included (owner token)"
          />
          <Row
            title="/v1/*"
            desc="Control Plane task API — frozen OpenAPI contract at /v1/openapi.yaml (bcp_ credential)"
          />
          <Row
            title="GET /status · /healthz · /readyz"
            desc="operational booleans, unauthenticated"
          />
        </Section>

        <Section title="Agent identity (mesh)">
          {!loaded ? (
            <Row title="loading…" />
          ) : card === null ? (
            <Row
              title="No agent card"
              desc="mesh identity is not configured (MESH_IDENTITY_KEY) — pairing and signed cards are off"
            >
              <Dot state="off" label="disabled" />
            </Row>
          ) : (
            <>
              <Row title="Agent" desc="signed agent card is being served">
                <Dot state="ok" label={String(card.name ?? "configured")} />
              </Row>
              {"capabilities" in card && Array.isArray(card.capabilities) && (
                <Row
                  title="Capabilities"
                  desc={(card.capabilities as string[]).join(", ")}
                />
              )}
            </>
          )}
        </Section>

        <Section title="Project">
          <Row title="Bastion" desc="the agent that can't betray you — self-hosted, keys yours, memory contestable, authority explicit">
            <a href="https://bastion.run" target="_blank" rel="noreferrer">
              bastion.run
            </a>
          </Row>
          <Row title="Source" desc="public repository">
            <a
              href="https://github.com/thewaifucorp/bastion-agent"
              target="_blank"
              rel="noreferrer"
            >
              github
            </a>
          </Row>
        </Section>
      </div>
    </>
  );
}
