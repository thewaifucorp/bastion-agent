import { FormEvent, useState } from "react";
import { tokens } from "../api";
import { Dot, Row, Section, useToast } from "../ui";

export default function Connection({ onSaved }: { onSaved: () => void }) {
  const [owner, setOwner] = useState(tokens.owner);
  const [cp, setCp] = useState(tokens.cp);
  const toast = useToast();

  function save(e: FormEvent) {
    e.preventDefault();
    tokens.owner = owner.trim();
    tokens.cp = cp.trim();
    toast("saved — reconnecting");
    onSaved();
  }

  return (
    <>
      <div className="page-head">
        <h1>Connection</h1>
        <span className="sub">
          two tokens, two surfaces — both stay in this browser only
        </span>
      </div>
      <div className="page-body">
        <form onSubmit={save}>
          <Section title="Owner token — live feed & chat">
            <Row
              title="Owner token"
              desc="authenticates /events and /webhook; the same token your webhook channel uses (OwnerMap in bastion.toml/.env), or a paired-device JWT"
            >
              <input
                type="password"
                autoComplete="off"
                value={owner}
                onChange={(e) => setOwner(e.target.value)}
                aria-label="owner token"
              />
            </Row>
            <Row title="Status">
              <Dot
                state={tokens.owner ? "ok" : "off"}
                label={tokens.owner ? "configured" : "not set"}
              />
            </Row>
          </Section>

          <Section title="Control Plane credential — tasks API">
            <Row
              title="bcp_ credential"
              desc="authenticates /v1/* (task table, pause/resume/steer/cancel). Scoped and revocable."
            >
              <input
                type="password"
                autoComplete="off"
                value={cp}
                onChange={(e) => setCp(e.target.value)}
                aria-label="control plane credential"
              />
            </Row>
            <Row
              title="How to issue one"
              desc="on the daemon console — the token is printed exactly once"
            >
              <code>/credential issue dashboard tasks:read,tasks:control</code>
            </Row>
            <Row title="Status">
              <Dot
                state={tokens.cp ? "ok" : "off"}
                label={tokens.cp ? "configured" : "not set"}
              />
            </Row>
          </Section>

          <Section title="Privacy">
            <Row
              title="Where tokens live"
              desc="localStorage of this browser; sent only to this daemon (same origin). The page's CSP blocks every other destination."
            />
            <Row title="Apply">
              <button type="submit" className="primary">
                save and connect
              </button>
            </Row>
          </Section>
        </form>
      </div>
    </>
  );
}
