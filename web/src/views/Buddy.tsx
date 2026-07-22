import { useCallback, useEffect, useState } from "react";
import { CareAction, CompanionSnapshot, companionApi, tokens } from "../api";
import { Empty, Row, Section, useToast } from "../ui";

// Buddy: the TUI's tamagotchi-style companion, mirrored on the web. The
// daemon is the single writer of companion.json while it's running (A5
// S5) — care actions here go straight through it, same as the TUI's own
// /pet commands. Purely cosmetic: no capabilities unlocked, ever.

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

export default function Buddy({ configTick }: { configTick: number }) {
  const [snapshot, setSnapshot] = useState<CompanionSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<CareAction | null>(null);
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

  return (
    <>
      <div className="page-head">
        <h1>Buddy</h1>
        <span className="sub">
          your companion — cosmetic progress only, no capabilities unlocked
        </span>
        <span className="spacer" />
        <button onClick={refresh}>refresh</button>
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
      </div>
    </>
  );
}
