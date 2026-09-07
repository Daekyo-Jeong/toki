import type { CSSProperties, ReactNode } from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { DotSprite } from "./sprites";

/* ─── Cassette Futurism case colors (V4-7) — shared by CassetteShell
   (TamagotchiShell.tsx) and CassettePanel (below) so settings/coaching
   panels pick up the same case_color/invert_screen choices as the pet
   shell. CRT/phosphor colors are unaffected by case color, only by invert. */
export const CASES: Record<string, { c: string; hi: string; lo: string; line: string; ink: string }> = {
  beige: { c: "#cabb9e", hi: "#e0d3b8", lo: "#a3937a", line: "#857761", ink: "#6b5f4c" },
  platinum: { c: "#c3c4bd", hi: "#dcdcd6", lo: "#9b9c95", line: "#7e7f78", ink: "#5c5d57" },
  ecru: { c: "#d8caa6", hi: "#ece0c2", lo: "#b0a180", line: "#8f8163", ink: "#6e6248" },
  slate: { c: "#8f9aa0", hi: "#aab4b9", lo: "#6f797f", line: "#565f64", ink: "#39424a" },
};
export const INV_VARS: Record<string, string> = {
  "--scr": "#e9ead0", "--scr2": "#f4f5de",
  "--phos": "#171720", "--phos-dim": "rgba(23,23,32,0.55)",
  "--phos-30": "rgba(23,23,32,0.32)", "--phos-14": "rgba(23,23,32,0.12)",
};
export function casingVars(casing: string, invert: boolean): CSSProperties {
  const k = CASES[casing] || CASES.beige;
  return {
    "--case": k.c, "--case-hi": k.hi, "--case-lo": k.lo, "--case-line": k.line, "--case-ink": k.ink,
    ...(invert ? INV_VARS : {}),
  } as CSSProperties;
}

/** Large-format cassette chrome for full-panel views (settings, coaching) —
 *  the same beige-case + dark-CRT-phosphor material as the pet shell, scaled
 *  up to fill the window instead of floating as a small device. Fills 100vh;
 *  `children` scrolls inside the CRT area, `footer` pins soft-buttons below. */
export function CassettePanel({
  title, onBack, casing = "beige", invert = false, children, footer,
}: {
  title: string;
  onBack: () => void;
  casing?: string;
  invert?: boolean;
  children: ReactNode;
  footer?: ReactNode;
}) {
  return (
    <div style={{ ...casingVars(casing, invert), width: "100%", height: "100vh", display: "flex", flexDirection: "column",
      background: "linear-gradient(158deg, var(--case-hi) 0%, var(--case) 30%, var(--case) 70%, var(--case-lo) 100%)" }}>
      {/* brand bar — also the drag handle, since these panels aren't the
          always-on desk and may need repositioning. */}
      <div
        data-tauri-drag-region
        onMouseDown={(e) => {
          const t = e.target as HTMLElement;
          if (t.closest("button")) return;
          if (e.button !== 0) return;
          e.preventDefault();
          getCurrentWindow().startDragging().catch(() => {});
        }}
        style={{ display: "flex", alignItems: "center", gap: 8, height: 34, padding: "0 14px", flexShrink: 0, cursor: "grab" }}
      >
        <span style={{ fontFamily: "var(--pixel)", fontSize: 13, color: "var(--case-ink)", letterSpacing: "0.04em" }} data-tauri-drag-region>toki</span>
        <span style={{ display: "flex", height: 5, gap: 0, boxShadow: "0 0 0 1px rgba(0,0,0,.12)" }}>
          <span style={{ width: 9, height: 5, background: "var(--rust)" }} />
          <span style={{ width: 9, height: 5, background: "var(--amber)" }} />
          <span style={{ width: 9, height: 5, background: "var(--teal)" }} />
        </span>
        <span style={{ flex: 1 }} data-tauri-drag-region />
        <span style={{ fontFamily: "var(--pixel)", fontSize: 11, color: "var(--case-ink)", letterSpacing: "0.08em" }} data-tauri-drag-region>{title}</span>
      </div>

      {/* recessed CRT bezel, filling the rest of the panel */}
      <div style={{ flex: 1, minHeight: 0, margin: "0 12px 12px", borderRadius: 16, padding: 10,
        background: "linear-gradient(160deg, var(--case-lo), var(--case))",
        boxShadow: "inset 0 3px 7px rgba(0,0,0,.4), inset 0 -1px 0 var(--case-hi), 0 1px 0 rgba(255,255,255,.2)",
        display: "flex", flexDirection: "column" }}>
        <div style={{ position: "relative", flex: 1, minHeight: 0, borderRadius: 10, overflow: "hidden",
          background: "radial-gradient(120% 130% at 50% 15%, var(--scr2) 0%, var(--scr) 78%, #0e0e15 100%)",
          boxShadow: "inset 0 0 34px rgba(0,0,0,.7), inset 0 0 0 1px rgba(0,0,0,.5)",
          display: "flex", flexDirection: "column" }}>
          <div style={{ position: "relative", zIndex: 2, flex: 1, minHeight: 0, overflowY: "auto",
            padding: "16px 18px", color: "var(--phos)", fontFamily: "var(--pixel)" }}>
            {children}
          </div>
          <div style={{ position: "relative", zIndex: 2, display: "flex", gap: 8,
            padding: "10px 18px", borderTop: "1.5px solid var(--phos-30)", flexShrink: 0 }}>
            <button className="cf-soft" onClick={onBack}>← 뒤로</button>
            {footer}
          </div>
          {/* scanlines + phosphor bloom, matching CrtScreen */}
          <div style={{ position: "absolute", inset: 0, zIndex: 3, pointerEvents: "none",
            backgroundImage: "repeating-linear-gradient(0deg, transparent 0 2px, rgba(0,0,0,.22) 2px 3px)" }} />
          <div style={{ position: "absolute", inset: 0, zIndex: 3, pointerEvents: "none",
            background: "radial-gradient(90% 80% at 50% 30%, rgba(233,234,208,0.06), transparent 70%)" }} />
        </div>
      </div>
    </div>
  );
}

