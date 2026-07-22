import { useCallback, useEffect, useState } from "react";
import {
  ApiError,
  PersonaContract,
  Proposal,
  personas,
  proposalsApi,
  request2,
  tokens,
} from "../api";
import { LoadoutData } from "./Loadout";
import { Empty, Row, Section, useToast } from "../ui";

// Personas are the agent's constitution — the web STAGES changes, it never
// applies them. A submitted edit becomes a pending proposal the operator
// reviews and approves on the daemon console (/proposal approve <id>),
// with a backup written beside the replaced SOUL.md. Authority explicit.
//
// C0-P4: the PRIMARY editor is a structured contract-v2 form (name,
// description, objectives, goals, tools allowlist-or-unrestricted, scope,
// privacy tier, weight, skills) built from `GET /personas/{slug}`'s parsed
// `contract`. A "raw" mode toggle stays as the advanced escape hatch — the
// ONLY usable mode for a SOUL.md that fails to parse at all (`contract:
// null`), since there is nothing to seed form fields from.

const PRIVACY_TIERS = ["local-only", "cloud-ok"] as const;

interface FormState {
  name: string;
  description: string;
  objectives: string[];
  goals: string[];
  toolsMode: "unrestricted" | "allowlist";
  toolsList: string[];
  scope: string;
  privacyTier: string;
  weight: string;
  skills: string[];
}

function defaultForm(slug: string): FormState {
  return {
    name: slug,
    description: "",
    objectives: [],
    goals: [],
    toolsMode: "unrestricted",
    toolsList: [],
    scope: "",
    privacyTier: "local-only",
    weight: "0.5",
    skills: [],
  };
}

function formFromContract(c: PersonaContract): FormState {
  return {
    name: c.name,
    description: c.description ?? "",
    objectives: [...c.objectives],
    goals: [...c.goals],
    toolsMode: c.tools === null ? "unrestricted" : "allowlist",
    toolsList: c.tools ? [...c.tools] : [],
    scope: c.scope ?? "",
    privacyTier: c.privacy_tier || "local-only",
    weight: String(c.weight),
    skills: [...c.skills],
  };
}

function sameForm(a: FormState, b: FormState): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/** Mirrors `PersonaFront::validate()` in bastion-core (objectives/goals
 * non-empty, scope present, an explicit tools allowlist non-empty) plus one
 * client-only nicety (non-empty name) the backend doesn't itself require —
 * `name: ""` still parses as a valid (if useless) string. */
function validateForm(f: FormState): Record<string, string> {
  const errors: Record<string, string> = {};
  if (!f.name.trim()) errors.name = "name is required";
  if (f.objectives.filter((x) => x.trim()).length === 0) {
    errors.objectives = "declare at least one objective";
  }
  if (f.goals.filter((x) => x.trim()).length === 0) {
    errors.goals = "declare at least one goal";
  }
  if (!f.scope.trim()) {
    errors.scope = "declare this persona's operating scope";
  }
  if (
    f.toolsMode === "allowlist" &&
    f.toolsList.filter((x) => x.trim()).length === 0
  ) {
    errors.tools =
      "list at least one allowed capability, or switch to unrestricted";
  }
  if (!Number.isFinite(Number(f.weight)) || f.weight.trim() === "") {
    errors.weight = "weight must be a number";
  }
  return errors;
}

/** A double-quoted YAML scalar (JSON's escaping rules ARE valid YAML
 * double-quoted-scalar escaping) for anything that isn't a safe bare word —
 * simpler and more robust than hand-rolling bare-word detection edge cases,
 * at the cost of quoting a few strings a human author wouldn't have. */
