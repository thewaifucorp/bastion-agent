import { FormEvent, useRef, useState } from "react";
import { ApiError, chat, tokens } from "../api";
import { Empty } from "../ui";

interface Msg {
  who: "me" | "bastion";
  text: string;
  pending?: boolean;
}

export default function Chat() {
  const [log, setLog] = useState<Msg[]>([]);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);

  async function send(e: FormEvent) {
    e.preventDefault();
    const t = text.trim();
    if (!t || busy) return;
    setText("");
    setBusy(true);
    setLog((l) => [
      ...l,
      { who: "me", text: t },
      { who: "bastion", text: "…", pending: true },
    ]);
    queueMicrotask(() =>
      scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight }),
    );
    let reply: string;
    try {
      reply = (await chat.turn(t)).reply;
    } catch (err) {
      reply =
        err instanceof ApiError && err.code === "token_missing"
          ? "owner token not set — configure it under Connection"
          : `turn failed: ${err instanceof Error ? err.message : err}`;
    }
    setLog((l) => [
      ...l.filter((m) => !m.pending),
      { who: "bastion", text: reply },
    ]);
    setBusy(false);
    queueMicrotask(() =>
      scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight }),
    );
  }

  return (
    <>
      <div className="page-head">
        <h1>Chat</h1>
        <span className="sub">
          the console's turn — slash commands (/task, /schedule, /help) work
        </span>
      </div>
      <div className="page-body flush">
        <div className="scrollwrap" ref={scrollRef} style={{ flex: 1, overflowY: "auto" }}>
          {log.length === 0 ? (
            <Empty start="PRESS START">
              {tokens.owner
                ? "talk to the daemon — a durable objective becomes a Pursue task"
                : "set the owner token under Connection first"}
            </Empty>
          ) : (
            <div className="chat-log">
              {log.map((m, i) => (
                <div
                  key={i}
                  className={`msg ${m.who} ${m.pending ? "pending" : ""}`}
                >
                  <span className="who">{m.who === "me" ? "you" : "bastion"}</span>
                  {m.text}
                </div>
              ))}
            </div>
          )}
        </div>
        <form className="chat-form" onSubmit={send}>
          <input
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder={busy ? "waiting for the turn…" : "message or /command"}
            disabled={busy}
            aria-label="message"
          />
          <button type="submit" className="primary" disabled={busy}>
            send
          </button>
        </form>
      </div>
    </>
  );
}