/* ─── MVP-S — procedural parallax BG ─────────────────────
   Toki walks forward by having the world scroll past. Instead of repeating
   a single hand-drawn tile (and seeing the same hills loop), we generate
   chunks deterministically from a seed and roll them through a fixed-size
   buffer. Each chunk's left/right edges have y-values clamped to constants,
   so neighbouring chunks always meet seamlessly at the seam regardless of
   their interior shape.                                                  */

/** Tiny seeded PRNG (mulberry32). Pure, deterministic, ~10 LOC. */
function mulberry32(seed: number): () => number {
  let s = seed | 0;
  return () => {
    s = (s + 0x6d2b79f5) | 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const CHUNKS_VISIBLE = 4;       // total chunks in buffer (each = 1 frame wide)
const STEPS_PER_CHUNK = 24;     // stepped scroll resolution (8-bit feel)
const CHUNK_MS = 8000;          // time to traverse one chunk

type ParallaxColors = { inkA: string; inkB: string };

/** A single 256-unit-wide chunk, deterministically derived from `seed`.
 *  Endpoints (x=0 and x=256) of both hill polylines are fixed to constants
 *  (`FG_EDGE`, `BG_EDGE`) so any two chunks stitch seamlessly side by side. */
function Chunk({ seed, inkA, inkB }: { seed: number } & ParallaxColors) {
  const FG_EDGE = 96;
  const BG_EDGE = 80;
  const { fgPoints, bgPoints, houses, stars } = useMemo(() => {
    const rand = mulberry32(seed + 1);
    // Foreground hills — 11 interior points between x=24..240 with varying y.
    const fg: string[] = [`0,${FG_EDGE}`];
    for (let x = 24; x <= 240; x += 24) {
      const y = 88 + Math.floor(rand() * 10); // 88..97
      fg.push(`${x},${y}`);
    }
    fg.push(`256,${FG_EDGE}`);
    // Background hills — wider strides, higher up.
    const bg: string[] = [`0,${BG_EDGE}`];
    for (let x = 32; x <= 224; x += 32) {
      const y = 70 + Math.floor(rand() * 10); // 70..79
      bg.push(`${x},${y}`);
    }
    bg.push(`256,${BG_EDGE}`);
    // 0–2 houses placed inside the safe band [50, 200] so we never clip
    // at the seam regardless of which neighbour comes next.
    const houseCount = Math.floor(rand() * 3); // 0/1/2
    const houses: Array<{ x: number; h: number }> = [];
    for (let i = 0; i < houseCount; i++) {
      const x = 50 + Math.floor(rand() * 150);
      const h = 4 + Math.floor(rand() * 7); // 4..10
      houses.push({ x, h });
    }
    // 6–12 stars in the band [8, 248] × y∈[114,122].
    const starCount = 6 + Math.floor(rand() * 7);
    const stars: Array<{ x: number; y: number }> = [];
    for (let i = 0; i < starCount; i++) {
      stars.push({
        x: 8 + Math.floor(rand() * 240),
        y: 114 + Math.floor(rand() * 9),
      });
    }
    return { fgPoints: fg.join(" "), bgPoints: bg.join(" "), houses, stars };
  }, [seed]);

  return (
    <svg
      className="pixel"
      width={`${100 / CHUNKS_VISIBLE}%`}
      height="100%"
      viewBox="0 0 256 128"
      preserveAspectRatio="none"
      style={{ display: "block", flexShrink: 0 }}
    >
      <line x1="0" y1="104" x2="256" y2="104" stroke={inkA} strokeWidth="1" />
      <polyline points={fgPoints} fill="none" stroke={inkA} strokeWidth="1" />
      <polyline points={bgPoints} fill="none" stroke={inkB} strokeWidth="1" />
      <g fill={inkB}>
        {houses.map((h, i) => (
          <g key={i}>
            <rect x={h.x} y={104 - h.h} width="2" height={h.h} />
            <rect x={h.x - 2} y={104 - h.h - 2} width="6" height="2" />
          </g>
        ))}
      </g>
      {stars.map((s, i) => (
        <rect key={i} x={s.x} y={s.y} width="1" height="1" fill={inkB} />
      ))}
    </svg>
  );
}

/** Drives the rolling chunk buffer. Returns the current seed array and
 *  a 0..1 progress within the leftmost chunk. Pauses when `walking=false`. */
function useRollingChunks(walking: boolean) {
  const seedCursor = useRef<number>(4);
  const [seeds, setSeeds] = useState<number[]>(() => [0, 1, 2, 3]);
  const [steps, setSteps] = useState(0); // 0..STEPS_PER_CHUNK
  useEffect(() => {
    if (!walking) return;
    const id = window.setInterval(() => {
      setSteps((s) => {
        const next = s + 1;
        if (next >= STEPS_PER_CHUNK) {
          setSeeds((arr) => [...arr.slice(1), seedCursor.current++]);
          return 0;
        }
        return next;
      });
    }, CHUNK_MS / STEPS_PER_CHUNK);
    return () => window.clearInterval(id);
  }, [walking]);
  const progress = steps / STEPS_PER_CHUNK;
  return { seeds, progress };
}

function ParallaxBG({ walking, inkA, inkB }: { walking: boolean } & ParallaxColors) {
  const { seeds, progress } = useRollingChunks(walking);
  // Container is N chunks wide; each chunk = 1 frame width. We translate by
  // 1 chunk width per cycle = (100 / N)% of container width.
  const translate = (progress * 100) / CHUNKS_VISIBLE;
  return (
    <div
      style={{
        position: "absolute",
        inset: 0,
        display: "flex",
        width: `${CHUNKS_VISIBLE * 100}%`,
        transform: `translateX(-${translate}%)`,
        willChange: "transform",
      }}
    >
      {seeds.map((s) => (
        <Chunk key={s} seed={s} inkA={inkA} inkB={inkB} />
      ))}
    </div>
  );
}

/* ─── Window shell ────────────────────────────────────── */
export function Frame({ children }: { children: ReactNode }) {
  return (
    <div
      style={{
        width: "100%",
        height: "100vh",
        background: "var(--bg)",
        color: "var(--fg)",
        fontFamily: "var(--term)",
        border: "2px solid var(--line)",
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
      }}
    >
      {children}
    </div>
  );
}

/* ─── Title bar — TOKI vN + 5 nav glyphs (▤ ◇ ▲ ◈ ≡) ──── */
/** Explicit drag handle — shown only in pin mode, sits above HeaderBar.
 *  Visible "grab here" affordance with a hash pattern. Mousedown initiates
 *  the system drag. Without this, users (rightly) can't find the 26px
 *  header's empty regions and conclude "it doesn't move." */
export function DragGrip() {
  return (
    <div
      data-tauri-drag-region
      onMouseDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        getCurrentWindow().startDragging().catch(() => {});
      }}
      style={{
        height: 10,
        flexShrink: 0,
        background: "var(--fg)",
        cursor: "grab",
        userSelect: "none",
        // Diagonal stripes — pure visual affordance, no text noise.
        backgroundImage:
          "repeating-linear-gradient(135deg, var(--fg) 0 4px, rgba(0,255,153,0.35) 4px 5px)",
      }}
      title="Drag to move window"
    />

  );
}

