import { useCallback, useEffect, useState } from "react";
import { personas, Proposal, proposalsApi, tokens } from "../api";
import { Empty, Row, Section, useToast } from "../ui";

// Personas are the agent's constitution — the web STAGES changes, it never
// applies them. A submitted edit becomes a pending proposal the operator
// reviews and approves on the daemon console (/proposal approve <id>),
// with a backup written beside the replaced SOUL.md. Authority explicit.

export default function Personas() {
  const [slugs, setSlugs] = useState<string[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [content, setContent] = useState("");
  const [original, setOriginal] = useState("");
  const [newSlug, setNewSlug] = useState("");
  const [items, setItems] = useState<Proposal[]>([]);
  const [busy, setBusy] = useState(false);
  const toast = useToast();

  const refresh = useCallback(async () => {
    if (!tokens.owner) return;
    try {
      const [p, props] = await Promise.all([
        personas.list(),
        proposalsApi.list(),
      ]);
      setSlugs(p.items);
      // this view narrates persona edits only — model/provider proposals
      // live under Models and Providers
      setItems(props.items.filter((x) => x.payload.kind === "persona_edit"));
    } catch (e) {
      toast(`personas unavailable: ${e instanceof Error ? e.message : e}`, true);
    }
  }, [toast]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  async function openPersona(slug: string) {
    try {
      const p = await personas.read(slug);
      setSelected(slug);
      setContent(p.content);
      setOriginal(p.content);
    } catch (e) {
      toast(`read failed: ${e instanceof Error ? e.message : e}`, true);
    }
  }

  function startNew() {
    const slug = newSlug.trim();
    if (!/^[a-zA-Z0-9_-]{1,64}$/.test(slug)) {
      toast("slug must be letters, digits, - or _ (max 64)", true);
      return;
    }
    setSelected(slug);
    setOriginal("");
    setContent(
      `---\nname: ${slug}\ndescription: what this persona is for\n---\n\nYou are ${slug}. `,
    );
    setNewSlug("");
  }

  async function stage() {
    if (!selected || busy) return;
    setBusy(true);
    try {
      const p = await proposalsApi.create(selected, content);
      toast(
        `staged as ${p.id} — approve on the daemon console: /proposal approve ${p.id}`,
      );
      refresh();
    } catch (e) {
      toast(`staging failed: ${e instanceof Error ? e.message : e}`, true);
    } finally {
      setBusy(false);
    }
  }

  const dirty = content !== original;

  return (
    <>
      <div className="page-head">
        <h1>Personas</h1>
        <span className="sub">
          the constitution — edits are staged here, applied only on the
          console
        </span>
        <span className="spacer" />
        <button onClick={refresh}>refresh</button>
      </div>
      <div className="page-body">
        {!tokens.owner ? (
          <Empty start="NO TOKEN">
            set the owner token under Connection first
          </Empty>
        ) : (
          <>
            <Section title="Loaded personas (./personas/<slug>/SOUL.md)">
              {slugs === null ? (
                <Row title="loading…" />
              ) : slugs.length === 0 ? (
                <Row
                  title="no personas on disk"
                  desc="create one below — it ships as a staged proposal"
                />
              ) : (
                slugs.map((s) => (
                  <Row key={s} title={s}>
                    <button onClick={() => openPersona(s)}>
                      {selected === s ? "editing" : "edit"}
                    </button>
                  </Row>
                ))
              )}
              <Row
                title="New persona"
                desc="a slug becomes personas/<slug>/SOUL.md"
              >
                <input
                  style={{ width: "18ch" }}
                  value={newSlug}
                  onChange={(e) => setNewSlug(e.target.value)}
                  placeholder="slug"
                  aria-label="new persona slug"
                />
                <button onClick={startNew}>draft</button>
              </Row>
            </Section>

            {selected && (
              <Section title={`Editing ${selected} — staged, never applied from here`}>
                <div className="row">
                  <textarea
                    value={content}
                    onChange={(e) => setContent(e.target.value)}
                    rows={16}
                    spellCheck={false}
                    aria-label="SOUL.md content"
                    style={{ fontFamily: "var(--mono)", fontSize: 12.5 }}
                  />
                </div>
                <Row
                  title="Stage this change"
                  desc="creates a pending proposal; apply it on the daemon console with /proposal approve <id> (a backup of the previous SOUL.md is kept)"
                >
                  <button
                    className="primary"
                    disabled={!dirty || busy}
                    onClick={stage}
                  >
                    {busy ? "staging…" : "stage proposal"}
                  </button>
                </Row>
              </Section>
            )}

            <Section title="Proposals">
              {items.length === 0 ? (
                <Row title="none yet" desc="staged changes appear here with their status" />
              ) : (
                items.map((p) => (
                  <Row
                    key={p.id}
                    title={`${p.payload.kind === "persona_edit" ? p.payload.slug : p.payload.kind} — ${p.id}`}
                    desc={`${p.origin} · ${new Date(p.created_at / 1e6).toLocaleString()}${
                      p.status === "pending"
                        ? ` · approve on console: /proposal approve ${p.id}`
                        : ""
                    }`}
                  >
                    <span className={`chip ${p.status === "pending" ? "running" : p.status === "approved" ? "succeeded" : "failed"}`}>
                      {p.status}
                    </span>
                  </Row>
                ))
              )}
            </Section>
          </>
        )}
      </div>
    </>
  );
}
