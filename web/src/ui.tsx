// Shared primitives following one strict grammar (the "settings row"
// pattern): a section is a typographic heading + exactly one bordered card;
// a card is rows; a row is text on the left and exactly one control on the
// right. Status is always dot + plain text, never a colored pill.

import {
  createContext,
  ReactNode,
  useCallback,
  useContext,
  useEffect,
  useState,
} from "react";
import { command } from "./api";

export function Section({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <div className="section">
      <h2>{title}</h2>
      <div className="card">{children}</div>
    </div>
  );
}

export function Row({
  title,
  desc,
  children,
}: {
  title: string;
  desc?: string;
  children?: ReactNode;
}) {
  return (
    <div className="row">
      <div className="txt">
        <div className="t">{title}</div>
        {desc && <div className="d">{desc}</div>}
      </div>
      {children && <div className="ctl">{children}</div>}
    </div>
  );
}

export function Dot({
  state,
  label,
}: {
  state: "ok" | "warn" | "bad" | "info" | "off";
  label: string;
}) {
  return (
    <span>
      <span className={`dot ${state === "off" ? "" : state}`} />
      {label}
    </span>
  );
}

export function Empty({ start, children }: { start: string; children?: ReactNode }) {
  return (
    <div className="empty">
      <span className="start">▶ {start}</span>
      {children && (
        <>
          <br />
          {children}
        </>
      )}
    </div>
  );
}

/** Terminal-style output block for command-backed views. */
export function Term({ cmd, out }: { cmd: string; out: string }) {
  return (
    <div className="term">
      <span className="prompt">$ {cmd}</span>
      {"\n"}
      {out}
    </div>
  );
}

/** Runs a daemon slash command and keeps its text output + state. */
export function useCommand(initial?: string) {
  const [out, setOut] = useState<string | null>(null);
  const [cmd, setCmd] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = useCallback(async (text: string) => {
    setBusy(true);
    setCmd(text);
    setError(null);
    try {
      setOut(await command(text));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setOut(null);
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    if (initial) run(initial);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return { out, cmd, busy, error, run };
}

// ── toast ────────────────────────────────────────────────────────────────

type ToastFn = (text: string, warn?: boolean) => void;
const ToastCtx = createContext<ToastFn>(() => {});

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toast, setToast] = useState<{ text: string; warn: boolean } | null>(
    null,
  );
  const show = useCallback<ToastFn>((text, warn = false) => {
    setToast({ text, warn });
    setTimeout(() => setToast(null), 5000);
  }, []);
  return (
    <ToastCtx.Provider value={show}>
      {children}
      {toast && (
        <div className={"toast" + (toast.warn ? " warn" : "")} role="status">
          {toast.text}
        </div>
      )}
    </ToastCtx.Provider>
  );
}

export const useToast = () => useContext(ToastCtx);
