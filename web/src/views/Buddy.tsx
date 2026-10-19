import { useCallback, useEffect, useRef, useState } from "react";
import {
  ApiError,
  CareAction,
  CompanionSnapshot,
  PresenceActionBody,
  PresenceAlert,
  PresenceDevice,
  PresenceSession,
  PresenceSnapshot,
  companionApi,
  personas as personasApi,
  presenceApi,
  tokens,
} from "../api";
import { Dot, Empty, Row, Section, useToast } from "../ui";

// Buddy: the TUI's tamagotchi-style companion, mirrored on the web. The
// daemon is the single writer of companion.json while it's running (A5
// S5) — care actions here go straight through it, same as the TUI's own
// /pet commands. Purely cosmetic: no capabilities unlocked, ever.
//
// Presence lives below the pet: the active persona, the host-agent sessions,
// pending approvals and paired devices (docs/en/presence.md). It refetches the
// snapshot on the presence SSE events (debounced) and polls every 10 s while
// the stream is down.

const PRESENCE_DEBOUNCE_MS = 150;
const PRESENCE_POLL_MS = 10_000;
const NOTE_MINUTES = 30;

const SESSION_DOT: Record<
  PresenceSession["st"],
  "ok" | "warn" | "bad" | "info" | "off"
> = {
  working: "ok",
  waiting: "warn",
  idle: "off",
  done: "info",
  error: "bad",
};

/** "12s", "4m", "2h" since a unix-seconds timestamp. */
function ago(since: number | undefined, now: number): string {
  if (since === undefined) return "";
  const s = Math.max(0, now - since);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  return `${Math.floor(s / 3600)}h`;
}

