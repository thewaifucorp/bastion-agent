import { FormEvent, useState } from "react";
import { tokens } from "../api";

export default function Config({ onSaved }: { onSaved: () => void }) {
  const [owner, setOwner] = useState(tokens.owner);
  const [cp, setCp] = useState(tokens.cp);
  const [saved, setSaved] = useState(false);

  function save(e: FormEvent) {
    e.preventDefault();
    tokens.owner = owner.trim();
    tokens.cp = cp.trim();
    setSaved(true);
    setTimeout(() => setSaved(false), 2500);
    onSaved();
  }

  return (
    <main className="view">
      <section className="pane">
        <div className="pane-head">config — tokens</div>
        <div className="scroll">
          <form className="cfg" onSubmit={save}>
            <p className="note">
              Dois tokens, duas superfícies. O token de <b>owner</b> autentica
              o feed ao vivo e o chat (<code>/events</code>,{" "}
              <code>/webhook</code>). A credencial do Control Plane autentica
              as tarefas (<code>/v1/*</code>) — emita uma no console do daemon
              com{" "}
              <code>/credential issue dashboard tasks:read,tasks:control</code>
              . Ambos ficam só neste navegador (localStorage) e só viajam para
              o próprio daemon.
            </p>
            <div className="row">
              <label htmlFor="tok-owner">token de owner</label>
              <input
                id="tok-owner"
                type="password"
                autoComplete="off"
                value={owner}
                onChange={(e) => setOwner(e.target.value)}
              />
            </div>
            <div className="row">
              <label htmlFor="tok-cp">credencial bcp_</label>
              <input
                id="tok-cp"
                type="password"
                autoComplete="off"
                value={cp}
                onChange={(e) => setCp(e.target.value)}
              />
            </div>
            <button type="submit" className="primary">
              {saved ? "salvo ✓" : "salvar e conectar"}
            </button>
          </form>
        </div>
      </section>
    </main>
  );
}
