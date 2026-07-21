import { useEffect, useMemo, useRef, useState } from "react";
import {
  BastionEvent,
  ConnState,
  streamEvents,
  tokens,
} from "./api";
import Vigilia from "./views/Vigilia";
import Tasks from "./views/Tasks";
import Chat from "./views/Chat";
import Config from "./views/Config";

const TABS = [
  { key: "vigilia", label: "1: vigília" },
  { key: "tarefas", label: "2: tarefas" },
  { key: "chat", label: "3: chat" },
  { key: "config", label: "4: config" },
] as const;
type TabKey = (typeof TABS)[number]["key"];

const LEDGER_MAX = 300;
export const LANTERN_GLOW_MS = 45_000;

export interface LedgerEntry {
  at: number;
  ev: BastionEvent;
}

export default function App() {
  const [tab, setTab] = useState<TabKey>(() =>
    tokens.owner || tokens.cp ? "vigilia" : "config",
  );
  const [conn, setConn] = useState<ConnState>("off");
  const [ledger, setLedger] = useState<LedgerEntry[]>([]);
  const [personas, setPersonas] = useState<Map<string, number>>(new Map());
  // Bump para forçar o resfriamento visual das lanternas com o tempo.
  const [, setClock] = useState(0);
  const [streamGen, setStreamGen] = useState(0);
  const stopRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    stopRef.current?.();
    stopRef.current = streamEvents((ev) => {
      setLedger((old) => [{ at: Date.now(), ev }, ...old].slice(0, LEDGER_MAX));
      if (
        (ev.event === "cabinet.started" || ev.event === "turn.completed") &&
        Array.isArray(ev.personas)
      ) {
        setPersonas((old) => {
          const next = new Map(old);
          for (const p of ev.personas!) next.set(p, Date.now());
          return next;
        });
      }
    }, setConn);
    return () => stopRef.current?.();
  }, [streamGen]);

  useEffect(() => {
    const t = setInterval(() => setClock((c) => c + 1), 5000);
    return () => clearInterval(t);
  }, []);

  // Atalhos de arcade: 1–4 trocam de aba (fora de campos de texto).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      if (target.closest("input, textarea")) return;
      const i = Number(e.key) - 1;
      if (i >= 0 && i < TABS.length) setTab(TABS[i].key);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const connLabel = useMemo(
    () =>
      ({
        off: "sem token",
        connecting: "reconectando…",
        live: "ao vivo",
        unauthorized: "token recusado",
      })[conn],
    [conn],
  );

  const lanterns = useMemo(
    () => [...personas.entries()].sort((a, b) => b[1] - a[1]),
    [personas],
  );

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          -BASTION- <small>guarda</small>
        </div>
        <nav className="tabs" role="tablist">
          {TABS.map((t) => (
            <button
              key={t.key}
              role="tab"
              className="tab"
              aria-selected={tab === t.key}
              onClick={() => setTab(t.key)}
            >
              {t.label}
            </button>
          ))}
        </nav>
        <span className="spacer" />
        <span className={`conn ${conn}`}>{connLabel}</span>
      </header>

      <div className="lanterns">
        {lanterns.length === 0 ? (
          <span className="hint">
            lanternas acendem quando uma persona fala
          </span>
        ) : (
          lanterns.map(([name, seen]) => (
            <span
              key={name}
              className={
                "lantern" +
                (Date.now() - seen < LANTERN_GLOW_MS ? " lit" : "")
              }
            >
              <span className="px" />
              {name}
            </span>
          ))
        )}
      </div>

      {tab === "vigilia" && <Vigilia ledger={ledger} />}
      {tab === "tarefas" && <Tasks />}
      {tab === "chat" && <Chat />}
      {tab === "config" && (
        <Config onSaved={() => setStreamGen((g) => g + 1)} />
      )}
    </div>
  );
}