function countdown(exp: number | undefined, now: number): string {
  if (exp === undefined) return "";
  const s = Math.max(0, exp - now);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

const CARE_ACTIONS: { action: CareAction; label: string }[] = [
  { action: "water", label: "Water" },
  { action: "feed", label: "Feed" },
  { action: "play", label: "Play" },
  { action: "sleep", label: "Sleep" },
];

const NEED_LABEL: Record<keyof CompanionSnapshot["needs"], string> = {
  water: "Water",
  food: "Food",
  play: "Play",
  rest: "Rest",
};

// needs keys (water/food/play/rest) vs. care-action names (water/feed/
// play/sleep) — the daemon's `cues` array uses the latter (same names
// POST /companion/care accepts), so cross-referencing needs a small map.
const CUE_FOR_NEED: Record<keyof CompanionSnapshot["needs"], string> = {
  water: "water",
  food: "feed",
  play: "play",
  rest: "sleep",
};

function needTone(pct: number): string {
  if (pct <= 25) return "bad";
  if (pct <= 60) return "warn";
  return "";
}

export default function Buddy({
  configTick,
  presenceTick,
  live,
}: {
  configTick: number;
  presenceTick: number;
  live: boolean;
}) {
  const [snapshot, setSnapshot] = useState<CompanionSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<CareAction | null>(null);
  const [presence, setPresence] = useState<PresenceSnapshot | null>(null);
  // "unavailable" = /presence/* answered 404: older daemon or presence off
  const [presenceState, setPresenceState] = useState<
    "loading" | "ok" | "unavailable" | "error"
  >("loading");
  const [presenceError, setPresenceError] = useState<string | null>(null);
  const [devices, setDevices] = useState<PresenceDevice[]>([]);
  const [personaList, setPersonaList] = useState<string[]>([]);
  const [acting, setActing] = useState<string | null>(null);
  const [noteText, setNoteText] = useState("");
  // local clock so countdowns and "time since" tick between snapshots
  const [nowSec, setNowSec] = useState(() => Math.floor(Date.now() / 1000));
  const debounceRef = useRef<number | null>(null);
  const toast = useToast();

  const refresh = useCallback(async () => {
    if (!tokens.owner) return;
    try {
      const s = await companionApi.get();
      setSnapshot(s);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh, configTick]);

  const refreshPresence = useCallback(async () => {
    if (!tokens.owner) return;
    try {
      const s = await presenceApi.getSnapshot();
      setPresence(s);
      setNowSec(s.now);
      setPresenceState("ok");
      setPresenceError(null);
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) {
        setPresenceState("unavailable");
      } else {
        setPresenceState("error");
        setPresenceError(e instanceof Error ? e.message : String(e));
      }
    }
  }, []);

  const refreshDevices = useCallback(async () => {
    if (!tokens.owner) return;
    try {
      const r = await presenceApi.getDevices();
      setDevices(r.items ?? r.devices ?? []);
    } catch {
      // read-only extra: a failure just leaves the list empty
      setDevices([]);
    }
  }, []);

  useEffect(() => {
    refreshPresence();
    refreshDevices();
    if (!tokens.owner) return;
    personasApi
      .list()
      .then((r) => setPersonaList(r.items))
      .catch(() => setPersonaList([]));
  }, [refreshPresence, refreshDevices, configTick]);

  // SSE-driven refetch, debounced so a burst of events costs one request
  useEffect(() => {
    if (presenceTick === 0) return undefined;
    if (debounceRef.current !== null) window.clearTimeout(debounceRef.current);
    debounceRef.current = window.setTimeout(() => {
      debounceRef.current = null;
      refreshPresence();
    }, PRESENCE_DEBOUNCE_MS);
    return () => {
      if (debounceRef.current !== null) window.clearTimeout(debounceRef.current);
    };
  }, [presenceTick, refreshPresence]);

  // fallback poll while the event stream is not connected
  useEffect(() => {
    if (live || presenceState === "unavailable") return undefined;
    const id = window.setInterval(refreshPresence, PRESENCE_POLL_MS);
    return () => window.clearInterval(id);
  }, [live, presenceState, refreshPresence]);

  useEffect(() => {
    const id = window.setInterval(
      () => setNowSec((n) => n + 1),
      1000,
    );
    return () => window.clearInterval(id);
  }, []);

  async function act(key: string, body: PresenceActionBody) {
    if (acting) return;
    setActing(key);
    try {
      // the action answers with the updated snapshot
      const next = await presenceApi.action(body);
      setPresence(next);
      setNowSec(next.now);
      if (body.action === "note") setNoteText("");
    } catch (e) {
      toast(
        `${body.action} failed: ${e instanceof Error ? e.message : String(e)}`,
        true,
      );
    } finally {
      setActing(null);
    }
  }

  async function care(action: CareAction) {
    if (busy) return;
    setBusy(action);
    try {
      // Optimistic refresh: the daemon's own response IS the new state —
      // no need for a second round trip.
      const next = await companionApi.care(action);
      setSnapshot(next);
    } catch (e) {
      toast(
        `${action} failed: ${e instanceof Error ? e.message : String(e)}`,
        true,
      );
    } finally {
      setBusy(null);
    }
  }

  function renderPresence() {
    if (presenceState === "unavailable") {
      return (
        <Section title="Presence">
          <Row
            title="Presence is not available"
            desc="this daemon is older than presence support, or [presence] is disabled in its config — see docs/en/presence.md"
          />
        </Section>
      );
    }
    if (presenceState === "error") {
      return (
        <div className="error-line">
          presence unavailable: {presenceError}{" "}
          <button onClick={refreshPresence} style={{ marginLeft: 8 }}>
            retry
          </button>
        </div>
      );
    }
    if (presence === null) {
      return (
        <div className="pcard skeleton" aria-hidden="true">
          <div className="phead">
            <span className="pname">loading…</span>
          </div>
          <div className="pmeta">reading presence state</div>
        </div>
      );
    }
    const sessions = presence.sessions ?? [];
    const alerts = presence.alerts ?? [];
    const approvals = alerts.filter((a) => a.kind === "approval");
    const others = alerts.filter((a) => a.kind !== "approval");
    const personaOptions =
      presence.persona && !personaList.includes(presence.persona)
        ? [presence.persona, ...personaList]
        : personaList;
    const sessionName = (sid: string | undefined) =>
      sessions.find((s) => s.id === sid)?.name;
    // Approvals show the whole call (`input`), never only the display cut.
    const alertDesc = (a: PresenceAlert) =>
      [a.input ?? a.det, sessionName(a.sid)].filter(Boolean).join(" · ") +
      (a.kind === "approval" && !a.input_complete
        ? " — too long to show whole: approve where you can read it"
        : "");

    return (
      <>
        <Section title="Presence">
          <Row
            title="Persona"
            desc={
              presence.host
                ? `active on ${presence.host} — host agents pick it up on their next turn`
                : "active persona for host agents"
            }
          >
            <select
              value={presence.persona ?? ""}
              disabled={acting !== null}
              onChange={(e) =>
                act("persona", {
                  action: "persona",
                  persona: e.target.value === "" ? null : e.target.value,
                })
              }
            >
              <option value="">(default)</option>
              {personaOptions.map((p) => (
                <option key={p} value={p}>
                  {p}
                </option>
              ))}
            </select>
          </Row>
          {presence.today && (
            <Row
              title="Today"
              desc={`${presence.today.turns ?? 0} turns · ${presence.today.tools ?? 0} tool calls · ${presence.today.errors ?? 0} errors`}
            />
          )}
          <Row
            title="Note"
            desc={
              presence.note
                ? `showing "${presence.note.text}" on devices${presence.note.exp ? ` — clears in ${countdown(presence.note.exp, nowSec)}` : ""}`
                : `a short note for your devices; clears after ${NOTE_MINUTES} min`
            }
          >
            <input
              value={noteText}
              maxLength={80}
              placeholder="back in 10 min"
              onChange={(e) => setNoteText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && noteText.trim())
                  act("note", {
                    action: "note",
                    text: noteText.trim(),
                    minutes: NOTE_MINUTES,
                  });
              }}
            />
            <button
              disabled={acting !== null || !noteText.trim()}
              onClick={() =>
                act("note", {
                  action: "note",
                  text: noteText.trim(),
                  minutes: NOTE_MINUTES,
                })
              }
            >
              {acting === "note" ? "Send…" : "Send"}
            </button>
          </Row>
        </Section>

        <Section title="Approvals">
          {alerts.length === 0 ? (
            <Row title="Nothing waiting" desc="no approvals or alerts right now" />
          ) : (
            <>
              {approvals.map((a) => (
                <Row key={a.id} title={a.tool ?? "approval"} desc={alertDesc(a)}>
                  <span className="need-pct" style={{ width: "auto" }}>
                    {countdown(a.exp, nowSec)}
                  </span>
                  <div className="care-actions">
                    <button
                      className="primary"
                      disabled={acting !== null || !a.input_complete}
                      onClick={() =>
                        act(`approve:${a.id}`, { action: "approve", id: a.id })
                      }
                    >
                      Approve
                    </button>
                    <button
                      className="danger"
                      disabled={acting !== null}
                      onClick={() =>
                        act(`deny:${a.id}`, { action: "deny", id: a.id })
                      }
                    >
                      Deny
                    </button>
                  </div>
                </Row>
              ))}
              {others.map((a) =>
                a.kind === "question" ? (
                  <Row key={a.id} title="Waiting at the computer" desc={alertDesc(a)}>
                    <Dot state="warn" label="question" />
                  </Row>
                ) : (
                  <Row key={a.id} title="Turn ended with an error" desc={alertDesc(a)}>
                    <span style={{ color: "var(--red)" }}>
                      <Dot state="bad" label="error" />
                    </span>
                  </Row>
                ),
              )}
            </>
          )}
        </Section>

        <Section title="Sessions">
          {sessions.length === 0 ? (
            <Empty start="NO SESSIONS">
              No agent sessions yet — install the Claude Code pack: see
              docs/en/presence.md
            </Empty>
          ) : (
            <>
              {sessions.map((s) => (
                <Row
                  key={s.id}
                  title={`${s.src} · ${s.name ?? s.id}`}
                  desc={
                    [s.tool, s.det, s.model].filter(Boolean).join(" · ") ||
                    undefined
                  }
                >
                  <Dot state={SESSION_DOT[s.st]} label={s.st} />
                  <span className="need-pct" style={{ width: "auto" }}>
                    {ago(s.since, nowSec)}
                  </span>
                </Row>
              ))}
              {(presence.more ?? 0) > 0 && (
                <Row title={`+${presence.more} more sessions not shown`} />
              )}
            </>
          )}
        </Section>

        <Section title="Devices">
          {devices.length === 0 ? (
            <Row
              title="No paired devices"
              desc="pair one from the terminal with bastion presence pair <addr> <code>"
            />
          ) : (
            devices.map((d) => (
              <Row
                key={d.id}
                title={d.name ?? d.id}
                desc={[
                  d.addr,
                  d.caps?.w && d.caps?.h ? `${d.caps.w}×${d.caps.h}` : null,
                  d.caps?.touch ? "touch" : null,
                ]
                  .filter(Boolean)
                  .join(" · ")}
              />
            ))
          )}
        </Section>
      </>
    );
  }

  return (
    <>
      <div className="page-head">
        <h1>Buddy</h1>
        <span className="sub">
          your companion — cosmetic progress only, no capabilities unlocked
        </span>
        <span className="spacer" />
        <button
          onClick={() => {
            refresh();
            refreshPresence();
            refreshDevices();
          }}
        >
          refresh
        </button>
      </div>
      <div className="page-body">
        {!tokens.owner ? (
          <Empty start="NO TOKEN">
            the companion is for the operator — set the owner token under
            Connection first
          </Empty>
        ) : error ? (
          <div className="error-line">
            companion unavailable: {error}{" "}
            <button onClick={refresh} style={{ marginLeft: 8 }}>
              retry
            </button>
          </div>
        ) : snapshot === null ? (
          <div className="pcard skeleton" aria-hidden="true">
            <div className="phead">
              <span className="pname">loading…</span>
            </div>
            <div className="pmeta">reading companion state</div>
          </div>
        ) : !snapshot.game_enabled ? (
          <Empty start="GAME OFF">
            the companion game is off — turn it on from the terminal with{" "}
            <span className="cmd">/pet game on</span>. The visual companion
            still shows in the TUI either way; this is purely cosmetic.
          </Empty>
        ) : (
          <>
            <Section title={`${snapshot.pack_name} — level ${snapshot.level}`}>
              <div className="pet-frame-wrap">
                <pre className="pet-frame">
                  {snapshot.frame.rows.join("\n")}
                </pre>
              </div>
              <Row
                title={`${snapshot.xp} XP`}
                desc={`${snapshot.successful_turns} completed turns`}
              />
            </Section>

            <Section title="Needs">
              {(Object.keys(NEED_LABEL) as (keyof typeof NEED_LABEL)[]).map(
                (need) => {
                  const pct = snapshot.needs[need];
                  const due = snapshot.cues.includes(CUE_FOR_NEED[need]);
                  return (
                    <Row
                      key={need}
                      title={NEED_LABEL[need]}
                      desc={due ? "due now" : undefined}
                    >
                      <div className="need-track">
                        <div
                          className={`need-fill ${needTone(pct)}`}
                          style={{ width: `${pct}%` }}
                        />
                      </div>
                      <span className="need-pct">{pct}%</span>
                    </Row>
                  );
                },
              )}
            </Section>

            <Section title="Care">
              <Row
                title="Actions"
                desc="applies immediately through the daemon — optimistic refresh from its response"
              >
                <div className="care-actions">
                  {CARE_ACTIONS.map((c) => (
                    <button
                      key={c.action}
                      disabled={busy !== null}
                      onClick={() => care(c.action)}
                    >
                      {busy === c.action ? `${c.label}…` : c.label}
                    </button>
                  ))}
                </div>
              </Row>
            </Section>
          </>
        )}
        {tokens.owner && renderPresence()}
      </div>
    </>
  );
}
