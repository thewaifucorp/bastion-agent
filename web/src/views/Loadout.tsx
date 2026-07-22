import { useEffect, useMemo, useState } from "react";
import { ApiError, request2, tokens } from "../api";
import { LANTERN_GLOW_MS } from "../App";
import { Empty } from "../ui";

// The Loadout view: your bastion, assembled piece by piece. Every piece is
// something the daemon REALLY reports (GET /loadout — captured at boot),
// drawn plugged into the core through its actual seam: personas through the
// responder port, tools through the capability registry, runtimes through
// Pursue delegation, channels through owner-routed I/O, MCP servers through
// the MCP client. Live events make the pieces glow — a persona speaking
// lights its plate; a running task pulses the runtime rack.

export interface LoadoutData {
  personas: string[];
  tools: string[];
  runtimes: { id: string }[];
  channels: { id: string; enabled: boolean }[];
  mcp_servers: string[];
  extensions: string[];
  captured_at: number;
}

interface Piece {
  id: string;
  label: string;
  state: "on" | "dim" | "off";
  glow?: boolean;
}

interface Zone {
  key: string;
  title: string;
  seam: string;
  side: "left" | "right";
  pieces: Piece[];
  emptyNote?: string;
}

const CORE_W = 190;
const CORE_H = 120;
const PIECE_W = 168;
const PIECE_H = 26;
const ZONE_GAP = 14;
const COL_X = 40;
const WIDTH = 980;