function yamlScalar(s: string): string {
  const bare =
    /^[A-Za-z0-9_][A-Za-z0-9 _.,!?'()/-]*$/.test(s) &&
    s === s.trim() &&
    !/^(true|false|null|yes|no|on|off)$/i.test(s) &&
    !s.includes(": ") &&
    !s.endsWith(":");
  return bare ? s : JSON.stringify(s);
}

function yamlListField(key: string, items: string[]): string {
  if (items.length === 0) return `${key}: []`;
  return `${key}:\n` + items.map((i) => `  - ${yamlScalar(i)}`).join("\n");
}

/** Assembles the FULL SOUL.md text from the structured form plus the
 * preserved markdown body (everything after the frontmatter's closing
 * `---` in the original file) — the form edits the frontmatter only, the
 * persona's prose is never touched. */
function assembleSoul(f: FormState, body: string): string {
  const lines: string[] = [];
  lines.push(`name: ${yamlScalar(f.name.trim())}`);
  const description = f.description.trim();
  if (description) lines.push(`description: ${yamlScalar(description)}`);
  lines.push("bastion:");
  lines.push(`  privacy_tier: ${f.privacyTier}`);
  lines.push(`  weight: ${Number(f.weight)}`);
  lines.push(
    yamlListField(
      "objectives",
      f.objectives.map((s) => s.trim()).filter(Boolean),
    ),
  );
  lines.push(
    yamlListField("goals", f.goals.map((s) => s.trim()).filter(Boolean)),
  );
  if (f.toolsMode === "allowlist") {
    lines.push(
      yamlListField(
        "tools",
        f.toolsList.map((s) => s.trim()).filter(Boolean),
      ),
    );
  }
  lines.push(`scope: ${yamlScalar(f.scope.trim())}`);
  const skills = f.skills.map((s) => s.trim()).filter(Boolean);
  if (skills.length > 0) lines.push(yamlListField("skills", skills));
  return `---\n${lines.join("\n")}\n---\n\n${body}`;
}

/** Mirrors bastion-core's `parse_soul` splitting logic (strip the leading
 * `---`, split at the closing `\n---`) purely to recover the markdown body
 * so a form save preserves it untouched. Only called when the source text
 * is known to parse (an existing persona's `contract` was non-null, or the
 * "new persona" default draft this view itself generates) — never on
 * arbitrary/unparseable text. */
function extractBody(raw: string): string {
  if (!raw.startsWith("---")) return raw.trim();
  const rest = raw.slice(3);
  const idx = rest.indexOf("\n---");
  if (idx < 0) return raw.trim();
  // mirror Rust's `prose.trim_start()` — ALL leading whitespace, not just
  // one newline, so repeated form-save round-trips don't accrete blank
  // lines between the frontmatter and the body.
  return rest.slice(idx + 4).replace(/^\s+/, "");
}

function defaultDraftRaw(slug: string): string {
  return `---\nname: ${slug}\ndescription: what this persona is for\n---\n\nYou are ${slug}. `;
}

export default function Personas({ configTick }: { configTick: number }) {
  const [slugs, setSlugs] = useState<string[] | null>(null);
  const [availableTools, setAvailableTools] = useState<string[]>([]);
  const [items, setItems] = useState<Proposal[]>([]);

  const [selected, setSelected] = useState<string | null>(null);
  const [mode, setMode] = useState<"form" | "raw">("form");
  const [unparseable, setUnparseable] = useState(false);
  const [serverProblems, setServerProblems] = useState<string[]>([]);
  const [rawContent, setRawContent] = useState("");
  const [originalRaw, setOriginalRaw] = useState("");
  const [form, setForm] = useState<FormState | null>(null);
  const [originalForm, setOriginalForm] = useState<FormState | null>(null);
  const [attempted, setAttempted] = useState(false);
  const [submitProblems, setSubmitProblems] = useState<string[]>([]);
  const [customToolDraft, setCustomToolDraft] = useState("");

  const [newSlug, setNewSlug] = useState("");
  const [busy, setBusy] = useState(false);
  const toast = useToast();

  const refresh = useCallback(async () => {
    if (!tokens.owner) return;
    try {
      const [p, props, loadout] = await Promise.all([
        personas.list(),
        proposalsApi.list(),
        request2<LoadoutData>(tokens.owner, "/loadout").catch(
          () => null as LoadoutData | null,
        ),
      ]);
      setSlugs(p.items);
      // this view narrates persona edits only — model/provider proposals
      // live under Models and Providers
      setItems(props.items.filter((x) => x.payload.kind === "persona_edit"));
      if (loadout) setAvailableTools(loadout.tools);
    } catch (e) {
      toast(`personas unavailable: ${e instanceof Error ? e.message : e}`, true);
    }
  }, [toast]);

  useEffect(() => {
    refresh();
  }, [refresh, configTick]);

  function resetEditorState() {
    setAttempted(false);
    setSubmitProblems([]);
    setCustomToolDraft("");
  }

  async function openPersona(slug: string) {
    try {
      const p = await personas.read(slug);
      setSelected(slug);
      setRawContent(p.content);
      setOriginalRaw(p.content);
      setServerProblems(p.problems);
      resetEditorState();
      if (p.contract) {
        setUnparseable(false);
        const f = formFromContract(p.contract);
        setForm(f);
        setOriginalForm(f);
        setMode("form");
      } else {
        setUnparseable(true);
        setForm(null);
        setOriginalForm(null);
        setMode("raw");
      }
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
    const raw = defaultDraftRaw(slug);
    const f: FormState = { ...defaultForm(slug), description: "what this persona is for" };
    setSelected(slug);
    setRawContent(raw);
    setOriginalRaw("");
    setServerProblems([]);
    setUnparseable(false);
    setForm(f);
    setOriginalForm(null);
    setMode("form");
    resetEditorState();
    setNewSlug("");
  }

  function switchMode(next: "form" | "raw") {
    if (next === mode) return;
    if (next === "raw" && form) {
      // Entering raw mode always regenerates the buffer from the current
      // form state — the raw view is the assembled preview PLUS an escape
      // hatch to hand-edit further.
      setRawContent(assembleSoul(form, extractBody(rawContent)));
    } else if (next === "form" && unparseable) {
      toast(
        "this file doesn't parse yet — fix it in raw mode, stage it, and reopen the persona once it's approved",
        true,
      );
      return;
    } else if (next === "form") {
      toast(
        "switching to structured mode keeps the form's last values — raw edits made since aren't pulled back in",
        true,
      );
    }
    setMode(next);
  }

  const formErrors = form ? validateForm(form) : {};
  const dirty =
    mode === "raw"
      ? rawContent !== originalRaw
      : form !== null && (originalForm === null || !sameForm(form, originalForm));

  async function stage() {
    if (!selected || busy || !dirty) return;
    let content: string;
    if (mode === "form") {
      setAttempted(true);
      if (!form || Object.keys(formErrors).length > 0) {
        toast("fix the highlighted fields before staging", true);
        return;
      }
      content = assembleSoul(form, extractBody(rawContent));
    } else {
      content = rawContent;
    }
    setBusy(true);
    setSubmitProblems([]);
    try {
      const p = await proposalsApi.create(selected, content);
      toast(
        `staged as ${p.id} — approve on the daemon console: /proposal approve ${p.id}`,
      );
      setOriginalRaw(content);
      if (mode === "form" && form) setOriginalForm(form);
      refresh();
    } catch (e) {
      if (e instanceof ApiError && e.problems && e.problems.length > 0) {
        setSubmitProblems(e.problems);
        toast("the daemon rejected this contract — see problems below", true);
      } else {
        toast(`staging failed: ${e instanceof Error ? e.message : e}`, true);
      }
    } finally {
      setBusy(false);
    }
  }

  function updateForm(patch: Partial<FormState>) {
    setForm((f) => (f ? { ...f, ...patch } : f));
  }

  function toggleTool(name: string, on: boolean) {
    if (!form) return;
    const list = on
      ? [...form.toolsList, name]
      : form.toolsList.filter((t) => t !== name);
    updateForm({ toolsList: list });
  }

  function addCustomTool() {
    const name = customToolDraft.trim();
    if (!name || !form) return;
    if (!form.toolsList.includes(name)) {
      updateForm({ toolsList: [...form.toolsList, name] });
    }
    setCustomToolDraft("");
  }

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

            {selected && form !== null && (
              <Section
                title={`Editing ${selected} — staged, never applied from here`}
              >
                <Row title="Editor mode" desc="the structured form is the primary editor; raw is the advanced escape hatch">
                  <div className="mode-toggle" role="group" aria-label="editor mode">
                    <button
                      aria-pressed={mode === "form"}
                      disabled={unparseable}
                      onClick={() => switchMode("form")}
                    >
                      structured
                    </button>
                    <button
                      aria-pressed={mode === "raw"}
                      onClick={() => switchMode("raw")}
                    >
                      raw SOUL.md
                    </button>
                  </div>
                </Row>

                {unparseable && (
                  <div className="contract-banner bad">
                    <strong>this SOUL.md could not be parsed</strong> — fix
                    it in raw mode below, then stage it. Problem:
                    <ul>
                      {serverProblems.map((p) => (
                        <li key={p}>{p}</li>
                      ))}
                    </ul>
                  </div>
                )}
                {!unparseable && serverProblems.length > 0 && (
                  <div className="contract-banner">
                    <strong>upgrade this persona to contract v2</strong> —
                    the current SOUL.md is missing:
                    <ul>
                      {serverProblems.map((p) => (
                        <li key={p}>{p}</li>
                      ))}
                    </ul>
                  </div>
                )}
                {submitProblems.length > 0 && (
                  <div className="contract-banner bad">
                    <strong>the daemon rejected this contract</strong>
                    <ul>
                      {submitProblems.map((p) => (
                        <li key={p}>{p}</li>
                      ))}
                    </ul>
                  </div>
                )}

                {mode === "raw" ? (
                  <div className="row">
                    <textarea
                      value={rawContent}
                      onChange={(e) => setRawContent(e.target.value)}
                      rows={16}
                      spellCheck={false}
                      aria-label="SOUL.md content"
                      style={{ fontFamily: "var(--mono)", fontSize: 12.5 }}
                    />
                  </div>
                ) : (
                  <>
                    <Row title="Name" desc="required">
                      <input
                        value={form.name}
                        onChange={(e) => updateForm({ name: e.target.value })}
                        aria-label="persona name"
                      />
                    </Row>
                    {attempted && formErrors.name && (
                      <div className="field-error" style={{ padding: "0 16px 8px" }}>
                        {formErrors.name}
                      </div>
                    )}

                    <Row title="Description" desc="optional">
                      <input
                        value={form.description}
                        onChange={(e) =>
                          updateForm({ description: e.target.value })
                        }
                        aria-label="persona description"
                      />
                    </Row>

                    <StringListField
                      label="Objectives"
                      desc="what this persona is FOR — required, at least one"
                      items={form.objectives}
                      onChange={(objectives) => updateForm({ objectives })}
                      error={attempted ? formErrors.objectives : undefined}
                    />

                    <StringListField
                      label="Goals"
                      desc="this persona's declared goals — required, at least one"
                      items={form.goals}
                      onChange={(goals) => updateForm({ goals })}
                      error={attempted ? formErrors.goals : undefined}
                    />

                    <Row
                      title="Tools"
                      desc="unrestricted (no allowlist) or a non-empty capability allowlist"
                    >
                      <div className="mode-toggle" role="group" aria-label="tools mode">
                        <button
                          aria-pressed={form.toolsMode === "unrestricted"}
                          onClick={() => updateForm({ toolsMode: "unrestricted" })}
                        >
                          unrestricted
                        </button>
                        <button
                          aria-pressed={form.toolsMode === "allowlist"}
                          onClick={() => updateForm({ toolsMode: "allowlist" })}
                        >
                          allowlist
                        </button>
                      </div>
                    </Row>
                    {attempted && formErrors.tools && (
                      <div className="field-error" style={{ padding: "0 16px 8px" }}>
                        {formErrors.tools}
                      </div>
                    )}
                    {form.toolsMode === "allowlist" && (
                      <>
                        {availableTools.map((t) => (
                          <Row key={t} title={t}>
                            <input
                              type="checkbox"
                              checked={form.toolsList.includes(t)}
                              onChange={(e) => toggleTool(t, e.target.checked)}
                              aria-label={`allow ${t}`}
                            />
                          </Row>
                        ))}
                        {form.toolsList
                          .filter((t) => !availableTools.includes(t))
                          .map((t) => (
                            <Row key={t} title={`${t} (custom)`}>
                              <button
                                className="danger"
                                onClick={() => toggleTool(t, false)}
                              >
                                remove
                              </button>
                            </Row>
                          ))}
                        <Row
                          title="Add a custom capability"
                          desc="for capability names not in this loadout's catalog"
                        >
                          <input
                            value={customToolDraft}
                            onChange={(e) => setCustomToolDraft(e.target.value)}
                            placeholder="capability id"
                            aria-label="custom capability id"
                          />
                          <button onClick={addCustomTool}>add</button>
                        </Row>
                      </>
                    )}

                    <Row title="Scope" desc="required — this persona's operating scope">
                      <textarea
                        value={form.scope}
                        onChange={(e) => updateForm({ scope: e.target.value })}
                        rows={3}
                        aria-label="persona scope"
                      />
                    </Row>
                    {attempted && formErrors.scope && (
                      <div className="field-error" style={{ padding: "0 16px 8px" }}>
                        {formErrors.scope}
                      </div>
                    )}

                    <Row title="Privacy tier">
                      <select
                        value={form.privacyTier}
                        onChange={(e) =>
                          updateForm({ privacyTier: e.target.value })
                        }
                        aria-label="privacy tier"
                      >
                        {PRIVACY_TIERS.map((t) => (
                          <option key={t} value={t}>
                            {t}
                          </option>
                        ))}
                      </select>
                    </Row>

                    <Row title="Weight">
                      <input
                        type="number"
                        step="0.1"
                        value={form.weight}
                        onChange={(e) => updateForm({ weight: e.target.value })}
                        aria-label="persona weight"
                      />
                    </Row>
                    {attempted && formErrors.weight && (
                      <div className="field-error" style={{ padding: "0 16px 8px" }}>
                        {formErrors.weight}
                      </div>
                    )}

                    <StringListField
                      label="Skills"
                      desc="optional"
                      items={form.skills}
                      onChange={(skills) => updateForm({ skills })}
                    />
                  </>
                )}

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

/** An editable string list — add/remove/reorder rows — for objectives,
 * goals, and skills. Not built on `<Row>` directly for the item rows: `Row`
 * only accepts a string `title`, and these rows need a live `<input>` in
 * that slot, so they reuse the same `.row`/`.txt`/`.ctl` markup by hand. */
function StringListField({
  label,
  desc,
  items,
  onChange,
  error,
}: {
  label: string;
  desc: string;
  items: string[];
  onChange: (items: string[]) => void;
  error?: string;
}) {
  const [draft, setDraft] = useState("");

  function update(i: number, v: string) {
    const next = [...items];
    next[i] = v;
    onChange(next);
  }
  function remove(i: number) {
    onChange(items.filter((_, j) => j !== i));
  }
  function move(i: number, delta: -1 | 1) {
    const j = i + delta;
    if (j < 0 || j >= items.length) return;
    const next = [...items];
    [next[i], next[j]] = [next[j], next[i]];
    onChange(next);
  }
  function add() {
    const v = draft.trim();
    if (!v) return;
    onChange([...items, v]);
    setDraft("");
  }

  return (
    <>
      <Row title={label} desc={desc} />
      {items.map((v, i) => (
        <div className="row list-row" key={i}>
          <div className="txt">
            <input
              value={v}
              onChange={(e) => update(i, e.target.value)}
              aria-label={`${label} ${i + 1}`}
            />
          </div>
          <div className="ctl">
            <button onClick={() => move(i, -1)} disabled={i === 0} aria-label="move up">
              ↑
            </button>
            <button
              onClick={() => move(i, 1)}
              disabled={i === items.length - 1}
              aria-label="move down"
            >
              ↓
            </button>
            <button className="danger" onClick={() => remove(i)}>
              remove
            </button>
          </div>
        </div>
      ))}
      <div className="row list-row">
        <div className="txt">
          <input
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                add();
              }
            }}
            placeholder={`add ${label.toLowerCase().replace(/s$/, "")}`}
            aria-label={`add ${label}`}
          />
        </div>
        <div className="ctl">
          <button onClick={add}>add</button>
        </div>
      </div>
      {error && (
        <div className="field-error" style={{ padding: "0 16px 8px" }}>
          {error}
        </div>
      )}
    </>
  );
}