export function HeaderBar({
  active,
  version = "v0.2",
  onNav,
  right,
  pinned,
  onTogglePin,
}: {
  active?: "stat" | "log" | "map" | "inv" | "cfg" | null;
  version?: string;
  onNav?: (key: "stat" | "log" | "map" | "inv" | "cfg") => void;
  right?: ReactNode;
  /** When true, render the 📌 button in pressed state. */
  pinned?: boolean;
  /** Click handler for the pin toggle. Omit to hide the button. */
  onTogglePin?: () => void;
}) {
  const tabs: { k: "stat" | "log" | "map" | "inv" | "cfg"; g: string; t: string }[] = [
    { k: "stat", g: "▤", t: "Stats" },
    { k: "log", g: "◇", t: "Dungeon Log" },
    { k: "map", g: "▲", t: "Dungeon" },
    { k: "inv", g: "◈", t: "Inventory" },
    { k: "cfg", g: "≡", t: "Settings" },
  ];
  return (
    <>
      {/* When the popover is pinned, every view shows the drag grip at
          the very top — not just the main screen. Users navigating into
          Stats/Log/etc. shouldn't lose the ability to move the window. */}
      {pinned && <DragGrip />}
    <div
      // The header doubles as a move handle. We belt-and-suspender it:
      //  - `data-tauri-drag-region` (auto-handled by Tauri)
      //  - explicit `startDragging()` on mousedown over non-button area
      //    (the attribute alone wasn't initiating moves reliably on macOS
      //    when alwaysOnTop+decorations:false combined.)
      data-tauri-drag-region
      onMouseDown={(e) => {
        // Skip when the press lands on an interactive control inside.
        const t = e.target as HTMLElement;
        if (t.closest("button, input, select, textarea, a")) return;
        if (e.button !== 0) return;
        e.preventDefault();
        getCurrentWindow()
          .startDragging()
          .catch(() => {});
      }}
      style={{
        display: "flex",
        alignItems: "stretch",
        borderBottom: "2px solid var(--line)",
        background: "var(--bg)",
        height: 26,
        flexShrink: 0,
        cursor: onTogglePin ? "grab" : "default",
        userSelect: "none",
      }}
    >
      <div
        className="px-8"
        data-tauri-drag-region
        style={{
          padding: "0 8px",
          display: "flex",
          alignItems: "center",
          gap: 8,
          borderRight: "1px solid var(--line)",
        }}
      >
        <span data-tauri-drag-region>TOKI</span>
        <span className="dim tm-14" data-tauri-drag-region>{version}</span>
      </div>
      <div style={{ flex: 1 }} data-tauri-drag-region />
      {right}
      {onTogglePin && (
        <button
          onClick={onTogglePin}
          title={pinned ? "Unpin (auto-anchor to tray)" : "Pin window to current position"}
          style={{
            all: "unset",
            cursor: "pointer",
            width: 26,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            borderLeft: "1px solid var(--line)",
            background: pinned ? "var(--accent)" : "var(--bg)",
            color: pinned ? "var(--fg)" : "var(--muted)",
            fontFamily: "var(--mono)",
            fontSize: 13,
            fontWeight: 700,
          }}
        >
          ◉
        </button>
      )}
      {tabs.map((b) => (
        <button
          key={b.k}
          title={b.t}
          onClick={() => onNav?.(b.k)}
          style={{
            all: "unset",
            cursor: "pointer",
            width: 26,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            borderLeft: "1px solid var(--line)",
            background: active === b.k ? "var(--fg)" : "var(--bg)",
            color: active === b.k ? "var(--bg)" : "var(--fg)",
            fontFamily: "var(--term)",
            fontSize: 16,
            fontWeight: 700,
          }}
        >
          {b.g}
        </button>
      ))}
    </div>
    </>
  );
}