export default function Loadout({
  personasLive,
  runningTasks,
}: {
  personasLive: Map<string, number>;
  runningTasks: Set<string>;
}) {
  const [data, setData] = useState<LoadoutData | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showTools, setShowTools] = useState(false);

  useEffect(() => {
    if (!tokens.owner) return;
    request2<LoadoutData>(tokens.owner, "/loadout")
      .then((d) => setData(d))
      .catch((e) =>
        setError(
          e instanceof ApiError && e.status === 401
            ? "owner token rejected (401)"
            : `loadout unavailable: ${e instanceof Error ? e.message : e}`,
        ),
      );
  }, []);

  const zones: Zone[] = useMemo(() => {
    if (!data) return [];
    const now = Date.now();
    return [
      {
        key: "personas",
        title: "personas",
        seam: "responder port",
        side: "left",
        pieces: data.personas.map((p) => ({
          id: p,
          label: p,
          state: "on" as const,
          glow: (personasLive.get(p) ?? 0) > now - LANTERN_GLOW_MS,
        })),
        emptyNote: "none loaded from ./personas/",
      },
      {
        key: "channels",
        title: "channels",
        seam: "owner-routed I/O",
        side: "left",
        pieces: data.channels.map((c) => ({
          id: c.id,
          label: c.id,
          state: c.enabled ? ("on" as const) : ("off" as const),
        })),
      },
      {
        key: "extensions",
        title: "extension packs",
        seam: "sandboxed host",
        side: "left",
        pieces: data.extensions.map((e) => ({
          id: e,
          label: e,
          state: "on" as const,
        })),
        emptyNote: "no packs installed",
      },
      {
        key: "runtimes",
        title: "coding runtimes",
        seam: "Pursue delegation",
        side: "right",
        pieces: data.runtimes.map((r) => ({
          id: r.id,
          label: r.id,
          state: "on" as const,
          glow: runningTasks.size > 0,
        })),
        emptyNote: "none registered",
      },
      {
        key: "mcp",
        title: "mcp servers",
        seam: "MCP client",
        side: "right",
        pieces: data.mcp_servers.map((m) => ({
          id: m,
          label: m,
          state: "on" as const,
        })),
        emptyNote: "none configured",
      },
      {
        key: "tools",
        title: `tools (${data.tools.length})`,
        seam: "capability registry",
        side: "right",
        pieces: [],
      },
    ];
  }, [data, personasLive, runningTasks]);

  // vertical layout per side
  const layout = useMemo(() => {
    const place = (side: "left" | "right") => {
      let y = 30;
      return zones
        .filter((z) => z.side === side)
        .map((z) => {
          const rows = Math.max(z.pieces.length, 1);
          const h = 22 + rows * (PIECE_H + 6) + 8;
          const box = { zone: z, y, h };
          y += h + ZONE_GAP;
          return box;
        });
    };
    const left = place("left");
    const right = place("right");
    const height = Math.max(
      left.at(-1) ? left.at(-1)!.y + left.at(-1)!.h : 0,
      right.at(-1) ? right.at(-1)!.y + right.at(-1)!.h : 0,
      360,
    );
    return { left, right, height: height + 30 };
  }, [zones]);

  const coreX = WIDTH / 2 - CORE_W / 2;
  const coreY = layout.height / 2 - CORE_H / 2;

  if (!tokens.owner) {
    return (
      <>
        <Head />
        <div className="page-body">
          <Empty start="NO TOKEN">
            the loadout is for the operator — set the owner token under
            Connection
          </Empty>
        </div>
      </>
    );
  }

  return (
    <>
      <Head />
      <div className="page-body">
        {error && <div className="error-line">{error}</div>}
        {!data && !error && <div className="empty">assembling…</div>}
        {data && (
          <>
            <svg
              viewBox={`0 0 ${WIDTH} ${layout.height}`}
              style={{ width: "100%", maxWidth: 1100, display: "block" }}
              role="img"
              aria-label="your bastion, assembled piece by piece"
            >
              {/* seams: one line per zone into the core */}
              {[...layout.left, ...layout.right].map(({ zone, y, h }) => {
                const zx =
                  zone.side === "left" ? COL_X + PIECE_W + 12 : WIDTH - COL_X - PIECE_W - 12;
                const zy = y + h / 2;
                const cx = zone.side === "left" ? coreX : coreX + CORE_W;
                const cy = coreY + CORE_H / 2;
                const midx = (zx + cx) / 2;
                return (
                  <g key={zone.key}>
                    <path
                      d={`M ${zx} ${zy} C ${midx} ${zy}, ${midx} ${cy}, ${cx} ${cy}`}
                      fill="none"
                      stroke="var(--line)"
                      strokeWidth="1"
                    />
                    <text
                      x={midx}
                      y={(zy + cy) / 2 - 6}
                      textAnchor="middle"
                      fill="var(--dim)"
                      fontSize="9"
                      style={{ letterSpacing: "0.08em" }}
                    >
                      {zone.seam}
                    </text>
                  </g>
                );
              })}

              {/* core */}
              <rect
                x={coreX}
                y={coreY}
                width={CORE_W}
                height={CORE_H}
                fill="var(--panel-raise)"
                stroke="var(--line2)"
              />
              <text
                x={coreX + CORE_W / 2}
                y={coreY + 34}
                textAnchor="middle"
                fill="var(--green)"
                fontSize="14"
                fontWeight="700"
                style={{ letterSpacing: "0.2em" }}
              >
                -BASTION-
              </text>
              <text
                x={coreX + CORE_W / 2}
                y={coreY + 54}
                textAnchor="middle"
                fill="var(--dim)"
                fontSize="10"
              >
                daemon core
              </text>
              <text
                x={coreX + CORE_W / 2}
                y={coreY + 76}
                textAnchor="middle"
                fill="var(--steel)"
                fontSize="10"
              >
                memory · beliefs · budget
              </text>
              <text
                x={coreX + CORE_W / 2}
                y={coreY + 92}
                textAnchor="middle"
                fill="var(--steel)"
                fontSize="10"
              >
                authority explicit
              </text>

              {/* zones */}
              {[...layout.left, ...layout.right].map(({ zone, y, h }) => {
                const x = zone.side === "left" ? COL_X : WIDTH - COL_X - PIECE_W - 12;
                return (
                  <g key={zone.key}>
                    <rect
                      x={x - 6}
                      y={y}
                      width={PIECE_W + 24}
                      height={h}
                      fill="none"
                      stroke="var(--line)"
                      strokeDasharray="3 3"
                    />
                    <text
                      x={x + 2}
                      y={y + 14}
                      fill="var(--dim)"
                      fontSize="9"
                      style={{ letterSpacing: "0.22em", textTransform: "uppercase" }}
                    >
                      {zone.title}
                    </text>
                    {zone.pieces.length === 0 && (
                      <text x={x + 2} y={y + 36} fill="var(--dim)" fontSize="10">
                        {zone.key === "tools" ? `${data.tools.length} attached` : (zone.emptyNote ?? "—")}
                      </text>
                    )}
                    {zone.pieces.map((p, i) => {
                      const py = y + 22 + i * (PIECE_H + 6);
                      const stroke = p.glow
                        ? "var(--amber)"
                        : p.state === "on"
                          ? "var(--green)"
                          : "var(--line)";
                      const fill = p.state === "off" ? "var(--dim)" : "var(--ice)";
                      return (
                        <g key={p.id}>
                          <rect
                            x={x}
                            y={py}
                            width={PIECE_W}
                            height={PIECE_H}
                            fill="var(--black)"
                            stroke={stroke}
                          />
                          <circle
                            cx={x + 12}
                            cy={py + PIECE_H / 2}
                            r={3}
                            fill={
                              p.glow
                                ? "var(--amber)"
                                : p.state === "on"
                                  ? "var(--green)"
                                  : "var(--dim)"
                            }
                          />
                          <text
                            x={x + 24}
                            y={py + PIECE_H / 2 + 3.5}
                            fill={fill}
                            fontSize="11"
                          >
                            {p.label}
                            {p.state === "off" ? " (off)" : ""}
                          </text>
                        </g>
                      );
                    })}
                  </g>
                );
              })}
            </svg>

            <div style={{ marginTop: 10, maxWidth: 1100 }}>
              <button onClick={() => setShowTools((s) => !s)}>
                {showTools ? "hide" : "show"} the {data.tools.length} attached tools
              </button>
              {showTools && (
                <div className="term" style={{ marginTop: 8 }}>
                  {data.tools.join("\n")}
                </div>
              )}
              <div className="error-line" style={{ color: "var(--dim)" }}>
                snapshot taken at daemon boot (
                {new Date(data.captured_at / 1e6).toLocaleString()}) — restart
                to re-capture
              </div>
            </div>
          </>
        )}
      </div>
    </>
  );
}

function Head() {
  return (
    <div className="page-head">
      <h1>Loadout</h1>
      <span className="sub">
        your bastion, piece by piece — every part and the seam it plugs into
      </span>
    </div>
  );
}
