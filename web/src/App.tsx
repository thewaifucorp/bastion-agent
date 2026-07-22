import { useEffect, useMemo, useRef, useState } from "react";
import { BastionEvent, ConnState, streamEvents, tokens } from "./api";
import { ToastProvider } from "./ui";
import Overview from "./views/Overview";
import LiveFeed from "./views/LiveFeed";
import Loadout from "./views/Loadout";
import Chat from "./views/Chat";
import Tasks from "./views/Tasks";
import Schedules from "./views/Schedules";
import CommandView from "./views/CommandView";
import Connection from "./views/Connection";
import Personas from "./views/Personas";
import Providers from "./views/Providers";
import Models from "./views/Models";
import Buddy from "./views/Buddy";
import About from "./views/About";

export interface LedgerEntry {
  at: number;
  ev: BastionEvent;
}
export const LANTERN_GLOW_MS = 45_000;
const LEDGER_MAX = 400;

interface NavItem {
  key: string;
  label: string;
}
const NAV: { section: string; items: NavItem[] }[] = [
  {
    section: "watch",
    items: [
      { key: "overview", label: "Overview" },
      { key: "loadout", label: "Loadout" },
      { key: "feed", label: "Live feed" },
    ],
  },
  {
    section: "work",
    items: [
      { key: "chat", label: "Chat" },
      { key: "tasks", label: "Tasks" },
      { key: "schedules", label: "Schedules" },
      { key: "personas", label: "Personas" },
      { key: "buddy", label: "Buddy" },
    ],
  },
  {
    section: "system",
    items: [
      { key: "models", label: "Models" },
      { key: "backends", label: "Backends" },
      { key: "providers", label: "Providers" },
      { key: "logs", label: "Logs" },
      { key: "update", label: "Update" },
    ],
  },
  {
    section: "settings",
    items: [
      { key: "connection", label: "Connection" },
      { key: "about", label: "About" },
    ],
  },
];
const ALL_KEYS = NAV.flatMap((s) => s.items.map((i) => i.key));

function routeFromHash(): string {
  const h = window.location.hash.replace(/^#\/?/, "");
  if (h === "connect") return "providers"; // legacy deep link
  return ALL_KEYS.includes(h) ? h : tokens.owner || tokens.cp ? "overview" : "connection";
}

export default function App() {
  const [route, setRoute] = useState<string>(routeFromHash);
  const [conn, setConn] = useState<ConnState>("off");
  const [ledger, setLedger] = useState<LedgerEntry[]>([]);
  const [personas, setPersonas] = useState<Map<string, number>>(new Map());
  const [runningTasks, setRunningTasks] = useState<Set<string>>(new Set());
  // bumped on config.change_requested / config.applied so the Providers and
  // Models views re-fetch what the daemon now reports
  const [configTick, setConfigTick] = useState(0);
  // bumped on companion.updated (A5 S5) so the Buddy view re-fetches after
  // a care action, an XP award, or a hook session event
  const [companionTick, setCompanionTick] = useState(0);
  const [streamGen, setStreamGen] = useState(0);
  const stopRef = useRef<(() => void) | null>(null);

  // hash <-> route (deep-linkable pages, no router dependency)
  useEffect(() => {
    const onHash = () => setRoute(routeFromHash());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);
  const go = (key: string) => {
    window.location.hash = `/${key}`;
  };

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
      // config plumbing: proposal staged (`event`) or override applied
      // (`type` — the config store frames it that way) refreshes A4 views
      const kind = typeof ev.event === "string" ? ev.event : ev.type;
      if (kind === "config.change_requested" || kind === "config.applied") {
        setConfigTick((t) => t + 1);
      }
      if (kind === "companion.updated") {
        setCompanionTick((t) => t + 1);
      }
      // attention plumbing: sidebar badge follows task lifecycle events
      if (ev.task && typeof ev.task === "string") {
        setRunningTasks((old) => {
          const next = new Set(old);
          if (ev.event === "task.terminal") next.delete(ev.task as string);
          else next.add(ev.task as string);
          return next;
        });
      }
    }, setConn);
    return () => stopRef.current?.();
  }, [streamGen]);

  const connLabel = useMemo(
    () =>
      ({
        off: "no token",
        connecting: "reconnecting…",
        live: "live",
        unauthorized: "token rejected",
      })[conn],
    [conn],
  );
  const connDot =
    conn === "live" ? "ok" : conn === "connecting" ? "warn" : "bad";

  const badge = (key: string): { n: number; hot: boolean } | null => {
    if (key === "tasks" && runningTasks.size > 0)
      return { n: runningTasks.size, hot: true };
    return null;
  };

  return (
    <ToastProvider>
      <div className="app">
        <aside className="sidebar">
          <div className="brand">
            -BASTION-
            <small>
              <span className={`dot ${connDot}`} />
              {connLabel}
            </small>
          </div>
          <nav className="nav">
            {NAV.map((sec) => (
              <div className="nav-section" key={sec.section}>
                <div className="nav-title">{sec.section}</div>
                {sec.items.map((item) => {
                  const b = badge(item.key);
                  return (
                    <button
                      key={item.key}
                      className="nav-item"
                      aria-current={route === item.key ? "page" : undefined}
                      onClick={() => go(item.key)}
                    >
                      {item.label}
                      {b && (
                        <span className={"badge" + (b.hot ? " hot" : "")}>
                          {b.n}
                        </span>
                      )}
                    </button>
                  );
                })}
              </div>
            ))}
          </nav>
          <div className="sidebar-foot">
            the agent that can't betray you
          </div>
        </aside>

        <div className="main">
          {route === "overview" && <Overview go={go} />}
          {route === "loadout" && (
            <Loadout personasLive={personas} runningTasks={runningTasks} />
          )}
          {route === "feed" && (
            <LiveFeed ledger={ledger} personas={personas} />
          )}
          {route === "chat" && <Chat />}
          {route === "tasks" && <Tasks />}
          {route === "schedules" && <Schedules />}
          {route === "personas" && <Personas configTick={configTick} />}
          {route === "buddy" && <Buddy configTick={companionTick} />}
          {route === "providers" && <Providers configTick={configTick} />}
          {route === "models" && <Models configTick={configTick} />}
          {route === "backends" && (
            <CommandView
              title="Backends"
              sub="conversation backend — model loop or a subscription runtime"
              listCmd="/backend"
              placeholder="backend id"
              buildCmd={(v) => `/backend use ${v}`}
              actionLabel="use backend"
            />
          )}
          {route === "logs" && (
            <CommandView
              title="Logs"
              sub="recent daemon ERROR/WARN entries (timestamp/level/message)"
              listCmd="/logs"
            />
          )}
          {route === "update" && (
            <CommandView
              title="Update"
              sub="release status and explicit host update"
              listCmd="/update status"
              confirmCmd={{
                cmd: "/update apply",
                label: "apply update",
                confirm:
                  "Apply the host update now? The daemon restarts through the trusted updater.",
              }}
            />
          )}
          {route === "connection" && (
            <Connection onSaved={() => setStreamGen((g) => g + 1)} />
          )}
          {route === "about" && <About />}
        </div>
      </div>
    </ToastProvider>
  );
}