/* ─── Single icon button (back-compat for code that already uses HBtn) */
export function HBtn({
  glyph,
  onClick,
  active = false,
  title,
}: {
  glyph: string;
  onClick?: () => void;
  active?: boolean;
  title?: string;
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      style={{
        all: "unset",
        cursor: "pointer",
        width: 26,
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        borderLeft: "1px solid var(--line)",
        background: active ? "var(--fg)" : "var(--bg)",
        color: active ? "var(--bg)" : "var(--fg)",
        fontFamily: "var(--term)",
        fontSize: 16,
        fontWeight: 700,
      }}
    >
      {glyph}
    </button>
  );
}

/* ─── Sub-view header (used by Settings/DungeonLog/Stats) ──── */
export function SubHeader({
  title,
  code = "01",
  back = "MAIN",
  onBack,
}: {
  title: string;
  code?: string;
  back?: string;
  onBack?: () => void;
}) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: "6px 10px",
        borderBottom: "2px solid var(--line)",
        background: "var(--bg)",
        flexShrink: 0,
      }}
    >
      <span className="px-8">{title}</span>
      <span style={{ flex: 1 }} />
      <span className="px-7 dim">§{code}</span>
      <button
        onClick={onBack}
        className="px-7"
        style={{
          all: "unset",
          cursor: onBack ? "pointer" : "default",
          padding: "3px 5px",
          background: "var(--fg)",
          color: "var(--bg)",
        }}
      >
        ‹ {back}
      </button>
    </div>
  );
}

/* ─── Stat bar — HP + Lv chip + class + gold ─────────── */
export function StatBar({
  lv,
  klass,
  gold,
}: {
  lv: number;
  klass: string;
  gold: string;
}) {
  // HP was removed — the 5h block usage it derived from slides silently
  // and the on/off cycle never aligned with what the user was doing. The
  // BlockBar at the bottom already shows the raw usage signal honestly.
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 10,
        padding: "6px 10px",
        borderBottom: "1px solid var(--line)",
        background: "var(--bg)",
        flexShrink: 0,
      }}
    >
      <div className="px-10" style={{ display: "flex", alignItems: "center", gap: 6 }}>
        <span>Lv.{lv}</span>
        <span style={{ background: "var(--fg)", color: "var(--bg)", padding: "3px 5px" }}>
          {klass.toUpperCase()}
        </span>
      </div>
      <span style={{ flex: 1 }} />
      <div className="px-10 tnum" style={{ whiteSpace: "nowrap" }}>¤{gold}</div>
    </div>
  );
}

/* ─── HP bar primitive (back-compat) ─────────────────── */
export function HpBar({
  hp,
  hp_max,
  crit = false,
}: {
  hp: number;
  hp_max: number;
  crit?: boolean;
}) {
  const pct = hp_max > 0 ? Math.max(0, Math.min(100, (hp / hp_max) * 100)) : 0;
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
      <span className="px-7">HP</span>
      <div className={"hpbar" + (crit ? " crit" : "")} style={{ flex: 1 }}>
        <div className="fill" style={{ width: `${pct}%` }} />
      </div>
      <span className="px-7 tnum">{hp}/{hp_max}</span>
    </div>
  );
}

/* ─── Stage view — 256×128 parallax canvas ─────────────
   Stretches to full frame width. Player anchored `playerX` from left.
   Mob anchored from left too (relative to player), so they stay adjacent
   regardless of how wide the frame renders. Sprites pixel-true at scale 2. */
