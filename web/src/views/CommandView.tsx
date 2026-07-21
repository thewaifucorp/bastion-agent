import { FormEvent, useState } from "react";
import { Empty, Term, useCommand, useToast } from "../ui";
import { tokens } from "../api";

/** A System view backed by a Remote-scope slash command: renders the
 * command's text output as a terminal block, with optional action input
 * (e.g. `/model <name>`) and optional confirmed action (e.g. `/update
 * apply`). Same output the console prints — one source of truth. */
export default function CommandView({
  title,
  sub,
  listCmd,
  placeholder,
  buildCmd,
  actionLabel,
  confirmCmd,
}: {
  title: string;
  sub: string;
  listCmd: string;
  placeholder?: string;
  buildCmd?: (value: string) => string;
  actionLabel?: string;
  confirmCmd?: { cmd: string; label: string; confirm: string };
}) {
  const { out, cmd, busy, error, run } = useCommand(
    tokens.owner ? listCmd : undefined,
  );
  const [value, setValue] = useState("");
  const toast = useToast();

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!buildCmd) return;
    const v = value.trim();
    if (!v) return;
    setValue("");
    await run(buildCmd(v));
    toast(`${actionLabel ?? "action"} sent`);
  }

  return (
    <>
      <div className="page-head">
        <h1>{title}</h1>
        <span className="sub">{sub}</span>
        <span className="spacer" />
        <button onClick={() => run(listCmd)} disabled={busy}>
          refresh
        </button>
        {confirmCmd && (
          <button
            className="danger"
            disabled={busy}
            onClick={() => {
              if (window.confirm(confirmCmd.confirm)) run(confirmCmd.cmd);
            }}
          >
            {confirmCmd.label}
          </button>
        )}
      </div>
      <div className="page-body">
        {!tokens.owner ? (
          <Empty start="NO TOKEN">
            this view drives the daemon's {listCmd} command — set the owner
            token under Connection
          </Empty>
        ) : (
          <>
            {error && <div className="error-line">{error}</div>}
            {busy && !out && <div className="empty">running {listCmd}…</div>}
            {out !== null && <Term cmd={cmd ?? listCmd} out={out} />}
            {buildCmd && (
              <form
                onSubmit={submit}
                style={{ display: "flex", gap: 8, marginTop: 12, maxWidth: 640 }}
              >
                <input
                  value={value}
                  onChange={(e) => setValue(e.target.value)}
                  placeholder={placeholder}
                  aria-label={placeholder}
                />
                <button type="submit" className="primary" disabled={busy}>
                  {actionLabel}
                </button>
              </form>
            )}
          </>
        )}
      </div>
    </>
  );
}
