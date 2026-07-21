import { FormEvent, useRef, useState } from "react";
import { ApiError, chat, tokens } from "../api";

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
          ? "sem token de owner — configure em 4: config"
          : `turno falhou: ${err instanceof Error ? err.message : err}`;
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
    <main className="view">
      <section className="pane">
        <div className="pane-head">chat — mesmo turno do console</div>
        <div className="scroll" ref={scrollRef}>
          {log.length === 0 ? (
            <div className="empty">
              <span className="start">▶ PRESS START</span>
              <br />
              {tokens.owner
                ? "fala com o daemon — comandos /task, /schedule etc. funcionam"
                : "configure o token de owner em 4: config"}
            </div>
          ) : (
            <div className="chat-log">
              {log.map((m, i) => (
                <div
                  key={i}
                  className={`msg ${m.who} ${m.pending ? "pending" : ""}`}
                >
                  <span className="who">
                    {m.who === "me" ? "você" : "bastion"}
                  </span>
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
            placeholder={busy ? "aguardando o turno…" : "mensagem ou /comando"}
            disabled={busy}
            aria-label="mensagem"
          />
          <button type="submit" className="primary" disabled={busy}>
            enviar
          </button>
        </form>
      </section>
    </main>
  );
}