export function Stage({
  sprite,
  scale = 2,
  mob,
  mobLeft = 88,
  playerX = 48,
  centerPlayer = false,
  tone = "normal",
  bezelH = 148,
  badge,
  rightBadge,
  overlay,
  noParallax = false,
  motionClass,
  transform,
  mobMotion,
  playerHit = false,
  walking = false,
}: {
  sprite: string;
  scale?: number;
  mob?: string | null;
  mobLeft?: number;
  playerX?: number;
  centerPlayer?: boolean;
  tone?: "normal" | "accent";
  bezelH?: number;
  badge?: ReactNode;
  rightBadge?: ReactNode;
  overlay?: ReactNode;
  noParallax?: boolean;
  motionClass?: string;
  transform?: string;
  mobMotion?: string;
  playerHit?: boolean;
  /** When true, parallax BG scrolls leftward to suggest forward motion.
   *  Pause this when a mob is on screen (combat stance). */
  walking?: boolean;
}) {
  const isAcc = tone === "accent";
  const stageBg = isAcc ? "var(--fg)" : "var(--bg)";
  const inkA = isAcc ? "var(--bg)" : "var(--line)";
  const inkB = isAcc ? "var(--accent)" : "var(--muted)";
  const palette: Record<string, string> = isAcc
    ? { "#": "var(--bg)", "@": "var(--accent)" }
    : { "#": "var(--fg)", "@": "var(--accent)" };

  return (
    <div
      className={isAcc ? undefined : "lcd-line"}
      style={{
        position: "relative",
        width: "100%",
        height: bezelH,
        background: stageBg,
        overflow: "hidden",
        flexShrink: 0,
      }}
    >
      {!noParallax && <ParallaxBG walking={walking} inkA={inkA} inkB={inkB} />}

      {sprite && (
        <div
          className={[transform ? undefined : motionClass, playerHit ? "player-hit" : undefined]
            .filter(Boolean)
            .join(" ")}
          style={{
            position: "absolute",
            bottom: 8,
            willChange: "transform",
            ...(centerPlayer
              ? { left: "50%", transform: transform ? `translateX(-50%) ${transform}` : "translateX(-50%)" }
              : { left: playerX, transform }),
          }}
        >
          <DotSprite sheet={sprite} scale={scale} palette={palette} />
        </div>
      )}

      {mob && (
        <div
          className={mobMotion}
          style={{
            position: "absolute",
            left: mobLeft,
            bottom: 8,
            willChange: "left, opacity, transform",
          }}
        >
          <DotSprite sheet={mob} scale={scale} palette={palette} />
        </div>
      )}

      {badge && (
        <div
          className="px-7"
          style={{
            position: "absolute",
            top: 6,
            left: 6,
            padding: "2px 4px",
            background: "var(--bg)",
            border: "1px solid var(--line)",
          }}
        >
          {badge}
        </div>
      )}
      {rightBadge && (
        <div
          className="px-7"
          style={{
            position: "absolute",
            top: 6,
            right: 6,
            padding: "2px 4px",
            background: "var(--bg)",
            border: "1px solid var(--line)",
          }}
        >
          {rightBadge}
        </div>
      )}
      {overlay}
    </div>
  );
}

/* ─── Back-compat alias for App.tsx which still imports DotCanvas ── */
export function DotCanvas({
  sprite,
  scale = 2,
  height = 148,
  motionClass,
  transform,
  overlay,
}: {
  sprite: string;
  scale?: number;
  height?: number;
  motionClass?: string;
  transform?: string;
  overlay?: ReactNode;
}) {
  return (
    <Stage
      sprite={sprite}
      scale={scale}
      bezelH={height}
      motionClass={motionClass}
      transform={transform}
      overlay={overlay}
    />
  );
}

/* ─── Speech bubble — TOKI tag + quote ────────────────── */
export function CliQuote({
  children,
  tone = "normal",
}: {
  children: ReactNode;
  tone?: "normal" | "alert";
}) {
  const bg = tone === "alert" ? "var(--accent)" : "var(--bg)";
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "34px 1fr",
        alignItems: "stretch",
        borderBottom: "1px solid var(--line)",
        background: bg,
        minHeight: 38,
        flexShrink: 0,
      }}
    >
      <div
        className="px-7"
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: "var(--fg)",
          color: "var(--bg)",
          borderRight: "1px solid var(--line)",
        }}
      >
        TOKI
      </div>
      <div
        className="tm-18"
        style={{
          padding: "6px 10px",
          display: "flex",
          alignItems: "center",
          gap: 6,
          overflow: "hidden",
        }}
      >
        <span className="px-8">{">"}</span>
        <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {children}
        </span>
      </div>
    </div>
  );
}

/* ─── Section rule ────────────────────────────────────── */
export function RuleHead({
  code,
  label,
  right,
}: {
  code: string;
  label: string;
  right?: string;
}) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "baseline",
        borderBottom: "1px dashed var(--line)",
        padding: "3px 10px",
        gap: 8,
        whiteSpace: "nowrap",
        background: "var(--bg)",
        flexShrink: 0,
      }}
    >
      <span className="px-7">§{code}</span>
      <span className="px-7 dim">{label}</span>
      <span style={{ flex: 1 }} />
      {right && <span className="px-7 dim">{right}</span>}
    </div>
  );
}

/* ─── Status strip (kept for back-compat) ────────────── */
export function StatusStrip({
  status,
  stage,
  day,
  time,
  live = true,
  alert = false,
}: {
  status: string;
  stage: string;
  day: number;
  time: string;
  live?: boolean;
  alert?: boolean;
}) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 6,
        padding: "4px 10px",
        borderTop: "1px solid var(--line)",
        borderBottom: "1px solid var(--line)",
        background: alert ? "var(--accent)" : "var(--bg)",
      }}
    >
      <span
        className="live-dot"
        style={{
          background: alert ? "var(--fg)" : live ? "var(--accent)" : "transparent",
        }}
      />
      <span className="px-7">{status}</span>
      <span className="px-7 dim">·</span>
      <span className="px-7">{stage}</span>
      <span className="px-7 dim">·</span>
      <span className="px-7 tnum">D{String(day).padStart(2, "0")}</span>
      <span style={{ flex: 1 }} />
      <span className="px-7 tnum">{time}</span>
    </div>
  );
}

/* ─── Lv progress with class chip ─────────────────────── */
export function LvProgress({
  lv,
  pct,
  className,
  nextLabel,
}: {
  lv: number;
  pct: number;
  className: string;
  nextLabel: string;
}) {
  return (
    <div
      style={{
        padding: "6px 10px",
        borderBottom: "1px solid var(--line)",
        background: "var(--bg)",
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 6, marginBottom: 4 }}>
        <span className="px-8">Lv.{lv}</span>
        <span
          className="px-7"
          style={{ background: "var(--fg)", color: "var(--bg)", padding: "2px 4px" }}
        >
          {className.toUpperCase()}
        </span>
        <span style={{ flex: 1 }} />
        <span className="px-7 dim tnum">{nextLabel}</span>
      </div>
      <div className="xpbar">
        <div className="fill" style={{ width: `${Math.max(0, Math.min(100, pct))}%` }} />
      </div>
    </div>
  );
}

/* ─── 5-stat tile grid ────────────────────────────────── */
export function StatGrid({
  stats,
}: {
  stats: { int: number; str: number; agi: number; wis: number; luk: number };
}) {
  const items: [string, number][] = [
    ["INT", stats.int],
    ["STR", stats.str],
    ["AGI", stats.agi],
    ["WIS", stats.wis],
    ["LUK", stats.luk],
  ];
  return (
    <div
      style={{
        padding: "6px 10px",
        borderBottom: "1px solid var(--line)",
        background: "var(--bg)",
      }}
    >
      <div style={{ display: "grid", gridTemplateColumns: "repeat(5, 1fr)", gap: 4 }}>
        {items.map(([k, v]) => (
          <div key={k} className="stat">
            <span className="k">{k}</span>
            <span className="v tnum">{v}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

/* ─── 5h Block bar — ccusage live + projection ────────── */
export function BlockBar({
  tokens,
  limit,
  burnRateTpm,
  projection,
  source,
}: {
  tokens: string;
  limit: string | null;
  burnRateTpm: number | null;
  projection: string | null;
  source: "ccusage" | "npx_ccusage" | "sqlite_fallback" | null;
}) {
  // We only surface the data source when it's the SQLite fallback —
  // that's the only state worth nagging about (ccusage missing or
  // failing). In the normal ccusage/npx_ccusage cases the badge is just
  // chrome that doesn't help the user.
  const isFallback = source === "sqlite_fallback";
  const burnPerHour =
    burnRateTpm != null
      ? burnRateTpm * 60 >= 1_000_000
        ? `${((burnRateTpm * 60) / 1_000_000).toFixed(1)}M/h`
        : `${((burnRateTpm * 60) / 1_000).toFixed(0)}K/h`
      : null;
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: isFallback ? "auto 1fr auto" : "auto 1fr",
        gap: 8,
        alignItems: "center",
        padding: "5px 10px",
        borderBottom: "1px solid var(--line)",
        background: "var(--bg)",
        flexShrink: 0,
      }}
    >
      <span className="px-7 dim">BLOCK 5H</span>
      <div className="tm-16 tnum" style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
        {tokens}
        {limit && <span className="dim">/{limit}</span>}
        {burnPerHour && <span className="dim">  ↗ {burnPerHour}</span>}
        {projection && <span className="dim">  → {projection}</span>}
      </div>
      {isFallback && (
        <span
          className="px-7"
          title="ccusage 못 찾음 — 자체 추정치로 폴백 중"
          style={{
            padding: "2px 5px",
            border: "1px solid var(--line)",
            background: "var(--bg)",
            color: "var(--muted)",
            whiteSpace: "nowrap",
          }}
        >
          ! FALLBACK
        </span>
      )}
    </div>
  );
}

/* ─── TOKEN footer — TODAY / TOTAL ────────────────────── */
export function TokenFooter({ today, total }: { today: string; total: string }) {
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "1fr 1fr",
        borderTop: "2px solid var(--line)",
        background: "var(--bg)",
        flexShrink: 0,
      }}
    >
      <div style={{ padding: "5px 10px", borderRight: "1px solid var(--line)" }}>
        <div className="px-7 dim">TODAY · cache_read</div>
        <div className="px-12 tnum" style={{ marginTop: 3 }}>{today}</div>
      </div>
      <div style={{ padding: "5px 10px" }}>
        <div className="px-7 dim">TOTAL</div>
        <div className="px-12 tnum" style={{ marginTop: 3 }}>{total}</div>
      </div>
    </div>
  );
}

/* ─── Dungeon Log glyph map + entry row ───────────────── */
export const LOG_GLYPHS: Record<string, string> = {
  enter: "[>]",
  battle: "[!]",
  boss: "[★]",
  quest: "[Q]",
  levelup: "[▲]",
  rest: "[Z]",
};

export function LogRow({
  time,
  kind = "battle",
  text,
  gain,
}: {
  time: string;
  kind?: keyof typeof LOG_GLYPHS;
  text: string;
  gain?: string;
}) {
  const star = kind === "boss" || kind === "levelup";
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "54px 32px 1fr auto",
        gap: 6,
        padding: "5px 10px",
        borderBottom: "1px dashed var(--line)",
        background: star ? "var(--accent)" : "transparent",
        alignItems: "baseline",
      }}
    >
      <div className="px-7 dim tnum">{time}</div>
      <div className="px-8">{LOG_GLYPHS[kind] || "[·]"}</div>
      <div className="tm-16" style={{ lineHeight: 1.2 }}>{text}</div>
      {gain && <div className="px-7 tnum" style={{ whiteSpace: "nowrap" }}>{gain}</div>}
    </div>
  );
}

/* ─── Setting row + toggle ────────────────────────────── */
export function SettingRow({
  code,
  label,
  sublabel,
  right,
}: {
  code: string;
  label: string;
  sublabel?: string;
  right: ReactNode;
}) {
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "34px 1fr auto",
        gap: 8,
        alignItems: "center",
        padding: "7px 10px",
        borderBottom: "1px dashed var(--line)",
      }}
    >
      <div className="px-7 dim tnum">{code}</div>
      <div>
        <div className="px-8">{label}</div>
        {sublabel && <div className="tm-14 dim" style={{ marginTop: 3 }}>{sublabel}</div>}
      </div>
      {right}
    </div>
  );
}

export function Toggle({ value, onChange }: { value: boolean; onChange?: (v: boolean) => void }) {
  return (
    <button
      onClick={() => onChange?.(!value)}
      style={{
        all: "unset",
        cursor: onChange ? "pointer" : "default",
        display: "inline-flex",
        border: "1px solid var(--line)",
        fontFamily: "var(--pixel)",
        fontSize: 7,
      }}
    >
      <span
        style={{
          padding: "3px 6px",
          background: value ? "var(--accent)" : "var(--bg)",
          fontWeight: 700,
        }}
      >
        ON
      </span>
      <span
        style={{
          padding: "3px 6px",
          background: !value ? "var(--fg)" : "var(--bg)",
          color: !value ? "var(--bg)" : "var(--fg)",
          borderLeft: "1px solid var(--line)",
          fontWeight: 700,
        }}
      >
        OFF
      </span>
    </button>
  );
}

/* ─── Banner (battle/branch/critical) ─────────────────── */
export function Banner({
  children,
  tone = "acc",
}: {
  children: ReactNode;
  tone?: "acc" | "fg";
}) {
  return (
    <div
      style={{
        padding: "4px 10px",
        background: tone === "acc" ? "var(--accent)" : "var(--fg)",
        color: tone === "acc" ? "var(--fg)" : "var(--bg)",
        borderBottom: "1px solid var(--line)",
        display: "flex",
        alignItems: "center",
        gap: 8,
        fontFamily: "var(--pixel)",
        fontSize: 8,
        flexShrink: 0,
      }}
    >
      {children}
    </div>
  );
}

/* ─── Eat badge (kept for back-compat in App.tsx) ─────── */
export function EatBadge({ tokens }: { tokens: number }) {
  const fmt = (n: number) =>
    n >= 1e6 ? (n / 1e6).toFixed(1) + "M" : n >= 1e3 ? (n / 1e3).toFixed(1) + "K" : String(n);
  return (
    <div
      className="px-8"
      style={{
        position: "absolute",
        top: 10,
        right: 10,
        padding: "4px 6px",
        background: "var(--accent)",
        color: "var(--fg)",
        border: "1px solid var(--line)",
        display: "flex",
        alignItems: "center",
        gap: 4,
      }}
    >
      <span>★</span>
      <span>+{fmt(tokens)}</span>
    </div>
  );
}

/* ─── Legacy export shims (kept so unrelated files still compile) ── */
export function Gauge({
  label,
  value,
  max = 100,
  accent = false,
}: {
  label: string;
  value: number;
  max?: number;
  accent?: boolean;
}) {
  const pct = Math.min(100, (value / max) * 100);
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "64px 1fr 44px",
        gap: 8,
        alignItems: "center",
        fontSize: 11,
        padding: "2px 0",
      }}
    >
      <div className="px-7">{label}</div>
      <div className="hpbar">
        <div
          className="fill"
          style={{ width: `${pct}%`, background: accent ? "var(--accent)" : "var(--fg)" }}
        />
      </div>
      <div className="px-7 tnum" style={{ textAlign: "right" } as CSSProperties}>
        {value}
      </div>
    </div>
  );
}

export function EvolutionTrack({ stage }: { stage: string; pct: number }) {
  return (
    <div style={{ padding: "4px 10px", fontSize: 10 }}>
      <span className="px-7 dim">STAGE</span>
      <span className="px-8" style={{ marginLeft: 8 }}>{stage.toUpperCase()}</span>
    </div>
  );
}

export function DiaryRow({
  time,
  glyph,
  text,
}: {
  time: string;
  glyph: string;
  text: string;
  kind?: "normal" | "alert" | "star";
}) {
  return (
    <LogRow time={time} kind={(glyph as keyof typeof LOG_GLYPHS) in LOG_GLYPHS ? (glyph as keyof typeof LOG_GLYPHS) : "battle"} text={text} />
  );
}

/* ─── Shared tiny-markdown renderer ──────────────────────────
   Headings (## / ###), bullets (- / *), bold (**…**), inline code (`…`).
   No nested lists, no tables — enough for the prompt-shaped coaching output
   without pulling a markdown lib. Used by Retrospective + CrossCoaching. */
export function renderMarkdown(src: string): ReactNode {
  const lines = src.replace(/\r\n/g, "\n").split("\n");
  const out: ReactNode[] = [];
  let buf: string[] = [];
  const flushPara = () => {
    if (buf.length === 0) return;
    out.push(
      <p key={`p${out.length}`} style={{ margin: "5px 0", lineHeight: 1.65, overflowWrap: "anywhere" }}>
        {mdInline(buf.join(" "))}
      </p>,
    );
    buf = [];
  };
  let listBuf: string[] = [];
  const flushList = () => {
    if (listBuf.length === 0) return;
    out.push(
      // phosphor "·" markers with a hanging indent — cleaner than browser discs
      // in the CRT, and each item breathes (readability pass).
      <ul key={`u${out.length}`} style={{ listStyle: "none", margin: "6px 0", padding: 0, lineHeight: 1.55, overflowWrap: "anywhere" }}>
        {listBuf.map((item, i) => (
          <li key={i} style={{ position: "relative", paddingLeft: 15, margin: "4px 0" }}>
            <span style={{ position: "absolute", left: 3, top: 0, color: "var(--phos-dim, var(--muted))" }}>·</span>
            {mdInline(item)}
          </li>
        ))}
      </ul>,
    );
    listBuf = [];
  };
  for (const raw of lines) {
    const line = raw.trimEnd();
    if (line.startsWith("## ")) {
      flushPara(); flushList();
      // Section header = label + hairline rule (matches the "코치의 말" divider).
      // Pixel font, no uppercase — reads cleanly in Korean, unlike the old
      // mono+CAPS .px-8.
      out.push(
        <div key={`h${out.length}`} style={{ display: "flex", alignItems: "center", gap: 8, margin: "15px 0 7px" }}>
          <span style={{ fontFamily: "var(--pixel)", fontSize: 12.5, color: "var(--phos, var(--fg))", letterSpacing: "0.02em", whiteSpace: "nowrap" }}>
            {line.slice(3)}
          </span>
          <span style={{ flex: 1, height: 1.5, background: "var(--phos-30, var(--line))" }} />
        </div>,
      );
    } else if (line.startsWith("### ")) {
      flushPara(); flushList();
      out.push(
        <div key={`h${out.length}`} style={{ fontFamily: "var(--pixel)", fontSize: 11.5, color: "var(--phos-dim, var(--muted))", margin: "10px 0 3px", letterSpacing: "0.02em" }}>
          {line.slice(4)}
        </div>,
      );
    } else if (/^[-*]\s+/.test(line)) {
      flushPara();
      listBuf.push(line.replace(/^[-*]\s+/, ""));
    } else if (line.trim() === "") {
      flushPara(); flushList();
    } else {
      flushList();
      buf.push(line);
    }
  }
  flushPara();
  flushList();
  return out;
}

function mdInline(s: string): ReactNode {
  const parts: ReactNode[] = [];
  let i = 0;
  let key = 0;
  while (i < s.length) {
    if (s.startsWith("**", i)) {
      const end = s.indexOf("**", i + 2);
      if (end > 0) {
        parts.push(<strong key={key++}>{s.slice(i + 2, end)}</strong>);
        i = end + 2;
        continue;
      }
    }
    if (s[i] === "`") {
      const end = s.indexOf("`", i + 1);
      if (end > 0) {
        parts.push(
          <code key={key++} style={{ background: "var(--fg)", color: "var(--bg)", padding: "0 3px", fontFamily: "var(--mono, monospace)", wordBreak: "break-all" }}>
            {s.slice(i + 1, end)}
          </code>,
        );
        i = end + 1;
        continue;
      }
    }
    const nextSpecial = (() => {
      const a = s.indexOf("**", i);
      const b = s.indexOf("`", i);
      const xs = [a, b].filter((x) => x > -1);
      return xs.length ? Math.min(...xs) : s.length;
    })();
    parts.push(s.slice(i, nextSpecial));
    i = nextSpecial;
  }
  return parts;
}
