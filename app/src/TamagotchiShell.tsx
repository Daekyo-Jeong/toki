/**
 * Toki v4 — Cassette Futurism shell. Ported from the Claude Design
 * "Tokisoft" project (Toki v4.html). A beige Mac-classic plastic case
 * with a recessed dark CRT + cream phosphor screen. NO physical buttons —
 * soft buttons live inside the CRT and change per mode.
 *
 * Modes: home(pet) · pomodoro · menu · appearance · coaching · settings.
 * Coaching (intro→loading→result, "자세히" opens a report popup) and settings
 * (cursor list) both live fully inside the shell now — matching the design's
 * `toki-flows.jsx` (CoachIntro/CoachLoading/CoachResult/CoachReport/
 * SettingsScreen). No separate opaque sub-view window for either anymore.
 *
 * (V4-1 shell + V4-2 mode state machine. Pet variants = V4-3.
 *  V4-9: design-sync — coaching depth + settings-in-CRT.)
 */

import { useEffect, useMemo, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { AppSettings } from "./Settings";
import { casingVars, CASES, renderMarkdown } from "./components";

/* ─── 1-bit pet sprites (cream phosphor on CRT) ──────────── */

const P_IDLE = [
  ".......#........", "......#.#.......", ".......#........", "....########....",
  "..##........##..", ".#............#.", "#..............#", "#....##..##....#",
  "#....##..##....#", "#..............#", "#..............#", ".#....####....#.",
  ".#............#.", "..##........##..", "....########....", "................",
];
const P_HUNGRY = [
  ".......#........", "......#.#.......", "........#.......", "....########....",
  "..##........##..", ".#............#.", "#..............#", "#....##..##....#",
  "#....##..##....#", "#..............#", "#.....####.....#", "#....#....#....#",
  ".#....####....#.", "..##........##..", "....########....", "................",
];
const P_FOCUS = [
  ".......#........", "......###.......", ".......#........", "....########....",
  "..##........##..", ".#............#.", "#...##....##...#", "#....##..##....#",
  "#..............#", "#..............#", "#.....####.....#", ".#............#.",
  ".#............#.", "..##........##..", "....########....", "................",
];

/* 깡총이(기본 펫)의 상태 표정 — 배고픔은 귀가 처지고 입이 벌어지고,
   집중은 눈이 가늘어진다. 동글이와 같은 문법(눈·입 행만 변주). */
const PV_BUNNY_HUNGRY = [
  "................", "..##........##..", "..##........##..", "...##......##...",
  "....########....", "..############..", ".#............#.", "#....##..##....#",
  "#....##..##....#", "#..............#", "#.....####.....#", "#....#....#....#",
  "..############..", "....########....", "................", "................",
];
const PV_BUNNY_FOCUS = [
  ".....#....#.....", ".....#....#.....", "....##....##....", "....##....##....",
  "....########....", "..############..", "#...##....##...#", "#....##..##....#",
  "#..............#", "#..............#", "#.....####.....#", ".#............#.",
  "..############..", "....########....", "................", "................",
];

/* ─── 12 pet variants (V4-3) — 16x16 outline, silhouette-only diff ── */

const PV_CAT = [
  "...#........#...", "...##......##...", "...###....###...", "..############..",
  ".#............#.", "#..............#", "#....##..##....#", "#....##..##....#",
  "#..............#", "#..............#", ".#....####....#.", ".#............#.",
  "..############..", "....########....", "................", "................",
];
const PV_BUNNY = [
  ".....#....#.....", ".....#....#.....", "....##....##....", "....##....##....",
  "....########....", "..############..", ".#............#.", "#....##..##....#",
  "#....##..##....#", "#..............#", "#.....####.....#", ".#............#.",
  "..############..", "....########....", "................", "................",
];
const PV_HORN = [
  ".......#........", "......###.......", "......###.......", "....########....",
  "..############..", ".#............#.", "#..............#", "#....##..##....#",
  "#....##..##....#", "#..............#", "#.....####.....#", ".#............#.",
  "..############..", "....########....", "................", "................",
];
const PV_BOX = [
  ".......#........", "......###.......", ".......#........", "..############..",
  "..#..........#..", "..#..........#..", "..#.##....##.#..", "..#.##....##.#..",
  "..#..........#..", "..#...####...#..", "..#..........#..", "..############..",
  ".....#....#.....", ".....#....#.....", "................", "................",
];
const PV_SPROUT = [
  "..##...#...##...", ".####..#..####..", "..##...#...##...", ".......#........",
  "....########....", "..############..", ".#............#.", "#....##..##....#",
  "#....##..##....#", "#..............#", "#.....####.....#", ".#............#.",
  "..############..", "....########....", "................", "................",
];
const PV_HAT = [
  "....######......", "....######......", "...########.....", ".##############.",
  "..############..", ".#............#.", "#....##..##....#", "#....##..##....#",
  "#..............#", "#.....####.....#", ".#............#.", "..############..",
  "....########....", "................", "................", "................",
];
const PV_GLASSES = [
  ".......#........", "......#.#.......", ".......#........", "....########....",
  "..############..", ".#............#.", "#.####.##.####.#", "#.#@.#....#.@#.#",
  "#.####....####.#", "#..............#", "#.....####.....#", ".#............#.",
  "..############..", "....########....", "................", "................",
];
const PV_GHOST = [
  "................", ".....######.....", "...##########...", "..############..",
  ".#............#.", "#..............#", "#....##..##....#", "#....##..##....#",
  "#..............#", "#.....####.....#", "#..............#", "#..............#",
  "#..............#", "#..##..##..##..#", "................", "................",
];
const PV_CROWN = [
  "...#..#..#..#...", "..############..", ".#............#.", "#..............#",
  "#....##..##....#", "#....##..##....#", "#..............#", "#.....####.....#",
  "#..............#", ".#............#.", "..############..", "....########....",
  "................", "................", "................", "................",
];
const PV_BEAR = [
  ".##.........##..", ".####.....####..", "..############..", ".#............#.",
  "#..............#", "#....##..##....#", "#....##..##....#", "#......##......#",
  "#.....####.....#", "#..............#", ".#............#.", "..############..",
  "....########....", "................", "................", "................",
];
const PV_OCTO = [
  "................", "....######......", "..##########....", ".############...",
  ".#..........#...", ".#.@@....@@.#...", ".#.@@....@@.#...", ".#..........#...",
  ".#....##....#...", ".############...", ".#.#.#.#.#.#.#..", "#.#.#.#.#.#.#.#.",
  "................", "................", "................", "................",
];

// V4-19: Studio 애니 모델 — 상태별 {fps, frames}. 내장 12종은 정적 rows만
// 갖고(states 없음), Studio에서 온 유저 펫만 states를 실어 프레임 플레이어를 탄다.
type PetAnim = { w?: number; h?: number; fps?: number; frames: string[][] };
type UserPetData = { id: string; name: string; states: Record<string, PetAnim> };
type PetVariant = {
  id: string; name: string; rows: string[]; unlockLv: number;
  /** 상태 표정 — 있으면 배고픔·집중에서 이 스프라이트로 바뀐다. */
  hungryRows?: string[]; focusRows?: string[];
  states?: Record<string, PetAnim>;
};
// V4-3: milestone unlock by LEVEL (lv is the honest visualization of
// cumulative token usage, already available). Locked variants show as ???
// in the appearance carousel until the level is reached.
const PET_VARIANTS: PetVariant[] = [
  // 첫 자리 = 기본 펫(설정이 없을 때의 폴백). 앱 이름이 Toki라 토끼가 먼저다.
  { id: "bunny", name: "깡총이", rows: PV_BUNNY, unlockLv: 1, hungryRows: PV_BUNNY_HUNGRY, focusRows: PV_BUNNY_FOCUS },
  { id: "dong", name: "동글이", rows: P_IDLE, unlockLv: 1, hungryRows: P_HUNGRY, focusRows: P_FOCUS },
  { id: "cat", name: "뾰족이", rows: PV_CAT, unlockLv: 2 },
  { id: "horn", name: "뿔이", rows: PV_HORN, unlockLv: 5 },
  { id: "box", name: "네모", rows: PV_BOX, unlockLv: 7 },
  { id: "sprout", name: "새싹이", rows: PV_SPROUT, unlockLv: 9 },
  { id: "hat", name: "모자이", rows: PV_HAT, unlockLv: 12 },
  { id: "glasses", name: "안경이", rows: PV_GLASSES, unlockLv: 15 },
  { id: "ghost", name: "유령이", rows: PV_GHOST, unlockLv: 18 },
  { id: "crown", name: "왕관이", rows: PV_CROWN, unlockLv: 22 },
  { id: "bear", name: "곰이", rows: PV_BEAR, unlockLv: 26 },
  { id: "octo", name: "문어", rows: PV_OCTO, unlockLv: 30 },
];
/** 설정이 비었을 때의 펫. PET_VARIANTS 첫 항목과 같아야 한다. */
export const DEFAULT_PET = "bunny";
/** 기본 펫의 idle 스프라이트 — 온보딩 부화 화면이 쓴다. */
export const DEFAULT_PET_ROWS = PV_BUNNY;
const isUnlocked = (v: PetVariant, lv: number) => lv >= v.unlockLv;
const unlockedCountFor = (lv: number) => PET_VARIANTS.filter((v) => isUnlocked(v, lv)).length;
// Locked-preview silhouette — a neutral "?" blob, not the real sprite.
const P_LOCKED = [
  "................", "....######......", "..##......##....", ".#..........#...",
  "#....####....#..", "#...#....#...#..", "#........#...#..", "#.......#....#..",
  "#......#.....#..", "#.....#......#..", "#............#..", ".#....##....#...",
  "..#........#....", "...########.....", "......##........", "......##........",
];

export function PetSprite({ rows, scale = 6 }: { rows: string[]; scale?: number }) {
  const h = rows.length;
  const w = Math.max(...rows.map((r) => r.length));
  const cells: string[] = [];
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < (rows[y] || "").length; x++) {
      const ch = rows[y][x];
      // '@' = eye pixel — bright like '#', but also cursor-tracked (detectEyes).
      const fill = ch === "#" || ch === "@" ? "var(--phos)" : ch === "o" ? "var(--phos-dim)" : null;
      if (fill) cells.push(`<rect x="${x}" y="${y}" width="1" height="1" fill="${fill}"/>`);
    }
  }
  const svg = `<svg class="sprite" width="${w * scale}" height="${h * scale}" viewBox="0 0 ${w} ${h}" shape-rendering="crispEdges" style="display:block">${cells.join("")}</svg>`;
  return <span dangerouslySetInnerHTML={{ __html: svg }} />;
}

// Eye-only cursor tracking. Eyes are detected generically so it works for
// ANY pet sprite (12 variants + dong states), not just the default: an eye row
// is a symmetric pair of small `#` groups inside the face (cols 3..w-4, not the
// outline), nearest the vertical middle — this rejects ears/mouth/body-curves.
// Verified in node across all sprites (only "문어"/octo has no clean pair).
// Memoized per sprite-rows reference (called every render).
const eyeCache = new Map<string[], [number, number][]>();
function detectEyes(rows: string[]): [number, number][] {
  const cached = eyeCache.get(rows);
  if (cached) return cached;
  // Authored eyes win: any '@' marks an eye pixel. You can't reliably guess
  // eyes on arbitrary art (안경이's glasses fooled the heuristic below), so
  // Studio-drawn / migrated sprites designate them explicitly. The heuristic
  // is a fallback only for legacy sprites that carry no '@'.
  const authored: [number, number][] = [];
  for (let y = 0; y < rows.length; y++) {
    const row = rows[y] || "";
    for (let x = 0; x < row.length; x++) if (row[x] === "@") authored.push([y, x]);
  }
  if (authored.length) { eyeCache.set(rows, authored); return authored; }
  const h = rows.length, w = Math.max(...rows.map((r) => r.length));
  const lo = 3, hi = w - 4, c = (w - 1) / 2;
  const cand: { y: number; cols: number[] }[] = [];
  for (let y = 0; y < h; y++) {
    const row = rows[y] || "";
    const cols: number[] = [];
    for (let x = lo; x <= hi; x++) if (row[x] === "#") cols.push(x);
    if (cols.length < 2 || cols.length > 4) continue;
    let groups = 1;
    for (let i = 1; i < cols.length; i++) if (cols[i] !== cols[i - 1] + 1) groups++;
    if (groups !== 2) continue;
    const mir = cols.map((x) => Math.round(2 * c - x)).sort((a, b) => a - b);
    if (JSON.stringify([...cols].sort((a, b) => a - b)) !== JSON.stringify(mir)) continue;
    cand.push({ y, cols });
  }
  let cells: [number, number][] = [];
  if (cand.length) {
    const mid = h / 2;
    cand.sort((a, b) => Math.abs(a.y - mid) - Math.abs(b.y - mid));
    const top = cand[0];
    for (const cc of cand)
      if (Math.abs(cc.y - top.y) <= 1 && JSON.stringify(cc.cols) === JSON.stringify(top.cols))
        for (const x of cc.cols) cells.push([cc.y, x]);
  }
  eyeCache.set(rows, cells);
  return cells;
}
// Clear the detected eyes and redraw them shifted by (dx,dy) native px — only
// the eyes move, crisp in the pixel grid (no CSS scale/blur).
function withGaze(rows: string[], eyes: [number, number][], dx: number, dy: number): string[] {
  if ((dx === 0 && dy === 0) || eyes.length === 0) return rows;
  const g = rows.map((r) => r.split(""));
  for (const [r, cc] of eyes) if (g[r]?.[cc] !== undefined) g[r][cc] = ".";
  for (const [r, cc] of eyes) {
    const nr = r + dy, nc = cc + dx;
    if (g[nr] && nc >= 0 && nc < g[nr].length) g[nr][nc] = "#";
  }
  return g.map((row) => row.join(""));
}

/* ─── shell primitives ───────────────────────────────────── */

type SoftItem = { label: string; primary?: boolean; onClick?: () => void };

function SoftBar({ items }: { items: SoftItem[] }) {
  return (
    <div style={{ display: "flex", gap: 7, paddingTop: 6, borderTop: "1.5px solid var(--phos-30)" }}>
      {items.map((it, i) => (
        <button key={i} className={"cf-soft" + (it.primary ? " primary" : "")} onClick={it.onClick}>
          {it.label}
        </button>
      ))}
    </div>
  );
}

/* P1-1: real 5h-quota battery. `pct` = utilization 0..100 (higher = more of
   the 5h block consumed). The battery shows REMAINING quota, so it drains as
   you use Claude: fresh block = 5 full cells, near the cap = empty. `null`
   while usage is still loading (renders a faint empty shell). Rendered in the
   case chrome (brand bar), so it uses `ink` (dark print) not phosphor. */
/** 사용률 소스 → 사람이 아는 이름. 배터리가 **누구 쿼터**를 가리키는지는
    화면에 라벨로 붙이지 않기로 했지만(spec §5.3 — 허기 게이지지 대시보드가
    아니다), 툴팁으로는 답할 수 있어야 한다. 혼용 사용자는 배터리가 조용히
    기준을 바꾸면 "왜 2칸이지?"에서 막힌다(2026-09-04 실제 질문). */
function sourceLabel(src?: string): string {
  switch (src) {
    case "OauthUsage": return "Claude";
    case "CodexRollout": return "Codex";
    case "Ccusage":
    case "NpxCcusage": return "Claude(근사)";
    case "SqliteFallback": return "로컬 근사";
    default: return "";
  }
}

/** 에이전트별 쿼터 사용률(배터리 툴팁용). 아래 컴포넌트 안에도 같은 이름의
    다른 타입(`get_agent_usage`, 토큰 집계)이 있어 이름을 갈라둔다. */
type AgentQuota = { agent: string; used_percent: number; window_minutes: number };

function Battery({ pct, source, agents, ink = "var(--phos)" }: {
  pct: number | null; source?: string; agents?: AgentQuota[]; ink?: string;
}) {
  const CELLS = 5;
  const remaining = pct == null ? null : Math.max(0, 100 - pct);
  // round so 44% remaining → 2 cells, 90% → 5, 10% → 1 (not 0 until truly spent).
  const lit = remaining == null ? 0 : Math.min(CELLS, Math.ceil(remaining / 20));
  const low = remaining != null && lit <= 1; // ~≥80% used — warn in rust
  const fill = low ? "var(--rust)" : ink;
  // 창 종류("5h"/"주간")는 안 쓴다 — Codex는 주간 창이고 UI는 창을 표시하지
  // 않는 게 원칙(spec §5.3). 허기 게이지지 쿼터 대시보드가 아니다.
  const who = sourceLabel(source);
  /* 배터리는 하나만 따르지만(최근 활동 에이전트) 툴팁엔 **아는 것을 다 적는다.**
     혼용 사용자는 그래야 "왜 이 칸수인지"를 스스로 안다. 따르는 쪽에 ▸를 붙인다. */
  const NAMES: Record<string, string> = { claude: "Claude", codex: "Codex" };
  /* **잔량으로 적는다.** 배터리는 남은 양을 그리는데 툴팁만 사용량이면
     숫자와 칸이 정반대로 읽힌다(2026-09-04 사용자 지적: "77%인데 2칸?"). */
  const lines = (agents ?? []).map((a) => {
    const nm = NAMES[a.agent] ?? a.agent;
    const mark = who && nm === who ? "▸ " : "   ";
    return `${mark}${nm} ${Math.round(Math.max(0, 100 - a.used_percent))}% 남음`;
  });
  const title = pct == null
    ? "사용량 확인 중"
    : lines.length > 1
      ? `배터리 ${lit}/${CELLS}칸\n${lines.join("\n")}`
      : `${who ? who + " " : ""}${Math.round(remaining ?? 0)}% 남음 · ${lit}/${CELLS}칸`;
  return (
    <span title={title} style={{ display: "inline-flex", alignItems: "center", gap: 1 }}>
      {/* body: outlined box holding the cells */}
      <span style={{ display: "inline-flex", gap: 1, alignItems: "center",
        padding: "1.5px", border: `1px solid ${ink}`, borderRadius: 1, opacity: pct == null ? 0.4 : 1 }}>
        {Array.from({ length: CELLS }).map((_, i) => (
          <span key={i} style={{ width: 3, height: 7, background: i < lit ? fill : "transparent" }} />
        ))}
      </span>
      {/* terminal nub */}
      <span style={{ width: 1.5, height: 4, background: ink, opacity: pct == null ? 0.4 : 1 }} />
    </span>
  );
}

function CrtScreen({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ position: "relative", borderRadius: 9, overflow: "hidden", height: 168,
      background: "radial-gradient(120% 130% at 50% 30%, var(--scr2) 0%, var(--scr) 78%, #0e0e15 100%)",
      boxShadow: "inset 0 0 34px rgba(0,0,0,.7), inset 0 0 0 1px rgba(0,0,0,.5)" }}>
      <div style={{ position: "relative", zIndex: 2, height: "100%", padding: "6px 9px", display: "flex", flexDirection: "column", color: "var(--phos)",
        /* CRT 기본 타이포 = Galmuri9 @10px. 폰트별 픽셀 퍼펙트 크기가 정해져
           있어(Mono9/Galmuri9=10px 정수배, 11계열=12px 정수배) 크기와 스택은
           항상 짝으로 바꿔야 한다 — 크기만 바꾸면 행이 빠져 깨진다. */
        fontFamily: "var(--pixel-9)", fontSize: 10 }}>
        {children}
      </div>
      <div style={{ position: "absolute", inset: 0, zIndex: 3, pointerEvents: "none", backgroundImage: "repeating-linear-gradient(0deg, transparent 0 2px, rgba(0,0,0,.22) 2px 3px)" }} />
      <div style={{ position: "absolute", inset: 0, zIndex: 3, pointerEvents: "none", background: "radial-gradient(90% 80% at 50% 42%, rgba(233,234,208,0.06), transparent 70%)" }} />
      <div style={{ position: "absolute", inset: 0, zIndex: 4, pointerEvents: "none", background: "linear-gradient(122deg, rgba(255,255,255,0.06) 0%, transparent 34%)" }} />
    </div>
  );
}

function Screen({ status, children, buttons }: { status?: React.ReactNode; children: React.ReactNode; buttons?: SoftItem[] }) {
  return (
    <>
      {status != null && (
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", fontFamily: "var(--pixel-9)", fontSize: 10, color: "var(--phos)", paddingBottom: 5, borderBottom: "1.5px solid var(--phos-30)" }}>
          {status}
        </div>
      )}
      <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column", justifyContent: "center", padding: "2px 0" }}>
        {children}
      </div>
      {buttons && <SoftBar items={buttons} />}
    </>
  );
}

const screw = (
  <span style={{ width: 7, height: 7, borderRadius: "50%", display: "inline-flex", alignItems: "center", justifyContent: "center", background: "radial-gradient(circle at 40% 35%, var(--case-hi), var(--case-lo))", boxShadow: "inset 0 0 0 1px rgba(0,0,0,.18)" }}>
    <span style={{ width: 5, height: 1.5, background: "var(--case-line)" }} />
  </span>
);
const vents = (
  <div style={{ flex: 1, display: "flex", flexDirection: "column", gap: 3 }}>
    {[0, 1, 2].map((i) => <span key={i} style={{ height: 3, borderRadius: 2, background: "var(--case-line)", opacity: 0.5, boxShadow: "0 1px 0 var(--case-hi)" }} />)}
  </div>
);

export function CassetteShell({ children, model = "T-1986", innerRef, casing = "beige", invert = false }: {
  children: React.ReactNode; model?: string; innerRef?: React.Ref<HTMLDivElement>;
  casing?: string; invert?: boolean;
}) {
  const vars = casingVars(casing, invert);
  return (
    // v4: floats on the transparent window — rounded corners + drop shadow.
    <div ref={innerRef} style={{ ...vars, width: 240, padding: "10px 10px 11px",
      background: "linear-gradient(158deg, var(--case-hi) 0%, var(--case) 30%, var(--case) 70%, var(--case-lo) 100%)",
      borderRadius: 15,
      // Small shadow that fits the tight window margin — keeps the
      // click-dead transparent ring minimal (Tauri captures clicks on
      // transparent areas too, so we minimize that area).
      boxShadow: "0 4px 12px rgba(30,25,15,.34), inset 0 2px 0 rgba(255,255,255,.28), inset 0 -8px 16px rgba(0,0,0,.12)",
      display: "flex", flexDirection: "column" }}>
      {/* top: brand + tricolor stripe + power LED */}
      <div style={{ display: "flex", alignItems: "center", gap: 7, height: 16, marginBottom: 7, padding: "0 2px" }}>
        <span style={{ fontFamily: "var(--pixel)", fontSize: 12, color: "var(--case-ink)", letterSpacing: "0" }}>toki</span>
        <span style={{ display: "flex", height: 5, gap: 0, boxShadow: "0 0 0 1px rgba(0,0,0,.12)" }}>
          <span style={{ width: 9, height: 5, background: "var(--rust)" }} />
          <span style={{ width: 9, height: 5, background: "var(--amber)" }} />
          <span style={{ width: 9, height: 5, background: "var(--teal)" }} />
        </span>
        <span style={{ flex: 1 }} />
        <span style={{ width: 6, height: 6, borderRadius: "50%", background: "var(--teal)", boxShadow: "0 0 5px var(--teal), inset 0 0 0 1px rgba(0,0,0,.2)" }} />
      </div>
      {/* recessed CRT bezel */}
      <div style={{ padding: 7, borderRadius: 11, background: "linear-gradient(160deg, var(--case-lo), var(--case))", boxShadow: "inset 0 3px 7px rgba(0,0,0,.4), inset 0 -1px 0 var(--case-hi), 0 1px 0 rgba(255,255,255,.2)" }}>
        <CrtScreen>{children}</CrtScreen>
      </div>
      {/* bottom deck: screw + vents + label + vents + screw */}
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginTop: 9, padding: "0 2px" }}>
        {screw}{vents}
        <span style={{ fontFamily: "var(--pixel-mono-9)", fontSize: 10, color: "var(--case-ink)", letterSpacing: "1px", whiteSpace: "nowrap" }}>MODEL {model}</span>
        {vents}{screw}
      </div>
    </div>
  );
}

/* ─── retro System alerts (V4-5) — rendered as an in-CRT overlay,
   not a second OS window. Chrome (title stripe/close-zoom boxes) is
   its own look; body reuses the same phosphor buttons/sprite. ──── */

/* 팝업을 타이틀바로 끌어 옮긴다 (2026-09-02).
   구형 Mac 창을 흉내낸 크롬이라 잡으면 움직여야 하는데 셸만 되고 팝업은
   안 됐다. 세 팝업(AlertDialog·RetroReportModal·DeepReportModal)이 같은
   타이틀바를 쓰므로 훅 하나로 전부 붙인다.

   - **본문이 아니라 타이틀바만** 손잡이다. 리포트 본문은 사용자가 긁어
     복사하는 자리라 드래그를 걸면 선택이 죽는다.
   - 원래 배치(중앙 정렬·CRT 오버레이)를 건드리지 않으려고 좌표를 다시
     잡지 않고 `translate` 오프셋만 얹는다.
   - 좌표는 **정수 px**로 반올림한다 — 소수점이면 그 안의 픽셀 글자가
     반픽셀에 앉아 1x 화면에서 깨진다(design-brief §6.6).
   - 팝업이 열릴 때마다 오프셋 0에서 시작한다(컴포넌트가 새로 마운트됨). */
function useDragBox() {
  const [d, setD] = useState({ x: 0, y: 0 });
  const moved = useRef(false);
  const boxRef = useRef<HTMLElement | null>(null);

  const onPointerDown = (e: ReactPointerEvent) => {
    // 타이틀바 안의 close/zoom 박스는 버튼이다 — 드래그로 먹지 않는다.
    if ((e.target as HTMLElement).closest("[data-dlg-btn]")) return;
    e.preventDefault();
    // 셸(.shell-wrap)의 드래그 핸들러가 React 트리로 이 이벤트를 받지 않게.
    e.stopPropagation();
    const sx = e.clientX - d.x;
    const sy = e.clientY - d.y;
    moved.current = false;
    // 드래그 시작 시점의 박스 — 클램프 계산에 쓴다.
    const el = (e.currentTarget as HTMLElement).closest("[data-drag-box]") as HTMLElement | null;
    boxRef.current = el;
    const move = (ev: PointerEvent) => {
      moved.current = true;
      setD(clamp(Math.round(ev.clientX - sx), Math.round(ev.clientY - sy)));
    };
    // 화면 밖으로 완전히 끌어내면 돌아올 방법이 없다 — 팝업엔 close가 없고
    // 백드롭이 클릭을 삼켜서 뒤의 셸도 못 누른다(2026-09-03 실기: 온보딩이
    // 이렇게 잠겼다). 타이틀바가 늘 보이도록 이동량을 가둔다.
    const clamp = (x: number, y: number) => {
      const el = boxRef.current;
      if (!el) return { x, y };
      const r = el.getBoundingClientRect();
      // 지금 transform을 뺀 "원래 자리"를 기준으로 허용 범위를 잡는다.
      const baseLeft = r.left - d.x;
      const baseTop = r.top - d.y;
      const KEEP = 28; // 최소한 타이틀바 한 줄은 화면 안에 남긴다
      const minX = -(baseLeft + r.width - KEEP);
      const maxX = window.innerWidth - baseLeft - KEEP;
      const minY = -baseTop;
      const maxY = window.innerHeight - baseTop - KEEP;
      return {
        x: Math.min(Math.max(x, minX), maxX),
        y: Math.min(Math.max(y, minY), maxY),
      };
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      // 드래그로 끝난 pointerup 직후에 오는 click은 백드롭 닫기로 오인되면 안 되니
      // 그 click이 플래그를 읽고 난 다음 틱에 되돌린다. 안 되돌리면 한 번 끈 뒤로
      // 백드롭 클릭이 영원히 무시된다 (2026-09-02 실기).
      setTimeout(() => { moved.current = false; }, 0);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return {
    /* 타이틀바에 편다. style은 호출부가 이미 쓰고 있어 충돌을 피하려고
       핸들러만 넘기고 `cursor: grab`은 각자 style에 적는다. */
    handle: { onPointerDown },
    /* 박스 style에 펼친다. `data-drag-box`는 클램프가 박스를 찾는 표식이다. */
    box: { transform: `translate(${d.x}px, ${d.y}px)`, "data-drag-box": "" } as React.CSSProperties,
    /* 드래그로 끝난 포인터였나 — 백드롭 클릭이 닫기로 오인되는 걸 막는다 */
    didDrag: () => moved.current,
  };
}

function DlgBox({ zoom }: { zoom?: boolean }) {
  return (
    <span style={{ width: 13, height: 11, border: "1.5px solid var(--phos)", background: "var(--scr2)", position: "relative", flex: "0 0 auto", display: "inline-block" }}>
      {zoom && <span style={{ position: "absolute", top: 1, right: 1, width: 5, height: 4, border: "1.5px solid var(--phos)", borderLeft: "none", borderBottom: "none" }} />}
    </span>
  );
}

export function AlertDialog({
  title = "메시지", children, sprite, spriteScale = 4, ok = "Ok", onOk, cancel, onCancel,
  copyright, width = 208, okDisabled, cancelDisabled,
}: {
  title?: string; children: React.ReactNode; sprite?: string[]; spriteScale?: number;
  ok?: string; onOk?: () => void; cancel?: string; onCancel?: () => void;
  copyright?: string; width?: number; okDisabled?: boolean; cancelDisabled?: boolean;
}) {
  const drag = useDragBox();
  return (
    <div style={{ width, fontFamily: "var(--pixel)", color: "var(--phos)", ...drag.box }}>
      <div style={{ background: "var(--scr)", border: "1.5px solid var(--phos)", boxShadow: "3px 4px 0 rgba(0,0,0,.5)" }}>
        <div {...drag.handle} style={{ position: "relative", display: "flex", alignItems: "center", gap: 5, padding: "3px 5px", height: 18, borderBottom: "1.5px solid var(--phos-30)", overflow: "hidden", background: "var(--scr2)", cursor: "grab" }}>
          <div style={{ position: "absolute", inset: 0, pointerEvents: "none", backgroundImage: "repeating-linear-gradient(to bottom, var(--phos-dim) 0 1.5px, transparent 1.5px 4px)" }} />
          <span style={{ position: "relative", display: "flex", gap: 3 }}><DlgBox /></span>
          <span style={{ flex: 1 }} />
          <span style={{ position: "relative", background: "var(--scr2)", padding: "0 8px", fontFamily: "var(--pixel-9)", fontSize: 10, letterSpacing: "0", whiteSpace: "nowrap", color: "var(--phos)" }}>{title}</span>
          <span style={{ flex: 1 }} />
          <span style={{ position: "relative", display: "flex", gap: 3 }}><DlgBox /><DlgBox zoom /></span>
        </div>
        <div style={{ background: "var(--scr)", padding: "10px 11px 9px" }}>
          <div style={{ border: "1.5px solid var(--phos-30)", padding: "10px 9px 11px", display: "flex", flexDirection: "column", alignItems: "center", gap: 8 }}>
            {/* CJK 기본 line-break는 단어 중간에서 끊는다("설정 파/일에").
                keep-all로 어절을 지키고, 긴 경로만 강제로 접는다. */}
            <div style={{ textAlign: "center", fontSize: 12, lineHeight: "18px", whiteSpace: "pre-line", color: "var(--phos)", wordBreak: "keep-all", overflowWrap: "anywhere" }}>{children}</div>
            {sprite && <PetSprite rows={sprite} scale={spriteScale} />}
            {/* 버튼이 아예 없는 단계가 있다(온보딩 "읽는 중" — 자동 전이).
                빈 라벨 버튼이 빈 상자로 남지 않게 줄째로 없앤다. */}
            {(ok || cancel) && (
              <div style={{ display: "flex", gap: 6, marginTop: 2 }}>
                {cancel && <button className="cf-soft auto" onClick={onCancel} disabled={cancelDisabled}>{cancel}</button>}
                {ok && <button className="cf-soft auto primary" onClick={onOk} disabled={okDisabled}>{ok}</button>}
              </div>
            )}
          </div>
        </div>
      </div>
      {/* 프레임 밖이라 뒤 배경(케이스 각인·바탕화면)이 그대로 비쳐 대비가
          1.05:1까지 떨어졌다(2026-09-03 실측). 서명은 유지하되 플레이트를 깐다. */}
      {copyright && (
        <div style={{ display: "flex", justifyContent: "flex-end", marginTop: 5 }}>
          <span style={{ fontFamily: "var(--pixel-mono-9)", fontSize: 10, color: "var(--phos-dim)", background: "var(--scr)", padding: "1px 4px" }}>{copyright}</span>
        </div>
      )}
    </div>
  );
}

/* ─── coaching report popup — retro terminal chrome (dark backdrop, title
   bar, tk-report pop-in). V5.1: 코칭 = 딥 분석 단일 흐름 (사용자 결정
   2026-08-31 "딥분석이 기존의 분석이 되면 됨"). 로컬 Ollama 크로스세션
   티어(신호 카드 + CoachReportModal)는 UI에서 내려갔다 — 백엔드
   (coaching_cross_*)는 남아 있어 되살릴 수 있음. 회고는 여전히 로컬. */

// V5: 딥 코칭 — 프롬프트 히스토리 원문 + 자산 인벤토리 + engram을
// claude -p(sonnet)가 추론한 결과 (자유 추론 마크다운).
type DeepCoaching = {
  body: string;
  n_prompts: number;
  n_days: number;
  model: string;
  generated_at: string;
};

// A/V4-14: a project the 회고 picker lists (recent sessions grouped by project).
type ProjectInfo = { dir: string; label: string; last_active: string; sessions: number };

/** "방금 / N분 전 / N시간 전 / N일 전" for a cached analysis timestamp. */
function relTime(iso: string): string {
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return "";
  const s = Math.max(0, (Date.now() - t) / 1000);
  if (s < 60) return "방금";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}분 전`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}시간 전`;
  return `${Math.floor(h / 24)}일 전`;
}

// Clipboard write that works inside WKWebView (Tauri): try the async API
// (secure context under the custom protocol), fall back to a hidden textarea +
// execCommand. Returns whether it succeeded.
async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch { /* fall through to legacy path */ }
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    return ok;
  } catch {
    return false;
  }
}

// A/V4-14: session-retro report — a markdown body (no signal cards). Same
// chrome as the coaching report + a copy button (retro markdown + a lead-in
// asking an AI to help act on it).
function retroToText(body: string): string {
  return `# 내 이번 Claude Code 세션 회고 (Toki)\n\n${body.trim()}\n\n---\n위는 방금 끝낸 내 AI 코딩 세션 회고야. 이걸 바탕으로 다음 세션에서 뭘 다르게 하면 좋을지 구체적으로 짚어줘.`;
}

function RetroReportModal({ body, error, title, onClose }: {
  body: string | null; error: string | null; title?: string; onClose: () => void;
}) {
  const [copied, setCopied] = useState(false);
  async function doCopy() {
    if (!body) return;
    const ok = await copyToClipboard(retroToText(body));
    if (ok) { setCopied(true); window.setTimeout(() => setCopied(false), 1500); }
  }
  const drag = useDragBox();
  return (
    <div onClick={() => { if (!drag.didDrag()) onClose(); }} style={{ position: "fixed", inset: 0, zIndex: 220, display: "flex", alignItems: "center", justifyContent: "center", padding: 20, background: "rgba(14,12,18,.55)" }}>
      <div className="tk-report" onClick={(e) => e.stopPropagation()} style={{ width: 560, maxWidth: "100%", maxHeight: "82vh", display: "flex", flexDirection: "column", fontFamily: "var(--pixel)", color: "var(--phos)", ...drag.box }}>
        <div style={{ display: "flex", flexDirection: "column", minHeight: 0, background: "var(--scr)", border: "1.5px solid var(--phos)", boxShadow: "5px 6px 0 rgba(0,0,0,.5)" }}>
          <div {...drag.handle} style={{ position: "relative", display: "flex", alignItems: "center", gap: 6, flex: "0 0 auto", padding: "5px 8px", height: 26, borderBottom: "1.5px solid var(--phos-30)", overflow: "hidden", background: "var(--scr2)", cursor: "grab" }}>
            <div style={{ position: "absolute", inset: 0, pointerEvents: "none", backgroundImage: "repeating-linear-gradient(to bottom, var(--phos-dim) 0 1.5px, transparent 1.5px 4px)" }} />
            <span data-dlg-btn onClick={onClose} style={{ position: "relative", width: 13, height: 11, cursor: "pointer", border: "1.5px solid var(--phos)", background: "var(--scr2)" }} />
            <span style={{ flex: 1 }} />
            <span style={{ position: "relative", background: "var(--scr2)", padding: "0 10px", fontSize: 12, whiteSpace: "nowrap" }}>{title ? `${title} 회고` : "회고"}</span>
            <span style={{ flex: 1 }} />
            <span style={{ position: "relative", width: 13, height: 11, border: "1.5px solid var(--phos)", background: "var(--scr2)" }} />
          </div>
          <div style={{ padding: 18, overflowY: "auto", minHeight: 0 }}>
            {error && <div className="pxmono" style={{ fontSize: 12, color: "var(--phos-dim)", overflowWrap: "anywhere" }}>{error}</div>}
            {body && <div style={{ fontSize: 12, lineHeight: "18px" }}>{renderMarkdown(body)}</div>}
            <div style={{ display: "flex", gap: 10, marginTop: 16 }}>
              <button className="cf-soft" style={{ flex: 1 }} onClick={onClose}>닫기</button>
              {body && (
                <button className="cf-soft primary" style={{ flex: 1 }} onClick={doCopy}>
                  {copied ? "복사됨" : "복사"}
                </button>
              )}
            </div>
          </div>
        </div>
        <div style={{ textAlign: "right", fontFamily: "var(--pixel-mono-9)", fontSize: 10, color: "var(--phos-dim)", marginTop: 8 }}>©1986 tokisoft</div>
      </div>
    </div>
  );
}

// V5: 딥 코칭 복붙 텍스트 — 그대로 Claude Code에 붙여 추천을 실행시키는 용도.
function deepToText(d: DeepCoaching): string {
  return `# 내 Claude Code 딥 코칭 (Toki · 최근 ${d.n_days}일 · 프롬프트 ${d.n_prompts}개 분석)\n\n${d.body.trim()}\n\n---\n위는 내 실제 사용 기록 기반 추천이야. 이 중 1번부터 실제로 세팅해줘.`;
}

// V5: 딥 코칭 리포트 — 회고 리포트와 같은 크롬, 추론 마크다운 본문.
// "다시 분석"이 안에 있는 이유: 딥은 쿼터를 쓰므로 재실행 진입점을 결과와 같은
// 자리에 두고, 버튼에 비용(claude)을 명시한다.
/** 코칭 실행 고지 (spec §9.2) — "뭉뚱그리지 않는다"가 계약이라 목록을 그대로
 *  보여준다. CRT 안에 넣으면 소스 4종에서 이미 넘쳐서 팝업으로 뺐다. */
function EgressModal({ preview, onClose }: { preview: DeepPreview | null; onClose: () => void }) {
  const drag = useDragBox();
  const rows: [string, string][] = [];
  if (preview) {
    rows.push([`최근 ${preview.n_days}일 프롬프트 원문`, `${preview.n_prompts}개`]);
    if (preview.has_inventory) rows.push(["자산 목록 (커맨드·스킬·에이전트)", "파일명"]);
    for (const s of preview.sources) {
      rows.push([`${s.kind === "rules" ? "규칙 파일" : "지속 기억"} · ${s.label}`, s.path]);
    }
    rows.push(["크로스세션 집계", "결정론 수치"]);
  }
  const line = preview
    ? preview.egresses
      ? `${preview.backend} 로 나가요 · 토키는 별도 채널을 열지 않아요`
      : "로컬 모델이라 이 컴퓨터 밖으로 나가지 않아요"
    : "준비 중";
  return (
    <div onClick={() => { if (!drag.didDrag()) onClose(); }} style={{ position: "fixed", inset: 0, zIndex: 220, display: "flex", alignItems: "center", justifyContent: "center", padding: 20, background: "rgba(14,12,18,.55)" }}>
      <div className="tk-report" onClick={(e) => e.stopPropagation()} style={{ width: 460, maxWidth: "100%", maxHeight: "82vh", display: "flex", flexDirection: "column", fontFamily: "var(--pixel)", color: "var(--phos)", ...drag.box }}>
        <div style={{ display: "flex", flexDirection: "column", minHeight: 0, background: "var(--scr)", border: "1.5px solid var(--phos)", boxShadow: "5px 6px 0 rgba(0,0,0,.5)" }}>
          <div {...drag.handle} style={{ position: "relative", display: "flex", alignItems: "center", gap: 6, flex: "0 0 auto", padding: "5px 8px", height: 26, borderBottom: "1.5px solid var(--phos-30)", overflow: "hidden", background: "var(--scr2)", cursor: "grab" }}>
            <div style={{ position: "absolute", inset: 0, pointerEvents: "none", backgroundImage: "repeating-linear-gradient(to bottom, var(--phos-dim) 0 1.5px, transparent 1.5px 4px)" }} />
            <span data-dlg-btn onClick={onClose} style={{ position: "relative", width: 13, height: 11, cursor: "pointer", border: "1.5px solid var(--phos)", background: "var(--scr2)" }} />
            <span style={{ flex: 1 }} />
            <span style={{ position: "relative", background: "var(--scr2)", padding: "0 10px", fontSize: 12, whiteSpace: "nowrap" }}>딥 분석 한 번에 나가는 것</span>
            <span style={{ flex: 1 }} />
            <span style={{ position: "relative", width: 13, height: 11, border: "1.5px solid var(--phos)", background: "var(--scr2)" }} />
          </div>
          <div style={{ padding: 16, overflowY: "auto", minHeight: 0, fontSize: 12, lineHeight: "18px" }}>
            <div style={{ color: "var(--phos-dim)", marginBottom: 10 }}>{line}</div>
            <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
              {rows.map(([what, from], i) => (
                <div key={i} style={{ display: "flex", justifyContent: "space-between", gap: 12, borderBottom: "1px solid var(--phos-30)", paddingBottom: 3 }}>
                  <span>{what}</span>
                  <span style={{ color: "var(--phos-dim)", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", maxWidth: 190 }}>{from}</span>
                </div>
              ))}
            </div>
            <div className="pxmono9" style={{ fontSize: 10, lineHeight: "15px", color: "var(--phos-dim)", marginTop: 12 }}>
              수집 서버 없음 · 저장은 전부 ~/.toki · 자격증명은 사용자 CLI 로그인을 빌려요.<br />
              쿼터를 아끼려면 설정에서 코칭 LLM을 LOCAL로 바꾸세요.
            </div>
            <div style={{ display: "flex", gap: 10, marginTop: 14 }}>
              <button className="cf-soft primary" style={{ flex: 1 }} onClick={onClose}>닫기</button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

/** 기억 붙여넣기 패널 (spec §6.4.1 · design-brief §13).
 *
 *  - 형식을 파싱하지 않는다 — 플레인 텍스트 그대로 `~/.toki/memory.md`로.
 *  - `.md`/`.txt` 업로드는 **필드로 읽어 들여 보여준 뒤** 저장한다. 뭐가
 *    들어갔는지 모르는 채로 저장되지 않게.
 *  - 셸은 드래그 방지로 `user-select: none`을 걸어놨다. 그 조상 체인 밑에서는
 *    WKWebView가 입력 캐럿을 아예 안 준다 — 필드에서 명시적으로 되살린다
 *    (메모 카드의 `.mc-title`이 같은 이유로 같은 처리를 한다).
 */
function MemoryPastePanel({ text, onChangeText, onClose, onSaved }: {
  text: string; onChangeText: (t: string) => void; onClose: () => void; onSaved: (bytes: number) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const drag = useDragBox();
  const bytes = byteLen(text);

  async function pickFile() {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({ multiple: false, filters: [{ name: "기억 파일", extensions: ["md", "txt"] }] });
      if (typeof picked !== "string") return;
      const body = await invoke<string>("read_text_file_utf8", { path: picked });
      // 덮어쓰지 않고 이어 붙인다 — 여러 도구의 기억을 모을 수 있게.
      onChangeText(text.trim() ? `${text.trimEnd()}\n\n${body.trim()}` : body.trim());
      setNote("불러왔어요 · 확인하고 저장");
    } catch (e) {
      setNote(String(e).slice(0, 60));
    }
  }

  async function save() {
    setBusy(true);
    try {
      const n = await invoke<number>("memory_paste_set", { text });
      onSaved(n);
    } catch (e) {
      setNote(String(e).slice(0, 60));
      setBusy(false);
    }
  }

  return (
    <div onClick={() => { if (!drag.didDrag()) onClose(); }} style={{ position: "fixed", inset: 0, zIndex: 220, display: "flex", alignItems: "center", justifyContent: "center", padding: 20, background: "rgba(14,12,18,.55)" }}>
      <div className="tk-report" onClick={(e) => e.stopPropagation()} style={{ width: 520, maxWidth: "100%", maxHeight: "82vh", display: "flex", flexDirection: "column", fontFamily: "var(--pixel)", color: "var(--phos)", ...drag.box }}>
        <div style={{ display: "flex", flexDirection: "column", minHeight: 0, background: "var(--scr)", border: "1.5px solid var(--phos)", boxShadow: "5px 6px 0 rgba(0,0,0,.5)" }}>
          <div {...drag.handle} style={{ position: "relative", display: "flex", alignItems: "center", gap: 6, flex: "0 0 auto", padding: "5px 8px", height: 26, borderBottom: "1.5px solid var(--phos-30)", overflow: "hidden", background: "var(--scr2)", cursor: "grab" }}>
            <div style={{ position: "absolute", inset: 0, pointerEvents: "none", backgroundImage: "repeating-linear-gradient(to bottom, var(--phos-dim) 0 1.5px, transparent 1.5px 4px)" }} />
            <span data-dlg-btn onClick={onClose} style={{ position: "relative", width: 13, height: 11, cursor: "pointer", border: "1.5px solid var(--phos)", background: "var(--scr2)" }} />
            <span style={{ flex: 1 }} />
            <span style={{ position: "relative", background: "var(--scr2)", padding: "0 10px", fontSize: 12, whiteSpace: "nowrap" }}>기억 붙여넣기</span>
            <span style={{ flex: 1 }} />
            <span style={{ position: "relative", width: 13, height: 11, border: "1.5px solid var(--phos)", background: "var(--scr2)" }} />
          </div>
          <div style={{ padding: 16, overflowY: "auto", minHeight: 0 }}>
            <div className="pxmono9" style={{ fontSize: 10, lineHeight: "15px", color: "var(--phos-dim)", marginBottom: 8 }}>
              쓰는 도구에서 기억을 뽑아 그대로 붙여넣으세요. 형식은 자유예요.<br />
              딥 코칭이 "기억에만 쌓인 것"을 자산으로 승격하라고 짚어줘요.
            </div>
            <textarea
              className="tk-mempaste"
              value={text}
              onChange={(e) => onChangeText(e.target.value)}
              spellCheck={false}
              placeholder="예) 나는 리포트를 표·차트 있는 서사형으로 원한다"
              style={{ width: "100%", height: 260, boxSizing: "border-box", padding: 10, background: "var(--scr2)",
                border: "1.5px solid var(--phos-30)", color: "var(--phos)", outline: "none", resize: "none",
                fontFamily: "var(--pixel)", fontSize: 12, lineHeight: "18px" }}
            />
            <div className="pxmono9" style={{ display: "flex", justifyContent: "space-between", fontSize: 10, color: "var(--phos-dim)", marginTop: 6 }}>
              <span>{note ?? "~/.toki/memory.md"}</span>
              <span>{bytes > 0 ? `${Math.max(1, Math.round(bytes / 1024))}KB` : "비어 있음"}</span>
            </div>
            <div style={{ display: "flex", gap: 10, marginTop: 14 }}>
              <button className="cf-soft" style={{ flex: 1 }} onClick={onClose}>닫기</button>
              <button className="cf-soft" style={{ flex: 1 }} onClick={pickFile}>파일 불러오기</button>
              <button className="cf-soft primary" style={{ flex: 1 }} disabled={busy} onClick={save}>{busy ? "저장 중" : "저장"}</button>
            </div>
          </div>
        </div>
      </div>
      {/* 함정 1: 셸 조상 체인의 user-select:none이 WKWebView에선 캐럿까지
          죽인다. 입력 섬에서만 되살린다. */}
      <style>{`.tk-mempaste { -webkit-user-select: text; user-select: text; }`}</style>
    </div>
  );
}

function DeepReportModal({ deep, onClose, onRerun }: {
  deep: DeepCoaching; onClose: () => void; onRerun: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const rel = relTime(deep.generated_at);
  const title = `딥 분석 · ${deep.n_days}일 · 프롬프트 ${deep.n_prompts}개${rel ? ` · ${rel}` : ""}`;
  async function doCopy() {
    const ok = await copyToClipboard(deepToText(deep));
    if (ok) { setCopied(true); window.setTimeout(() => setCopied(false), 1500); }
  }
  const drag = useDragBox();
  return (
    <div onClick={() => { if (!drag.didDrag()) onClose(); }} style={{ position: "fixed", inset: 0, zIndex: 220, display: "flex", alignItems: "center", justifyContent: "center", padding: 20, background: "rgba(14,12,18,.55)" }}>
      <div className="tk-report" onClick={(e) => e.stopPropagation()} style={{ width: 560, maxWidth: "100%", maxHeight: "82vh", display: "flex", flexDirection: "column", fontFamily: "var(--pixel)", color: "var(--phos)", ...drag.box }}>
        <div style={{ display: "flex", flexDirection: "column", minHeight: 0, background: "var(--scr)", border: "1.5px solid var(--phos)", boxShadow: "5px 6px 0 rgba(0,0,0,.5)" }}>
          <div {...drag.handle} style={{ position: "relative", display: "flex", alignItems: "center", gap: 6, flex: "0 0 auto", padding: "5px 8px", height: 26, borderBottom: "1.5px solid var(--phos-30)", overflow: "hidden", background: "var(--scr2)", cursor: "grab" }}>
            <div style={{ position: "absolute", inset: 0, pointerEvents: "none", backgroundImage: "repeating-linear-gradient(to bottom, var(--phos-dim) 0 1.5px, transparent 1.5px 4px)" }} />
            <span data-dlg-btn onClick={onClose} style={{ position: "relative", width: 13, height: 11, cursor: "pointer", border: "1.5px solid var(--phos)", background: "var(--scr2)" }} />
            <span style={{ flex: 1 }} />
            <span style={{ position: "relative", background: "var(--scr2)", padding: "0 10px", fontSize: 12, whiteSpace: "nowrap" }}>{title}</span>
            <span style={{ flex: 1 }} />
            <span style={{ position: "relative", width: 13, height: 11, border: "1.5px solid var(--phos)", background: "var(--scr2)" }} />
          </div>
          <div style={{ padding: 18, overflowY: "auto", minHeight: 0 }}>
            <div style={{ fontSize: 12, lineHeight: "18px" }}>{renderMarkdown(deep.body)}</div>
            <div style={{ display: "flex", gap: 10, marginTop: 16 }}>
              <button className="cf-soft" style={{ flex: 1 }} onClick={onClose}>닫기</button>
              <button className="cf-soft" style={{ flex: 1 }} onClick={() => { onClose(); onRerun(); }}>다시 분석 (claude)</button>
              <button className="cf-soft primary" style={{ flex: 1 }} onClick={doCopy}>{copied ? "복사됨" : "복사"}</button>
            </div>
          </div>
        </div>
        <div style={{ textAlign: "right", fontFamily: "var(--pixel-mono-9)", fontSize: 10, color: "var(--phos-dim)", marginTop: 8 }}>©1986 tokisoft</div>
      </div>
    </div>
  );
}

/* ─── shell state machine ────────────────────────────────── */

type Mode = "home" | "menu" | "pomo" | "appearance" | "coaching" | "retro" | "settings" | "info";
// "홈"은 목록에서 뺐다 — depth-entry 규칙상 뒤로 버튼이 홈 복귀를 담당한다.
const MENU: { k: Mode; t: string }[] = [
  { k: "info", t: "정보" },
  { k: "appearance", t: "외형" }, { k: "coaching", t: "코칭" }, { k: "retro", t: "회고" }, { k: "settings", t: "설정" },
];
const pad = (n: number) => (n < 10 ? "0" : "") + n;

// Reconciled level+usage bundle from the `get_level_info` Rust command.
// `to_next`/`xp`/totals are cache_read-scale, so they run into the billions —
// fmtTok carries a B tier the older M-capped formatter lacked.
type LevelInfo = {
  lv: number; at_cap: boolean;
  xp: number; into_level: number; span: number; to_next: number; progress: number;
  total_input: number; total_output: number; total_cache_creation: number; total_cache_read: number;
};
type SkillWindow = {
  sessions: number; edits: number; reworks: number; one_shot: number;
  prompts: number; short_prompts: number; delegation: number; max_chain: number;
};
type SkillTrend = { now: SkillWindow; prev: SkillWindow; computed_at: string };

const fmtTok = (n: number): string => {
  if (n >= 1e9) return (n / 1e9).toFixed(2).replace(/\.?0+$/, "") + "B";
  if (n >= 1e6) return (n / 1e6).toFixed(1).replace(/\.0$/, "") + "M";
  if (n >= 1e3) return (n / 1e3).toFixed(1).replace(/\.0$/, "") + "K";
  return String(n);
};

// Idle chatter — varied state-aware one-liners so the home screen isn't a
// single static string. Rotated on a slow timer; the pool is picked by the
// pet's current mood. (LLM-generated chatter is a documented future upgrade —
// a per-line local-LLM call would be too slow/wasteful for flavor text.)
const CHAT_HUNGRY = [
  "배고파… 밥 줄래?",
  "토큰… 토큰이 먹고 싶어…",
  "기운이 없어… 세션 하나만?",
  "꼬르륵… 밥 버튼 어디 있더라?",
];
const CHAT_FOCUS = [
  "집중 중… 방해 금지!",
  "몰입 모드. 지켜봐줘.",
  "타이머 도는 중, 파이팅!",
  "지금이 제일 잘 되는 시간이야.",
];
const CHAT_IDLE = [
  "깜빡… 커서를 보는 중",
  "오늘도 토큰 냠냠?",
  "심심하다… 뭐 할까?",
  "커서가 어디 가나 지켜보는 중",
  "가끔은 쉬어가도 좋아",
  "…딴생각 중",
  "너 지금 뭐 해?",
  "졸리다… 밥 주면 깰 텐데",
  "좋은 코드 쓰고 있어?",
  "나 여기 있는 거 알지?",
];

type CoachStage = "intro" | "waking" | "loading" | "result" | "error";

type HookStatus = {
  installed: boolean; port: number | null; received_count: number;
  // M4: 배터리 1순위 소스(statusLine) + Codex 훅. codex_available=false면
  // 행을 숨기지 않고 "미감지"로 보여준다 — 조건부 숨김 금지.
  statusline_installed: boolean; codex_available: boolean; codex_installed: boolean;
  /** M9: 훅이 도는 POSIX 셸(Windows 는 Git for Windows)이 있나 */
  shell_ok?: boolean;
  /** M9: 에이전트 CLI 유무 + 없을 때 복사시킬 설치 명령 */
  agents?: { id: string; label: string; cli: string | null; install_cmd: string; install_url: string }[];
};

/** 기본 브라우저로. 셸 안에서도 온보딩과 같은 경로를 쓴다. */
async function openExternal(url: string) {
  try {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url);
  } catch { /* 열기 실패는 조용히 — 링크 하나가 화면을 죽일 이유는 없다 */ }
}

/** 코칭 백엔드 순환 — Rust `coach::BACKEND_CYCLE`과 같은 순서·값. */
const BACKEND_CYCLE = ["auto", "claude", "codex", "ollama"];
function backendLabel(v?: string): string {
  switch (v) {
    case "claude": return "CLAUDE";
    case "codex": return "CODEX";
    case "ollama": return "LOCAL";
    default: return "AUTO";
  }
}
function byteLen(s: string): number {
  return new TextEncoder().encode(s).length;
}

/** 딥 코칭 실행 전 고지 (Rust `deep_coach::DeepPreview`). */
type DeepPreview = {
  backend: string; backend_kind: string; egresses: boolean;
  n_prompts: number; n_days: number;
  sources: { label: string; kind: "memory" | "rules"; path: string; bytes: number }[];
  has_inventory: boolean;
};

type SettingsRow = { id: string; label: string };
/** 설정 최상위. `g:` 는 들어가는 그룹 — 전부 펼쳐 두면 스크롤 목록이 되고, 어떤
 *  게 "상태"고 어떤 게 "설정"인지 안 갈렸다(대교 실기 2차, 2026-09-11). CLI 유무
 *  같은 **상태**는 정보 화면(에이전트)으로 갔다. */
const SETTINGS_TOP: SettingsRow[] = [
  { id: "notify", label: "알림" },
  { id: "autostart", label: "로그인 시 자동 실행" },
  { id: "g:display", label: "표시" },
  { id: "g:coach", label: "코칭·회고" },
  { id: "g:data", label: "데이터" },
];
const SETTINGS_GROUPS: Record<string, { title: string; rows: SettingsRow[] }> = {
  display: { title: "표시", rows: [
    { id: "onTop", label: "최상위 고정" },
    { id: "casing", label: "케이스 색상" },
    { id: "invert", label: "화면 반전" },
  ] },
  coach: { title: "코칭·회고", rows: [
    { id: "coachBackend", label: "기본 AI" },
    { id: "memoryPaste", label: "기억 붙여넣기" },
    { id: "promptsReset", label: "코칭 프롬프트" },
    // 이름은 **사용자가 얻는 것**으로("상태줄"은 뭔지 모른다 — 대교 실기 1차).
    { id: "hooks", label: "Claude 기록 연동" },
    { id: "codexHooks", label: "Codex 기록 연동" },
    { id: "statusline", label: "사용량 연동" },
  ] },
  data: { title: "데이터", rows: [
    { id: "sinceInstall", label: "설치 이후만 집계" },
    { id: "reset", label: "데이터 초기화" },
  ] },
};

function Pill({ on, fg, bg }: { on: boolean; fg: string; bg: string }) {
  return (
    <span style={{ fontFamily: "var(--pixel-mono-9)", fontSize: 10, lineHeight: 1, letterSpacing: "1px",
      padding: "1px 4px", border: `1px solid ${fg}`, background: on ? fg : "transparent", color: on ? bg : fg }}>
      {on ? "ON" : "OFF"}
    </span>
  );
}

/* CRT scan loader — ported from the design's CoachLoading (tk-scan/tk-bar
   keyframes live in tokens.css). */
// Analysis loader — disk-defragmenter style (Toki v4 design, cassette-flows
// CoachLoadingDefrag). Scattered sectors (phos-30) get compacted left→right
// into solid phosphor; the write head (teal) + read source (amber) blink.
// NOTE: the defrag motion is a decorative INDETERMINATE indicator — the LLM
// call is one opaque blocking op with no progress signal, so there's no real
// %. The readout shows real ELAPSED TIME instead of a fake percentage.
function CoachLoadingScreen({ waking = false, tag, onCancel }: { waking?: boolean; tag?: string; onCancel?: () => void }) {
  const COLS = 14, N = COLS * 8, CELL = 7, GAP = 2;
  const frag = useRef<boolean[] | null>(null);
  if (!frag.current) frag.current = Array.from({ length: N }, () => Math.random() < 0.44);
  const [p, setP] = useState(0);
  const [dots, setDots] = useState(1);
  const [elapsed, setElapsed] = useState(0); // real seconds since the wait began
  useEffect(() => {
    const t = window.setInterval(() => setP((v) => {
      if (v >= N) { frag.current = Array.from({ length: N }, () => Math.random() < 0.44); return 0; }
      return v + 1;
    }), 85);
    const d = window.setInterval(() => setDots((x) => (x % 3) + 1), 350);
    const e = window.setInterval(() => setElapsed((s) => s + 1), 1000);
    return () => { window.clearInterval(t); window.clearInterval(d); window.clearInterval(e); };
  }, [N]);
  // read source = the next scattered fragment ahead of the write head
  let readIdx = -1;
  for (let i = p + 1; i < N; i++) { if (frag.current[i]) { readIdx = i; break; } }
  // P3-15: cold first run loads the ~8B model into RAM (10-30s). `waking` says so.
  const statusTag = tag ?? (waking ? "코칭 · 준비" : "코칭 · 분석");
  const btn = onCancel ? "분석 중단" : waking ? "깨우는 중" : "분석 중";
  const line = waking ? "모델 깨우는 중" : "기록 분석 중";
  return (
    <Screen
      status={<><span style={{ color: "var(--phos-dim)" }}>{statusTag}</span><span className="pxmono9" style={{ color: "var(--phos-dim)" }}>{elapsed}초</span></>}
      buttons={[{ label: btn, onClick: onCancel }]}
    >
      <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 8 }}>
        <div style={{ padding: 4, border: "1.5px solid var(--phos-30)", display: "grid", gridTemplateColumns: `repeat(${COLS}, ${CELL}px)`, gridAutoRows: `${CELL}px`, gap: GAP }}>
          {Array.from({ length: N }, (_, i) => {
            let bg = "transparent", bd = "var(--phos-14)", anim = "none";
            if (i < p) { bg = "var(--phos)"; bd = "var(--phos)"; }                                           // 정리됨
            else if (i === p) { bg = "var(--teal)"; bd = "var(--teal)"; anim = "tk-defrag-w .3s steps(2) infinite"; }   // 쓰기 헤드
            else if (i === readIdx) { bg = "var(--amber)"; bd = "var(--amber)"; anim = "tk-defrag-w .3s steps(2) infinite"; } // 읽기
            else if (frag.current![i]) { bg = "var(--phos-30)"; bd = "var(--phos-30)"; }                     // 흩어진 조각
            return <div key={i} style={{ width: CELL, height: CELL, background: bg, boxShadow: "inset 0 0 0 1px " + bd, animation: anim }} />;
          })}
        </div>
        {/* 한 줄만 — 격자 8행 + 라벨이면 168px CRT가 꽉 찬다. 예전엔 "첫 실행은
            10-30초 걸려요" 힌트를 아래 덧붙였는데 버튼바 뒤로 잘려 넘쳤다.
            대기 시간은 상태줄의 경과초(N초)가 이미 정직하게 말해준다. */}
        <div style={{ fontFamily: "var(--pixel-9)", fontSize: 10, lineHeight: "15px", color: "var(--phos-dim)", letterSpacing: "1px" }}>
          {line}{".".repeat(dots)}
        </div>
      </div>
    </Screen>
  );
}

export function TamagotchiShell({
  lv,
  hungry,
  hasCoaching,
  reportClickRects = true,
  initialMode = "home",
}: {
  lv: number;
  hungry: boolean;
  hasCoaching: boolean;
  // When false, the desk owns multi-rect click-through reporting and the
  // shell skips its own single-rect report (they'd otherwise fight).
  reportClickRects?: boolean;
  // Dev-preview only — jump straight to a mode (e.g. `?mode=settings`)
  // instead of clicking through 메뉴 by hand.
  initialMode?: Mode;
}) {
  const [mode, setMode] = useState<Mode>(initialMode);
  const [cursor, setCursor] = useState(0);
  const [now, setNow] = useState(new Date());
  // 집중 길이는 **마지막에 맞춘 값**을 기억한다(대교 실기 2차, 2026-09-11) —
  // 매번 25분으로 돌아오면 ＋/－ 를 매번 눌러야 한다. 휴식은 5분 고정.
  const [pomoFocusFull, setPomoFocusFull] = useState(() => {
    const v = Number(localStorage.getItem("toki-pomo-focus"));
    return Number.isFinite(v) && v >= 60 && v <= 90 * 60 ? v : 25 * 60;
  });
  const rememberFocus = (sec: number) => { setPomoFocusFull(sec); try { localStorage.setItem("toki-pomo-focus", String(sec)); } catch {} };
  const [pomoLeft, setPomoLeft] = useState(() => pomoFocusFull);
  // Full duration of the current phase — the denominator for the 10-dot
  // progress meter. Tracks ＋/－ (session length) and resets on each phase flip.
  const [pomoTotal, setPomoTotal] = useState(() => pomoFocusFull);
  const [pomoRunning, setPomoRunning] = useState(false);
  const [pomoPhase, setPomoPhase] = useState<"FOCUS" | "BREAK">("FOCUS");
  const [pomoDone, setPomoDone] = useState(0);
  const pomoTimer = useRef<number | null>(null);
  const [petPreviewIdx, setPetPreviewIdx] = useState(0);
  const [alert, setAlert] = useState<null | "hunger" | "retro" | "newpet" | "pomo" | "update">(null);
  /* M8: 새 버전 알림. **데스크 얼러트**로 띄운다(사용자 결정 2026-09-04) —
     셸 안 배지면 셸을 열어야 보이는데, 업데이트는 열지 않아도 닿아야 한다.
     확인은 기동 직후 1회 + 하루 1회. **자동 설치는 없다**(spec §9.4). */
  const [update, setUpdate] = useState<{ current: string; latest: string | null } | null>(null);
  const [updating, setUpdating] = useState(false);
  const [updateError, setUpdateError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    const check = () => {
      invoke<{ current: string; latest: string | null; available: boolean }>("update_check")
        .then((u) => {
          if (!alive || !u.available) return;
          setUpdate({ current: u.current, latest: u.latest });
          setAlert((a) => a ?? "update");   // 다른 알림을 밀어내지 않는다
        })
        .catch(() => {});
    };
    // 기동 직후는 잠깐 미룬다 — 창이 자리를 잡기 전에 얼러트가 뜨면 튄다.
    const first = window.setTimeout(check, 8000);
    const daily = window.setInterval(check, 24 * 60 * 60 * 1000);
    return () => { alive = false; window.clearTimeout(first); window.clearInterval(daily); };
  }, []);

  // 디스플레이 픽셀 밀도 — 정보 화면 표시용. main.tsx가 <html data-dpi>를
  // 찍는 것과 같은 소스(devicePixelRatio)라, 여기 값이 곧 보정 적용 여부다.
  const [dpr, setDpr] = useState(() => window.devicePixelRatio || 1);
  useEffect(() => {
    const sync = () => setDpr(window.devicePixelRatio || 1);
    window.addEventListener("resize", sync);
    let mq: MediaQueryList | null = null;
    try {
      mq = matchMedia("(resolution: 1dppx)");
      mq.addEventListener("change", sync);
    } catch { /* 구형 WebKit */ }
    return () => {
      window.removeEventListener("resize", sync);
      mq?.removeEventListener("change", sync);
    };
  }, []);

  // V4-19: user pets from Tokisoft Studio (~/.toki/pets/*.json). Fetched on
  // mount + refreshed on appearance entry so a freshly dropped file shows up
  // without an app restart. Merged after the built-in 12; id collisions with
  // built-ins are ignored (built-in wins).
  const [userPets, setUserPets] = useState<UserPetData[]>([]);
  useEffect(() => {
    if (mode !== "appearance" && userPets.length > 0) return;
    invoke<UserPetData[]>("user_pets").then(setUserPets).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode]);
  const allPets: PetVariant[] = useMemo(
    () => [
      ...PET_VARIANTS,
      ...userPets
        .filter((u) => !PET_VARIANTS.some((v) => v.id === u.id) && u.states?.idle?.frames?.length)
        .map((u) => ({ id: u.id, name: u.name, rows: u.states.idle.frames[0], unlockLv: 1, states: u.states })),
    ],
    [userPets],
  );

  // V4-14: per-character coaching personas — shown in the appearance carousel
  // so picking a pet is also picking a coaching voice. Static data (Rust
  // PERSONAS table is the single source) → fetch once, lazily on first need.
  const [personas, setPersonas] = useState<Record<string, { name: string; desc: string; example: string }> | null>(null);
  useEffect(() => {
    if (mode !== "appearance" || personas) return;
    invoke<{ id: string; name: string; desc: string; example: string }[]>("coach_personas")
      .then((rows) => setPersonas(Object.fromEntries(rows.map((r) => [r.id, r]))))
      .catch(() => {});
  }, [mode, personas]);

  // 실력 추세 — 레벨(누적 토큰=소비)과 별개로 "얼마나 늘었나". Rust가 30분마다
  // 백그라운드로 계산해두므로(계산 자체는 11초) 여기선 캐시만 읽는다.
  const [skill, setSkill] = useState<SkillTrend | null>(null);
  const [infoPage, setInfoPage] = useState(0);
  useEffect(() => {
    if (mode !== "info") return;
    invoke<SkillTrend | null>("get_skill_trend").then(setSkill).catch(() => {});
  }, [mode]);

  // "정보" screen — level progress + cumulative token usage. Fetched fresh on
  // each entry (numbers move as tokens accrue); backend reconciles the level
  // math so the divisor lives in one place.
  const [levelInfo, setLevelInfo] = useState<LevelInfo | null>(null);
  useEffect(() => {
    if (mode !== "info") return;
    let live = true;
    invoke<LevelInfo>("get_level_info")
      .then((r) => { if (live) setLevelInfo(r); })
      .catch(() => {});
    return () => { live = false; };
  }, [mode]);

  // M2: 에이전트별 누적 소비(XP 화폐, 표시용 분해) + Codex 파싱 통계.
  type AgentUsage = {
    agents: { label: string; tokens: number }[];
    codex_parse_ok: number;
    codex_parse_failed: number;
  };
  const [agentUsage, setAgentUsage] = useState<AgentUsage | null>(null);
  useEffect(() => {
    if (mode !== "info") return;
    invoke<AgentUsage>("get_agent_usage").then(setAgentUsage).catch(() => {});
  }, [mode]);

  // P2-7: mirror an in-app alert to an OS notification when the desk isn't
  // focused (hidden/backgrounded) — a visible desk already shows the modal, so
  // firing both would be redundant. Rust gates by notifications_level.
  const osNotify = (title: string, body: string, important: boolean) => {
    if (document.hasFocus()) return;
    invoke("send_alert_notification", { title, body, important }).catch(() => {});
  };

  // Eye-tracking gaze: the pet's EYES look toward the mouse (dx/dy ∈ {-1,0,1}
  // native px, applied by withGaze — the body doesn't move). The cursor comes
  // from Rust's "cursor-pos" event (window-relative CSS/logical px) — NOT webview
  // pointermove, which never fires over the transparent/click-through desk.
  const [gaze, setGaze] = useState({ dx: 0, dy: 0 });
  const petBoxRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    // Rust 는 창마다 `emit_to(label)` 로 **자기 창 좌표**를 쏜다. 그걸 받으려면
    // 리스너도 자기 라벨을 타깃으로 걸어야 한다 — 기본 `listen()`(target: Any)은
    // 라벨 지정 이벤트를 **못 받는다**(tauri manager::filter_target, `_ => false`).
    // 프리뷰(브라우저)엔 창이 없으니 Any 로 둔다.
    let target: { kind: "AnyLabel"; label: string } | undefined;
    try { target = { kind: "AnyLabel", label: getCurrentWindow().label }; } catch { target = undefined; }
    const un = listen<[number, number]>("cursor-pos", (e) => {
      const el = petBoxRef.current;
      if (!el) return; // pet only shows on home
      const r = el.getBoundingClientRect();
      const scx = r.left + r.width / 2; // sprite center, CSS px
      const scy = r.top + r.height / 2;
      const [cx, cy] = e.payload;
      const dead = 6;
      // Horizontal ramps to 2px when the cursor is clearly off to a side.
      const ux = cx - scx;
      const dx = Math.abs(ux) < dead ? 0 : Math.sign(ux) * (Math.abs(ux) < 60 ? 1 : 2);
      // Vertical capped at ±1px (half the horizontal range) — up/down eye
      // motion reads stronger, so keep it gentler.
      const uy = cy - scy;
      const dy = Math.abs(uy) < dead ? 0 : Math.sign(uy);
      setGaze((g) => (g.dx === dx && g.dy === dy ? g : { dx, dy }));
    }, target ? { target } : undefined);
    return () => { un.then((f) => f()).catch(() => {}); };
  }, []);

  // Idle chatter rotation — advance the line every ~7s so home isn't static.
  const [chatTick, setChatTick] = useState(0);
  useEffect(() => {
    const id = window.setInterval(() => setChatTick((t) => t + 1), 7000);
    return () => window.clearInterval(id);
  }, []);

  // v4 appearance/case-color/invert settings (V4-3/V4-7) — backed by
  // app_settings (same get_settings/set_settings the Settings view uses).
  const [settings, setSettingsState] = useState<AppSettings | null>(null);
  useEffect(() => {
    invoke<AppSettings>("get_settings").then(setSettingsState).catch(() => {});
  }, []);

  // P1-1: surface the authoritative 5h usage (V4-4) that was previously
  // computed but never shown. `active_block_tokens / token_limit` = the block
  // utilization ratio for every source (oauth maps five_hour_pct→tokens with
  // limit 100). Backend caches ~10s; poll every 20s. null until first fetch.
  const [usagePct, setUsagePct] = useState<number | null>(null);
  // 배터리가 어느 에이전트 쿼터를 가리키는지 — 툴팁에만 쓴다.
  const [usageSource, setUsageSource] = useState<string | undefined>(undefined);
  const [usageAgents, setUsageAgents] = useState<AgentQuota[]>([]);
  useEffect(() => {
    let alive = true;
    const pull = () => {
      invoke<{ active_block_tokens: number; token_limit: number; source?: string; agents?: AgentQuota[] } | null>("get_usage_snapshot")
        .then((s) => {
          if (!alive) return;
          setUsagePct(s && s.token_limit > 0
            ? Math.max(0, Math.min(100, (s.active_block_tokens / s.token_limit) * 100))
            : null);
          setUsageSource(s?.source);
          setUsageAgents(s?.agents ?? []);
        })
        .catch(() => {});
    };
    pull();
    const id = window.setInterval(pull, 20_000);
    return () => { alive = false; window.clearInterval(id); };
  }, []);
  function patchSettings(patch: Partial<AppSettings>) {
    setSettingsState((prev) => {
      if (!prev) return prev;
      const next = { ...prev, ...patch };
      invoke("set_settings", { settings: next }).catch(() => {});
      return next;
    });
  }
  // 유저 펫 파일이 삭제되면 find가 실패하고 동글이로 폴백된다.
  const activeVariant = allPets.find((v) => v.id === settings?.active_character) ?? PET_VARIANTS[0];

  // V5.1: 코칭 = 딥 분석 단일 흐름 (사용자 결정 2026-08-31). 로컬 크로스세션
  // 티어의 UI 오케스트레이션(Ollama 웜업 폴, 신호 카드, coaching_cross_*)은
  // 제거 — 백엔드는 남아 있어 되살릴 수 있다. 쿼터를 소비하므로(claude -p)
  // 자동 실행 절대 없음: 캐시 로드 or 명시적 "분석" 버튼만. 결과는
  // deep != null로 존재, deepStage는 흐름(loading/error)만 담당. 취소는
  // 회고와 같은 run-token 방식(서브프로세스는 못 죽이지만 UI는 복귀).
  // v5 M5: 붙여넣은 기억(~/.toki/memory.md)과 프롬프트 오버라이드 되돌리기.
  const [memoryPasteOpen, setMemoryPasteOpen] = useState(false);
  const [memoryPasteText, setMemoryPasteText] = useState("");
  const [memoryPasteBytes, setMemoryPasteBytes] = useState(0);
  const [promptsResetNote, setPromptsResetNote] = useState<string | null>(null);
  function openMemoryPaste() {
    invoke<string>("memory_paste_get")
      .then((t) => { setMemoryPasteText(t); setMemoryPasteOpen(true); })
      .catch(() => { setMemoryPasteText(""); setMemoryPasteOpen(true); });
  }
  useEffect(() => {
    invoke<string>("memory_paste_get").then((t) => setMemoryPasteBytes(byteLen(t))).catch(() => {});
  }, []);

  const [deepStage, setDeepStage] = useState<"idle" | "loading" | "error">("idle");
  const [deep, setDeep] = useState<DeepCoaching | null>(null);
  const [deepError, setDeepError] = useState<string | null>(null);
  const [deepReportOpen, setDeepReportOpen] = useState(false);
  const deepRun = useRef(0);
  function runDeepCoaching() {
    const myRun = ++deepRun.current;
    setDeepStage("loading");
    setDeepError(null);
    invoke<DeepCoaching>("coaching_deep_generate")
      .then((d) => {
        if (deepRun.current !== myRun) return;
        setDeep(d); setDeepStage("idle"); setDeepReportOpen(true);
      })
      .catch((e) => {
        if (deepRun.current !== myRun) return;
        setDeepError(String(e)); setDeepStage("error");
      });
  }
  function cancelDeep() { ++deepRun.current; setDeepStage("idle"); }
  // 재진입 시 캐시 즉시 로드 (쿼터 0) — 있으면 결과 화면, 없으면 intro.
  // 같이 실행 전 고지(spec §9.2)도 받아둔다. 둘 다 LLM 호출 0.
  const [deepPreview, setDeepPreview] = useState<DeepPreview | null>(null);
  const [egressOpen, setEgressOpen] = useState(false);
  useEffect(() => {
    if (mode !== "coaching") return;
    invoke<DeepCoaching | null>("coaching_deep_latest")
      .then((d) => { if (d) setDeep(d); })
      .catch(() => {});
    invoke<DeepPreview>("coaching_deep_preview")
      .then(setDeepPreview)
      .catch(() => setDeepPreview(null));
  }, [mode]);

  // A/V4-14: 회고 메뉴. retroStage "intro" = 프로젝트 PICKER (세션을 프로젝트로
  // 그룹). 프로젝트 고르면 → 캐시 있으면 즉시 표시("N분 전"), 없으면 병합 세션
  // 회고 생성. 코칭처럼 캐시(`retro_project_cached`)로 재진입 즉시 로드.
  const [retroStage, setRetroStage] = useState<CoachStage>("intro");
  const [retroBody, setRetroBody] = useState<string | null>(null);
  const [retroGeneratedAt, setRetroGeneratedAt] = useState<string | null>(null);
  const [retroError, setRetroError] = useState<string | null>(null);
  const [retroReportOpen, setRetroReportOpen] = useState(false);
  const [retroProjects, setRetroProjects] = useState<ProjectInfo[]>([]);
  const [retroCursor, setRetroCursor] = useState(0);
  const [retroPickedDir, setRetroPickedDir] = useState<string | null>(null);
  // Run token — bumped on every start/cancel so a stale in-flight result (or the
  // warmth poll) can't apply after the user cancelled or started another run.
  const retroRun = useRef(0);
  function runProjectRetro(dir: string) {
    const myRun = ++retroRun.current;
    setRetroPickedDir(dir);
    setRetroStage("loading");
    setRetroError(null);
    const applyWarm = (loaded: boolean) => { if (myRun === retroRun.current) setRetroStage(loaded ? "loading" : "waking"); };
    invoke<boolean>("coaching_ollama_loaded").then(applyWarm).catch(() => {});
    const poll = window.setInterval(() => {
      if (myRun !== retroRun.current) { window.clearInterval(poll); return; }
      invoke<boolean>("coaching_ollama_loaded")
        .then((loaded) => { if (myRun === retroRun.current && loaded) setRetroStage("loading"); })
        .catch(() => {});
    }, 2000);
    invoke<string>("retro_project_generate", { dir })
      .then((b) => { if (myRun !== retroRun.current) return; window.clearInterval(poll); setRetroBody(b); setRetroGeneratedAt(new Date().toISOString()); setRetroStage("result"); })
      .catch((e) => { if (myRun !== retroRun.current) return; window.clearInterval(poll); setRetroError(String(e)); setRetroStage("error"); });
  }
  // Pick a project: show its cached retro instantly if present, else generate.
  function pickProject(dir: string) {
    setRetroPickedDir(dir);
    invoke<{ body: string; generated_at: string } | null>("retro_project_cached", { dir })
      .then((c) => {
        if (c) { setRetroBody(c.body); setRetroGeneratedAt(c.generated_at); setRetroStage("result"); }
        else runProjectRetro(dir);
      })
      .catch(() => runProjectRetro(dir));
  }
  // Cancel an in-flight analysis → back to the picker. The Rust generate keeps
  // running in the background but its result is dropped (run token stale).
  const cancelRetro = () => { retroRun.current++; setRetroStage("intro"); };
  const rerunRetro = () => (retroPickedDir ? runProjectRetro(retroPickedDir) : setRetroStage("intro"));
  useEffect(() => {
    if (mode !== "retro") return;
    setRetroError(null);
    setRetroCursor(0);
    setRetroStage("intro");
    invoke<ProjectInfo[]>("retro_project_list").then(setRetroProjects).catch(() => setRetroProjects([]));
  }, [mode]);

  // V4-9: settings-in-CRT (cursor list, ▲▼변경 — same pattern as MENU).
  const [settingsCursor, setSettingsCursor] = useState(0);
  const [settingsGroup, setSettingsGroup] = useState<string | null>(null);
  const [settingsTopCursor, setSettingsTopCursor] = useState(0);
  const [hook, setHook] = useState<HookStatus | null>(null);
  const [resetArmed, setResetArmed] = useState(false);
  useEffect(() => {
    if (mode !== "settings") return;
    setSettingsCursor(0);
    setResetArmed(false);
    const poll = () => invoke<HookStatus>("hooks_status").then(setHook).catch(() => {});
    poll();
    const id = window.setInterval(poll, 2000);
    return () => window.clearInterval(id);
  }, [mode]);
  // 인스톨러 3종(Claude 훅·Claude 상태줄·Codex 훅)은 전부 같은 모양:
  // 커맨드 하나 부르고 hooks_status를 다시 읽는다.
  async function runInstaller(cmd: string) {
    try {
      await invoke(cmd);
      setHook(await invoke<HookStatus>("hooks_status"));
    } catch (e) {
      console.error(e);
    }
  }
  const toggleHooks = (install: boolean) => runInstaller(install ? "hooks_install" : "hooks_uninstall");
  function settingsRowRight(id: string): { kind: "pill" | "cycle" | "text"; on?: boolean; text?: string } {
    switch (id) {
      case "notify": return { kind: "cycle", text: (settings?.notifications_level ?? "impt").toUpperCase() };
      case "autostart": return { kind: "pill", on: !!settings?.autostart_enabled };
      case "sinceInstall": return { kind: "pill", on: !!settings?.track_since_install };
      // Windows 에 Git for Windows 가 없으면 훅이 아예 못 돈다 — 켜기 전에 말한다.
      case "hooks": return hook?.shell_ok === false ? { kind: "text", text: "Git 필요" } : { kind: "pill", on: !!hook?.installed };
      case "g:display": case "g:coach": case "g:data": return { kind: "text", text: "▸" };
      case "statusline": return { kind: "pill", on: !!hook?.statusline_installed };
      case "codexHooks": return hook?.codex_available
        ? { kind: "pill", on: !!hook.codex_installed }
        : { kind: "text", text: "미감지" };
      case "casing": return { kind: "cycle", text: (settings?.case_color ?? "beige").toUpperCase() };
      case "invert": return { kind: "pill", on: !!settings?.invert_screen };
      case "onTop": return { kind: "pill", on: settings?.always_on_top ?? true };
      case "coachBackend": return { kind: "cycle", text: backendLabel(settings?.coach_backend) };
      case "memoryPaste": return { kind: "text", text: memoryPasteBytes > 0 ? `${Math.max(1, Math.round(memoryPasteBytes / 1024))}KB` : "비어 있음" };
      case "promptsReset": return { kind: "text", text: promptsResetNote ?? "되돌리기" };
      case "reset": return { kind: "text", text: resetArmed ? "정말?" : "확인" };
      default: return { kind: "text", text: "" };
    }
  }
  function changeSettingsRow(id: string) {
    if (id === "notify") {
      const order = ["off", "impt", "all"];
      const i = order.indexOf(settings?.notifications_level ?? "impt");
      patchSettings({ notifications_level: order[(i + 1) % order.length] });
    } else if (id === "autostart") {
      patchSettings({ autostart_enabled: !settings?.autostart_enabled });
    } else if (id === "sinceInstall") {
      const v = !settings?.track_since_install;
      patchSettings({ track_since_install: v, install_baseline_cache_read: v ? (settings?.install_baseline_cache_read ?? 0) : 0 });
    } else if (id === "hooks") {
      if (hook?.shell_ok === false) openExternal("https://git-scm.com/downloads/win");
      else toggleHooks(!hook?.installed);
    } else if (id.startsWith("g:")) {
      setSettingsTopCursor(settingsCursor);
      setSettingsGroup(id.slice(2));
      setSettingsCursor(0);
    } else if (id === "statusline") {
      runInstaller(hook?.statusline_installed ? "statusline_uninstall" : "statusline_install");
    } else if (id === "codexHooks") {
      if (hook?.codex_available) runInstaller(hook.codex_installed ? "codex_hooks_uninstall" : "codex_hooks_install");
    } else if (id === "casing") {
      const keys = Object.keys(CASES);
      const i = keys.indexOf(settings?.case_color ?? "beige");
      patchSettings({ case_color: keys[(i + 1) % keys.length] });
    } else if (id === "invert") {
      patchSettings({ invert_screen: !settings?.invert_screen });
    } else if (id === "onTop") {
      patchSettings({ always_on_top: !(settings?.always_on_top ?? true) });
    } else if (id === "coachBackend") {
      const i = BACKEND_CYCLE.indexOf(settings?.coach_backend ?? "auto");
      patchSettings({ coach_backend: BACKEND_CYCLE[(i + 1) % BACKEND_CYCLE.length] });
    } else if (id === "memoryPaste") {
      openMemoryPaste();
    } else if (id === "promptsReset") {
      // 로컬 오버라이드(~/.toki/prompts/*.md)를 지워 코드 기본값으로 되돌린다.
      invoke<number>("prompts_reset")
        .then((n) => { setPromptsResetNote(n > 0 ? `${n}개 삭제` : "이미 기본값"); })
        .catch((e) => setPromptsResetNote(String(e).slice(0, 14)));
    } else if (id === "reset") {
      if (resetArmed) { invoke("reset_character").catch(() => {}); setResetArmed(false); }
      else setResetArmed(true);
    }
  }

  useEffect(() => {
    const id = window.setInterval(() => setNow(new Date()), 1000);
    return () => window.clearInterval(id);
  }, []);

  // 새 회고 도착 → 회고 얼러트 (rising edge only — 스팸 방지).
  const prevHasCoaching = useRef(hasCoaching);
  useEffect(() => {
    if (hasCoaching && !prevHasCoaching.current) {
      setAlert("retro");
      osNotify("코칭이 준비됐어요", "최근 세션 분석이 새로 나왔어요.", true);
    }
    prevHasCoaching.current = hasCoaching;
  }, [hasCoaching]);

  // V4-5: 레벨업으로 새 캐릭터가 해금되면 "새 친구 도착" 얼러트 — 세션 중 lv가
  // 실제로 오를 때만.
  // 버그(픽스): prevUnlocked를 마운트 시점에 초기화했는데, 그때 lv는 아직
  // 기본값(App의 `save?.lv ?? 1`)이라 1이다. 저장된 실제 lv(예: 98)가 ~200ms
  // 뒤 로드되며 해금 수가 점프 → "레벨업"으로 오인해 **매 실행마다 문어 알림**이
  // 떴다. 하이드레이션 점프(마운트 직후 1회)는 무시하고, settle 창 이후의 진짜
  // 상승만 발화한다. 실제 레벨업은 토큰 누적이 필요해 수 분 뒤라 창과 안 겹침.
  const unlockedCount = unlockedCountFor(lv);
  const prevUnlocked = useRef(unlockedCount);
  const newpetArmed = useRef(false);
  useEffect(() => {
    const t = window.setTimeout(() => { newpetArmed.current = true; }, 3000);
    return () => window.clearTimeout(t);
  }, []);
  useEffect(() => {
    if (newpetArmed.current && unlockedCount > prevUnlocked.current && alert === null) {
      setAlert("newpet");
      osNotify("새 친구가 도착했어요", "새 외형이 해금됐어요.", true);
    }
    prevUnlocked.current = unlockedCount;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [unlockedCount]);

  // 배고픔 전이(포만→배고픔) → 배고픔 얼러트. rising edge만 트리거해서
  // hungry가 계속 true여도 매 렌더/폴링마다 재발화하지 않는다.
  const prevHungry = useRef(hungry);
  useEffect(() => {
    if (hungry && !prevHungry.current && alert === null) {
      setAlert("hunger");
      osNotify("토키가 배고파요", "5시간 사용량이 찼어요 — 밥 주러 갈까요?", true);
    }
    prevHungry.current = hungry;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hungry]);

  // 외형설정 진입 시 캐러셀 커서를 현재 active_character 위치로 맞춘다.
  useEffect(() => {
    if (mode === "appearance") {
      const idx = PET_VARIANTS.findIndex((v) => v.id === (settings?.active_character ?? DEFAULT_PET));
      setPetPreviewIdx(idx >= 0 ? idx : 0);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode]);

  // v4 forward click-through: report the shell's window-relative rect so the
  // transparent margin (shadow / desk) passes clicks through to apps behind.
  const shellRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!reportClickRects) return; // desk owns reporting in desk mode
    const report = () => {
      const el = shellRef.current;
      if (!el) return;
      const r = el.getBoundingClientRect();
      // Report in CSS/logical px; Rust converts cursor+window to the same
      // space (cursor ÷ primary_scale, window ÷ its own scale) so mixed-DPR
      // multi-monitor setups compare correctly.
      invoke("set_click_rects", {
        rects: [[r.left, r.top, r.width, r.height]],
      }).catch(() => {});
    };
    report();
    const id = window.setInterval(report, 500);
    window.addEventListener("resize", report);
    return () => { window.clearInterval(id); window.removeEventListener("resize", report); };
  }, [reportClickRects]);

  // 틱은 **숫자만 줄인다.** 그 외의 일은 하지 않는다.
  useEffect(() => {
    if (!pomoRunning) { if (pomoTimer.current) { window.clearInterval(pomoTimer.current); pomoTimer.current = null; } return; }
    pomoTimer.current = window.setInterval(() => setPomoLeft((left) => (left > 0 ? left - 1 : 0)), 1000);
    return () => { if (pomoTimer.current) { window.clearInterval(pomoTimer.current); pomoTimer.current = null; } };
  }, [pomoRunning]);

  // 만료 처리는 **별도 effect**다.
  //
  // 예전엔 `setPomoLeft` 의 updater 안에서 `setPomoPhase` 를 불렀다. updater 는
  // 순수해야 하고 React 는 그걸 두 번 호출할 수 있다 — 실제로 StrictMode 에서
  // 두 번 돌아 phase 가 FOCUS→BREAK→FOCUS 로 **되돌아갔고**, 집중이 끝났는데
  // "휴식 끝! 다시 집중해볼까요?" 팝업이 떴다(2026-09-07 실측).
  //
  // Phase 는 여기서 넘기고 **일시정지**한다 — 다음 판은 사람이 시작한다.
  useEffect(() => {
    if (!pomoRunning || pomoLeft > 0) return;
    const focusEnded = pomoPhase === "FOCUS";
    const nextFull = focusEnded ? 5 * 60 : pomoFocusFull;
    setPomoRunning(false);
    setPomoPhase(focusEnded ? "BREAK" : "FOCUS");
    setPomoTotal(nextFull);
    setPomoLeft(nextFull);
    if (focusEnded) setPomoDone((d) => d + 1);
    setAlert("pomo");
    // 사용자가 직접 건 타이머라 **중요** 알림이다 — false 면 기본 단계(impt)에서
    // 토스트가 안 떠 "팝업만 뜬다"가 됐다(대교 실기, 2026-09-11).
    osNotify("집중", focusEnded ? "집중 끝! 5분 쉬어가요." : "휴식 끝 — 다시 집중?", true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pomoLeft, pomoRunning, pomoPhase, pomoFocusFull]);

  // 상태 표정(idle/hungry/focus)은 `hungryRows`·`focusRows`를 가진 펫만 구분한다.
  // 나머지는 실루엣 변주만 있는 정적 외형(design-brief §8) — 상태 무관하게 그대로.
  // 예전엔 id === "dong" 하드코딩이었는데, 기본 펫을 바꾸는 순간 표정이 통째로
  // 사라졌다(2026-09-03). 어느 펫이 기본이든 표정을 가질 수 있어야 한다.
  const isFocus = pomoRunning && pomoPhase === "FOCUS";

  // V4-19 frame player — Studio 펫(states 보유)만 애니메이션. 상태 키는 dong과
  // 같은 우선순위(focus > hungry > idle), 없는 상태는 idle로 폴백. 프레임
  // 배열 참조가 안정적(state에서 옴)이라 detectEyes의 per-rows 메모가 유효.
  const petStateKey = isFocus ? "focus" : hungry ? "hungry" : "idle";
  const activeAnim = activeVariant.states
    ? activeVariant.states[petStateKey] ?? activeVariant.states.idle
    : null;
  const [frameTick, setFrameTick] = useState(0);
  const animFrames = activeAnim?.frames.length ?? 0;
  useEffect(() => {
    setFrameTick(0);
    if (!activeAnim || animFrames <= 1) return;
    const iv = window.setInterval(
      () => setFrameTick((t) => t + 1),
      Math.max(80, 1000 / (activeAnim.fps || 3)),
    );
    return () => window.clearInterval(iv);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeVariant.id, petStateKey, animFrames]);

  const baseRows = activeAnim
    ? activeAnim.frames[frameTick % activeAnim.frames.length]
    : (isFocus && activeVariant.focusRows) || (hungry && activeVariant.hungryRows) || activeVariant.rows;
  // Eyes track the cursor for whatever pet is shown (detected per sprite),
  // except while the default pet is focusing (집중 중엔 화면만 응시).
  const petRows = isFocus && activeVariant.focusRows
    ? baseRows
    : withGaze(baseRows, detectEyes(baseRows), gaze.dx, gaze.dy);
  // 팝업 스프라이트도 **지금 쓰는 펫**이어야 한다. 예전엔 기본 펫(동글이)
  // 스프라이트를 하드코딩해서, 깡총이나 Studio 펫을 쓰는 사람에게는 셸 안의
  // 펫과 팝업 속 펫이 서로 다른 생물이었다(2026-09-07 지적).
  // 애니메이션 프레임(activeAnim)이 아니라 정지 프레임을 쓴다 — 팝업은 한 컷이다.
  const alertRows = (k: "idle" | "hungry" | "focus") =>
    (k === "focus" && activeVariant.focusRows) ||
    (k === "hungry" && activeVariant.hungryRows) ||
    activeVariant.rows;
  // 집중이 끝났으면 쉬는 얼굴(idle), 휴식이 끝났으면 집중 얼굴(focus).
  const pomoAlertRows = alertRows(pomoPhase === "BREAK" ? "idle" : "focus");

  const hhmm = `${pad(now.getHours())}:${pad(now.getMinutes())}`;

  /* ── mode renders ── */
  let screen: React.ReactNode;
  if (mode === "home") {
    screen = (
      <Screen
        status={<><span>Lv.{lv}</span><span style={{ display: "flex", alignItems: "center", gap: 8 }}><Battery pct={usagePct} source={usageSource} agents={usageAgents} /><span className="pxmono9">{hhmm}</span></span></>}
        buttons={[
          { label: "메뉴", onClick: () => { setMode("menu"); setCursor(0); } },
          { label: "밥", primary: true, onClick: () => invoke("open_agent").catch(() => {}) },
          { label: "집중", onClick: () => setMode("pomo") },
        ]}
      >
        <div style={{ display: "flex", alignItems: "center", justifyContent: "center" }}>
          <div ref={petBoxRef} style={{ display: "flex" }}>
            <PetSprite rows={petRows} scale={3} />
          </div>
        </div>
        <div style={{ textAlign: "center", fontFamily: "var(--pixel-9)", fontSize: 10, color: "var(--phos-dim)" }}>
          {(() => {
            const pool = hungry ? CHAT_HUNGRY : (pomoRunning && pomoPhase === "FOCUS") ? CHAT_FOCUS : CHAT_IDLE;
            return pool[chatTick % pool.length];
          })()}
        </div>
      </Screen>
    );
  } else if (mode === "menu") {
    screen = (
      <Screen
        status={<><span style={{ color: "var(--phos-dim)", letterSpacing: "1px" }}>MENU</span><span className="pxmono9" style={{ color: "var(--phos-dim)" }}>{cursor + 1}/{MENU.length}</span></>}
        buttons={[
          { label: "뒤로", onClick: () => setMode("home") },
          { label: "▲", onClick: () => setCursor((c) => (c - 1 + MENU.length) % MENU.length) },
          { label: "▼", onClick: () => setCursor((c) => (c + 1) % MENU.length) },
          { label: "선택", primary: true, onClick: () => setMode(MENU[cursor].k) },
        ]}
      >
        <div style={{ display: "flex", flexDirection: "column", gap: 2, alignSelf: "stretch" }}>
          {MENU.map((m, i) => {
            const on = i === cursor;
            return (
              <div key={m.t} style={{ display: "flex", alignItems: "center", gap: 6, fontFamily: "var(--pixel-9)", fontSize: 10, lineHeight: 1, padding: "2px 4px", background: on ? "var(--phos)" : "transparent", color: on ? "var(--scr)" : "var(--phos)" }}>
                <span style={{ width: 7 }}>{on ? "▸" : ""}</span><span>{m.t}</span>
              </div>
            );
          })}
        </div>
      </Screen>
    );
  } else if (mode === "pomo") {
    const m = Math.floor(pomoLeft / 60), s = pomoLeft % 60;
    // 10-dot meter = current phase progress (elapsed / full duration), not the
    // completed-session count. Clamp so ＋/－ overshoot can't over/underfill.
    const DOTS = 10;
    const filled = pomoTotal > 0 ? Math.max(0, Math.min(DOTS, Math.round((1 - pomoLeft / pomoTotal) * DOTS))) : 0;
    // ＋/－ set the session length: nudge remaining AND the progress denominator.
    const nudge = (delta: number) => setPomoLeft((v) => {
      const nv = Math.max(60, Math.min(90 * 60, v + delta));
      setPomoTotal(nv);
      if (pomoPhase === "FOCUS") rememberFocus(nv);
      return nv;
    });
    // Reset the CURRENT phase timer to full and pause. Snap to a whole minute
    // (초 단위 nudge로 오염된 pomoTotal이 24:49처럼 되던 것 방지) — progress
    // denominator도 같이 정규화. Phase·회차(누적 성과)는 유지.
    const resetPhase = () => {
      const full = Math.max(60, Math.round(pomoTotal / 60) * 60);
      setPomoRunning(false); setPomoTotal(full); setPomoLeft(full);
    };
    // Skip the current phase → flip to the next and pause. 회차(pomoDone)는 올리지
    // 않는다 — 완주가 아니라 중도 건너뛰기라 성과로 세면 부정직. (타이머 만료는
    // 완주라 별도로 +1.) 다음 phase는 기본 길이로.
    const skipPhase = () => {
      const focusEnded = pomoPhase === "FOCUS";
      const nextFull = focusEnded ? 5 * 60 : pomoFocusFull;
      setPomoPhase(focusEnded ? "BREAK" : "FOCUS");
      setPomoTotal(nextFull); setPomoLeft(nextFull); setPomoRunning(false);
    };
    // "건드린" 타이머: 돌고 있거나(일시정지 포함) 시간이 줄어든 상태. fresh면
    // －/＋(시간 조절), dirty면 리셋/스킵으로 가운데 두 슬롯을 바꾼다. phase flip
    // 직후는 pomoLeft==pomoTotal이라 자동으로 fresh로 떨어짐.
    const pomoDirty = pomoRunning || pomoLeft < pomoTotal;
    screen = (
      <Screen
        status={<><span style={{ background: "var(--phos)", color: "var(--scr)", padding: "2px 6px", letterSpacing: "1px" }}>{pomoPhase}</span><span className="pxmono9">{pomoDone}회</span></>}
        buttons={[
          { label: "뒤로", onClick: () => setMode("home") },
          pomoDirty
            ? { label: "리셋", onClick: resetPhase }
            : { label: "－", onClick: () => nudge(-60) },
          pomoDirty
            ? { label: "스킵", onClick: skipPhase }
            : { label: "＋", onClick: () => nudge(60) },
          // 시작하면 **홈으로 나온다.** 타이머 화면에 남으면 정작 펫이 안 보여서
          // "펫이 같이 화면을 본다"는 기능이 성립하지 않는다(2026-09-07 실사용 지적).
          // 정지는 이 화면에 남는다 — 방금 멈춘 값을 보고 조절하려는 것이므로.
          { label: pomoRunning ? "정지" : "시작", primary: true,
            onClick: () => { if (!pomoRunning) { setPomoRunning(true); setMode("home"); } else setPomoRunning(false); } },
        ]}
      >
        <div style={{ textAlign: "center" }}>
          <div className="pxmono" style={{ fontSize: 48, lineHeight: 1, color: "var(--phos)", marginBottom: 12 }}>{pad(m)}:{pad(s)}</div>
          <div style={{ display: "flex", gap: 4, justifyContent: "center" }}>
            {Array.from({ length: DOTS }).map((_, i) => (
              <span key={i} style={{ width: 6, height: 6, borderRadius: "50%", background: i < filled ? "var(--phos)" : "transparent", boxShadow: "inset 0 0 0 1px var(--phos)" }} />
            ))}
          </div>
        </div>
      </Screen>
    );
  } else if (mode === "appearance") {
    const preview = allPets[petPreviewIdx % allPets.length];
    const unlocked = isUnlocked(preview, lv); // 유저 펫은 unlockLv 1 = 항상 해금
    screen = (
      <Screen
        status={<><span>외형</span><span className="pxmono9" style={{ color: "var(--phos-dim)" }}>{(petPreviewIdx % allPets.length) + 1}/{allPets.length}</span></>}
        buttons={[
          { label: "뒤로", onClick: () => setMode("home") },
          { label: "◀", onClick: () => setPetPreviewIdx((i) => (i - 1 + allPets.length) % allPets.length) },
          { label: "▶", onClick: () => setPetPreviewIdx((i) => (i + 1) % allPets.length) },
          // Locked variant: 결정 disabled — keep it on the carousel but don't
          // let it be selected.
          unlocked
            ? { label: "결정", primary: true, onClick: () => { patchSettings({ active_character: preview.id }); setMode("home"); } }
            : { label: "잠김" },
        ]}
      >
        <div style={{ display: "flex", alignItems: "center", justifyContent: "center", gap: 8 }}>
          <span style={{ fontFamily: "var(--pixel-9)", fontSize: 10, color: "var(--phos-dim)" }}>◂</span>
          <div style={{ width: 60, display: "flex", justifyContent: "center", opacity: unlocked ? 1 : 0.5 }}>
            <PetSprite rows={unlocked ? preview.rows : P_LOCKED} scale={3} />
          </div>
          <span style={{ fontFamily: "var(--pixel-9)", fontSize: 10, color: "var(--phos-dim)" }}>▸</span>
        </div>
        <div style={{ textAlign: "center", fontFamily: "var(--pixel-9)", fontSize: 10, color: "var(--phos)", marginTop: 4 }}>
          {unlocked ? preview.name : `??? · Lv.${preview.unlockLv}에 해금`}
        </div>
        {/* V4-14: coaching-voice preview — what this pet sounds like as a coach.
            Fixed 2-line box (descs are normalized to ~40-46 chars = 2 lines);
            locked pets keep their mystery but the space stays reserved so the
            carousel doesn't jump between pets. */}
        <div style={{ textAlign: "center", fontFamily: "var(--pixel-9)", fontSize: 10, lineHeight: "15px", height: 30, overflow: "hidden", color: "var(--phos-dim)", marginTop: 3, padding: "0 6px" }}>
          {unlocked
            ? personas?.[preview.id]?.desc ?? (preview.states ? "스튜디오에서 만든 펫이에요. 코칭은 기본 톤으로 말해요." : "")
            : ""}
        </div>
      </Screen>
    );
  } else if (mode === "coaching" && deepStage === "loading") {
    // V5.1 코칭 = 딥 분석. 모델이 30일 히스토리+기억+인벤토리를 읽으므로
    // 1~3분 걸린다 (실측 57~122s). 태그에 실제 백엔드를 적는다.
    screen = <CoachLoadingScreen tag={`코칭 · 분석 (${backendLabel(deepPreview?.backend_kind).toLowerCase()})`} onCancel={cancelDeep} />;
  } else if (mode === "coaching" && deepStage === "error") {
    screen = (
      <Screen
        status={<><span style={{ color: "var(--phos-dim)" }}>코칭 · 오류</span></>}
        buttons={[{ label: "뒤로", onClick: cancelDeep }, { label: "다시", primary: true, onClick: runDeepCoaching }]}
      >
        <div className="pxmono9" style={{ fontSize: 10, lineHeight: "15px", color: "var(--phos)" }}>
          <div style={{ marginBottom: 4 }}>분석을 만들지 못했어요.</div>
          <div style={{ color: "var(--phos-dim)", overflowWrap: "anywhere" }}>{deepError}</div>
        </div>
      </Screen>
    );
  } else if (mode === "coaching" && !deep) {
    // intro — 캐시가 없을 때만. 쿼터를 쓰는 유일한 진입이 이 "분석" 버튼이라
    // 비용(claude·쿼터)을 여기 명시한다.
    screen = (
      <Screen
        status={<><span style={{ color: "var(--phos-dim)" }}>코칭</span><span className="pxmono9" style={{ color: "var(--phos-dim)" }}>30일</span></>}
        buttons={[
          { label: "뒤로", onClick: () => setMode("home") },
          { label: "고지", onClick: () => setEgressOpen(true) },
          { label: "분석", primary: true, onClick: runDeepCoaching },
        ]}
      >
        {/* 코칭·회고 화면엔 펫을 세우지 않는다(2026-09-03 결정). 여기서 말하는
            주체는 펫이 아니라 분석이고, 펫을 세우면 어느 펫이 기본이냐에 따라
            같은 화면이 달라 보인다. 대신 본문이 폭을 다 쓴다.
            CRT는 두 줄이 한계라 §9.2 "나가는 것" 목록은 '고지' 팝업이 진다. */}
        <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
          <div style={{ fontFamily: "var(--pixel-9)", fontSize: 10, lineHeight: "15px", color: "var(--phos)" }}>
            최근 {deepPreview?.n_days ?? 30}일 기록을<br />딥 분석해 드릴까요?
            <div style={{ marginTop: 4, color: "var(--phos-dim)" }}>
              {deepPreview?.egresses
                ? <>{backendLabel(deepPreview?.backend_kind).toLowerCase()}로 돌아요<br />(쿼터 조금 소비)</>
                : <>로컬 모델로 돌아요<br />(밖으로 안 나가요)</>}
            </div>
          </div>
        </div>
      </Screen>
    );
  } else if (mode === "coaching" && deep) {
    // result — 캐시 or 방금 생성. "다시"만 쿼터를 쓴다. (deep 없는 경우는 위
    // intro 분기가 이미 소진 — && deep은 TS 내로잉용.)
    const rel = relTime(deep.generated_at);
    screen = (
      <Screen
        status={<><span style={{ color: "var(--phos-dim)" }}>코칭 · 결과</span><span className="pxmono9" style={{ color: "var(--phos-dim)" }}>{rel || "완료"}</span></>}
        buttons={[
          { label: "뒤로", onClick: () => setMode("home") },
          { label: "다시", onClick: runDeepCoaching },
          { label: "자세히", primary: true, onClick: () => setDeepReportOpen(true) },
        ]}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <div style={{ fontFamily: "var(--pixel-9)", fontSize: 10, lineHeight: "15px", color: "var(--phos)", whiteSpace: "nowrap" }}>
            <div>최근 {deep.n_days}일 기록</div>
            <div>프롬프트 {deep.n_prompts}개 분석</div>
            <div style={{ color: "var(--phos-dim)" }}>{rel ? `${rel} 분석` : "분석 준비됨"}</div>
            <div style={{ color: "var(--phos-dim)" }}>'자세히'로 보기</div>
          </div>
        </div>
      </Screen>
    );
  } else if (mode === "retro" && retroStage === "intro") {
    // Project picker — recent projects (sessions merged per project). ▲▼ 선택.
    const VIS = 5;
    const start = Math.max(0, Math.min(retroCursor - 2, Math.max(0, retroProjects.length - VIS)));
    const shown = retroProjects.slice(start, start + VIS);
    screen = (
      <Screen
        status={<><span style={{ color: "var(--phos-dim)" }}>회고 · 프로젝트</span><span className="pxmono9" style={{ color: "var(--phos-dim)" }}>{retroProjects.length ? `${retroCursor + 1}/${retroProjects.length}` : "—"}</span></>}
        buttons={retroProjects.length ? [
          { label: "뒤로", onClick: () => setMode("home") },
          { label: "▲", onClick: () => setRetroCursor((c) => (c - 1 + retroProjects.length) % retroProjects.length) },
          { label: "▼", onClick: () => setRetroCursor((c) => (c + 1) % retroProjects.length) },
          { label: "선택", primary: true, onClick: () => pickProject(retroProjects[retroCursor].dir) },
        ] : [{ label: "뒤로", onClick: () => setMode("home") }]}
      >
        {retroProjects.length === 0 ? (
          <div className="pxmono9" style={{ fontSize: 10, color: "var(--phos-dim)" }}>최근 7일 세션이 없어요.</div>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
            {shown.map((p) => {
              const i = retroProjects.indexOf(p);
              const on = i === retroCursor;
              return (
                <div key={p.dir} style={{ display: "flex", alignItems: "center", gap: 5, padding: "2px 4px", background: on ? "var(--phos)" : "transparent", color: on ? "var(--scr)" : "var(--phos)" }}>
                  <span style={{ width: 9, flex: "0 0 auto" }}>{on ? "▸" : ""}</span>
                  <span style={{ flex: 1, fontFamily: "var(--pixel-9)", fontSize: 10, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{p.label}</span>
                  <span className="pxmono9" style={{ flex: "0 0 auto", fontSize: 10, opacity: 0.85 }}>{p.sessions}세션 · {relTime(p.last_active)}</span>
                </div>
              );
            })}
          </div>
        )}
      </Screen>
    );
  } else if (mode === "retro" && (retroStage === "loading" || retroStage === "waking")) {
    screen = <CoachLoadingScreen waking={retroStage === "waking"} tag="회고 · 분석" onCancel={cancelRetro} />;
  } else if (mode === "retro" && retroStage === "error") {
    screen = (
      <Screen
        status={<><span style={{ color: "var(--phos-dim)" }}>회고 · 오류</span></>}
        buttons={[{ label: "뒤로", onClick: () => setRetroStage("intro") }, { label: "다시", primary: true, onClick: rerunRetro }]}
      >
        <div className="pxmono9" style={{ fontSize: 10, lineHeight: "15px", color: "var(--phos)" }}>
          <div style={{ marginBottom: 4 }}>회고를 만들지 못했어요.</div>
          <div style={{ color: "var(--phos-dim)", overflowWrap: "anywhere" }}>{retroError}</div>
        </div>
      </Screen>
    );
  } else if (mode === "retro") {
    screen = (
      <Screen
        status={<><span style={{ color: "var(--phos-dim)" }}>회고 · 결과</span><span className="pxmono9" style={{ color: "var(--phos-dim)" }}>{(retroGeneratedAt && relTime(retroGeneratedAt)) || "완료"}</span></>}
        buttons={[
          { label: "뒤로", onClick: () => setRetroStage("intro") },
          { label: "다시", onClick: rerunRetro },
          { label: "자세히", primary: true, onClick: () => setRetroReportOpen(true) },
        ]}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <div style={{ fontFamily: "var(--pixel-9)", fontSize: 10, lineHeight: "15px", color: "var(--phos)", whiteSpace: "nowrap", minWidth: 0 }}>
            <div style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{retroProjects.find((p) => p.dir === retroPickedDir)?.label ?? "프로젝트"}</div>
            <div>회고 준비됨</div>
            <div style={{ color: "var(--phos-dim)" }}>{retroGeneratedAt ? relTime(retroGeneratedAt) : "방금"}</div>
            <div style={{ color: "var(--phos-dim)" }}>'자세히'로 보기</div>
          </div>
        </div>
      </Screen>
    );
  } else if (mode === "info") {
    // 정보 — 페이지 전환(◀▶). CRT가 좁아 한 화면에 다 못 넣는다: 레벨/실력/시스템.
    const li = levelInfo;
    const pct = li ? Math.round(li.progress * 100) : 0;
    const rows: [string, number][] = li
      ? [["읽기", li.total_cache_read], ["쓰기", li.total_cache_creation], ["출력", li.total_output], ["입력", li.total_input]]
      : [];
    const totalUsed = li ? li.total_cache_read + li.total_cache_creation + li.total_output + li.total_input : 0;
    const PAGES = ["레벨", "실력", "에이전트", "시스템"];
    const page = infoPage % PAGES.length;
    const line = (k: React.ReactNode, v: React.ReactNode, strong = false) => (
      <div style={{ display: "flex", justifyContent: "space-between", fontFamily: "var(--pixel-mono-9)", fontSize: 10, lineHeight: "15px" }}>
        <span style={{ color: "var(--phos-dim)" }}>{k}</span>
        <span style={{ color: strong ? "var(--phos)" : "var(--phos-dim)" }}>{v}</span>
      </div>
    );
    // 절대값 등급은 매기지 않는다 — 작업 종류에 confound된다(§3.3.1에서 신호
    // 4종을 죽인 병). 같은 사람의 주 대비 변화만 보여준다.
    const delta = (a: number, b: number) => {
      const d = a - b;
      const sign = d > 0 ? "▲" : d < 0 ? "▼" : "·";
      return `${sign} ${d >= 0 ? "+" : ""}${d.toFixed(1)}%p`;
    };
    screen = (
      <Screen
        status={<><span style={{ color: "var(--phos-dim)", letterSpacing: "1px" }}>{PAGES[page]}</span><span className="pxmono9" style={{ color: "var(--phos-dim)" }}>{page + 1}/{PAGES.length}</span></>}
        buttons={[
          { label: "뒤로", onClick: () => setMode("home") },
          { label: "◀", onClick: () => setInfoPage((i) => (i - 1 + PAGES.length) % PAGES.length) },
          { label: "▶", onClick: () => setInfoPage((i) => (i + 1) % PAGES.length) },
        ]}
      >
        <div style={{ alignSelf: "stretch", minHeight: 0, overflow: "hidden", display: "flex", flexDirection: "column", gap: 6, fontFamily: "var(--pixel-9)" }}>
          {page === 0 && (!li ? (
            <div style={{ fontSize: 10, color: "var(--phos-dim)" }}>불러오는 중…</div>
          ) : (
            <>
              <div>
                <div style={{ position: "relative", height: 7, background: "var(--phos-14)", boxShadow: "inset 0 0 0 1px var(--phos-30)" }}>
                  <div style={{ position: "absolute", left: 0, top: 0, bottom: 0, width: `${pct}%`, background: "var(--phos)" }} />
                </div>
                {/* Lv·남은량·%를 한 줄로 — CRT 내용 영역이 ~102px이라 행 하나가
                    곧 넘침이다(실측으로 두 페이지 다 삐져나갔음). */}
                <div style={{ display: "flex", justifyContent: "space-between", fontFamily: "var(--pixel-mono-9)", fontSize: 10, lineHeight: "15px", marginTop: 4 }}>
                  <span style={{ color: "var(--phos-dim)" }}>
                    <span style={{ color: "var(--phos)" }}>Lv.{li.lv}</span>
                    {li.at_cap ? " · 최고 레벨" : ` · ${fmtTok(li.to_next)} 남음`}
                  </span>
                  <span style={{ color: "var(--phos)" }}>{li.at_cap ? "MAX" : `${pct}%`}</span>
                </div>
              </div>
              <div style={{ borderTop: "1.5px solid var(--phos-30)", paddingTop: 6 }}>
                {line("누적 사용 토큰", fmtTok(totalUsed), true)}
                <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: "3px 12px", marginTop: 4 }}>
                  {rows.map(([lbl, v]) => (
                    <div key={lbl} style={{ display: "flex", justifyContent: "space-between", fontFamily: "var(--pixel-mono-9)", fontSize: 10 }}>
                      <span style={{ color: "var(--phos-dim)" }}>{lbl}</span>
                      <span style={{ color: "var(--phos)" }}>{fmtTok(v)}</span>
                    </div>
                  ))}
                </div>
              </div>
            </>
          ))}

          {/* 실력 — 레벨은 소비(누적 토큰)를 재므로 "늘었는지"는 여기서 본다. */}
          {page === 1 && (!skill ? (
            <div style={{ fontSize: 10, lineHeight: "15px", color: "var(--phos-dim)" }}>
              계산 중이에요.<br />30분마다 백그라운드로 집계해요.
            </div>
          ) : (
            <>
              <div>
                <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-end", height: 20 }}>
                  <span style={{ fontSize: 10, lineHeight: "15px", color: "var(--phos-dim)" }}>한 번에 붙은 편집</span>
                  <span className="pxmono9" style={{ fontSize: 20, lineHeight: "20px", color: "var(--phos)" }}>{skill.now.one_shot.toFixed(0)}%</span>
                </div>
                <div style={{ fontFamily: "var(--pixel-mono-9)", fontSize: 10, lineHeight: "15px", color: "var(--phos-dim)", marginTop: 2 }}>
                  지난주 {skill.prev.one_shot.toFixed(0)}% · {delta(skill.now.one_shot, skill.prev.one_shot)}
                </div>
              </div>
              <div style={{ borderTop: "1.5px solid var(--phos-30)", paddingTop: 5, display: "flex", flexDirection: "column", gap: 2 }}>
                {line("되돌린 편집", `${skill.now.reworks} / ${skill.now.edits}`)}
                {line("같은 자리 최다 연속", `${skill.now.max_chain}회`)}
                {line("방향 없이 넘김", `${skill.now.delegation.toFixed(0)}% · ${delta(skill.now.delegation, skill.prev.delegation)}`)}
              </div>
            </>
          ))}

          {/* M2: 에이전트별 소비 — XP는 한 펫에 합산, 여긴 표시용 분해.
              두 에이전트를 항상 보여준다(없으면 0) — 조건부 숨김 금지 원칙. */}
          {page === 2 && (!agentUsage ? (
            <div style={{ fontSize: 10, color: "var(--phos-dim)" }}>불러오는 중…</div>
          ) : (
            <div style={{ display: "flex", flexDirection: "column", gap: 3 }}>
              {line("누적 소비 · 한 펫 합산", "", true)}
              {["claude", "codex"].map((a) =>
                line(a, fmtTok(agentUsage.agents.find((x) => x.label === a)?.tokens ?? 0)),
              )}
              {/* CLI 유무는 **상태**라 설정이 아니라 여기(대교, 2026-09-11). 없으면
                  온보딩·설정이 아니라 이 줄이 말하고, 설치 명령은 온보딩 1/4 에 있다. */}
              <div style={{ borderTop: "1.5px solid var(--phos-30)", paddingTop: 5 }}>
                {(hook?.agents ?? []).map((a) =>
                  line(`${a.id} CLI`, a.cli ? "✓" : <span style={{ color: "var(--rust)" }}>없음</span>),
                )}
              </div>
              <div style={{ borderTop: "1.5px solid var(--phos-30)", paddingTop: 5 }}>
                {line(
                  "codex 파싱",
                  agentUsage.codex_parse_failed > 0 ? (
                    <span style={{ color: "var(--rust)" }}>
                      실패 {agentUsage.codex_parse_failed}건
                    </span>
                  ) : (
                    `정상 · ${agentUsage.codex_parse_ok}건`
                  ),
                )}
              </div>
            </div>
          ))}

          {page === 3 && (
            <div style={{ display: "flex", flexDirection: "column", gap: 3 }}>
              {line("디스플레이", `${dpr}x · 보정 ${dpr < 1.5 ? "켜짐" : "꺼짐"}`)}
              {line("펫", activeVariant.name)}
              {line("유저 펫", `${userPets.length}종`)}
              {skill && line("이번 주 세션", `${skill.now.sessions}개`)}
              {skill && line("실력 집계", relTime(skill.computed_at))}
            </div>
          )}
        </div>
      </Screen>
    );
  } else {
    // settings — cursor list (▲▼변경), same interaction pattern as MENU.
    // 두 단: 최상위(알림·자동 실행·그룹 3개) → 그룹 안. "뒤로"는 한 단씩 나간다.
    const group = settingsGroup ? SETTINGS_GROUPS[settingsGroup] : null;
    const SETTINGS_ROWS = group ? group.rows : SETTINGS_TOP;
    const VIS = 5;
    const start = Math.max(0, Math.min(settingsCursor - 2, Math.max(0, SETTINGS_ROWS.length - VIS)));
    const shown = SETTINGS_ROWS.slice(start, start + VIS);
    const back = () => {
      if (group) { setSettingsGroup(null); setSettingsCursor(settingsTopCursor); }
      else setMode("home");
    };
    const primaryLabel = SETTINGS_ROWS[settingsCursor]?.id.startsWith("g:") ? "열기" : "변경";
    screen = (
      <Screen
        status={<><span style={{ color: "var(--phos-dim)", letterSpacing: "1px" }}>{group ? group.title : "SETTINGS"}</span><span className="pxmono9" style={{ color: "var(--phos-dim)" }}>{settingsCursor + 1}/{SETTINGS_ROWS.length}</span></>}
        buttons={[
          { label: "뒤로", onClick: back },
          { label: "▲", onClick: () => setSettingsCursor((c) => (c - 1 + SETTINGS_ROWS.length) % SETTINGS_ROWS.length) },
          { label: "▼", onClick: () => setSettingsCursor((c) => (c + 1) % SETTINGS_ROWS.length) },
          { label: primaryLabel, primary: true, onClick: () => changeSettingsRow(SETTINGS_ROWS[settingsCursor].id) },
        ]}
      >
        <div style={{ alignSelf: "stretch", display: "flex", flexDirection: "column", gap: 1 }}>
          {shown.map((row) => {
            const idx = SETTINGS_ROWS.indexOf(row);
            const on = idx === settingsCursor;
            const fg = on ? "var(--scr)" : "var(--phos)";
            const bg = on ? "var(--phos)" : "var(--scr)";
            const right = settingsRowRight(row.id);
            return (
              <div key={row.id} onClick={() => { setSettingsCursor(idx); changeSettingsRow(row.id); }}
                style={{ display: "flex", alignItems: "center", gap: 6, cursor: "pointer",
                  fontFamily: "var(--pixel-9)", fontSize: 10, lineHeight: 1, height: 15, padding: "0 5px", background: on ? "var(--phos)" : "transparent", color: fg }}>
                <span style={{ width: 7, flex: "0 0 auto" }}>{on ? "▸" : ""}</span>
                <span style={{ flex: 1, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{row.label}</span>
                {right.kind === "pill"
                  ? <Pill on={!!right.on} fg={fg} bg={bg} />
                  : right.kind === "cycle"
                  ? <span style={{ display: "flex", alignItems: "center", gap: 4, fontFamily: "var(--pixel-mono-9)", fontSize: 10 }}>
                      <span style={{ opacity: 0.6 }}>◂</span><span style={{ minWidth: 46, textAlign: "center" }}>{right.text}</span><span style={{ opacity: 0.6 }}>▸</span>
                    </span>
                  : <span style={{ fontFamily: "var(--pixel-mono-9)", fontSize: 10, color: row.id === "reset" && resetArmed ? "var(--rust)" : fg }}>{right.text}</span>}
              </div>
            );
          })}
          {start + VIS < SETTINGS_ROWS.length && (
            <div style={{ textAlign: "center", fontFamily: "var(--pixel-mono-9)", fontSize: 10, color: "var(--phos-30)", marginTop: 1 }}>▾</div>
          )}
        </div>
      </Screen>
    );
  }

  // 레트로 System 얼러트(V4-5) — 셸 CRT 안이 아니라 데스크 위에 뜨는
  // 별도 플로팅 창(구형 Mac System 알림처럼). `screen`은 그대로 두고,
  // 아래 return에서 fixed 오버레이로 띄운다.
  let alertNode: React.ReactNode = null;
  if (alert === "hunger") {
    alertNode = (
      <AlertDialog
        title="경고" sprite={alertRows("hungry")} spriteScale={4}
        ok="밥 주기" onOk={() => { setAlert(null); invoke("open_agent").catch(() => {}); }}
        cancel="나중에" onCancel={() => setAlert(null)}
        copyright="©1986 tokisoft"
      >
        {"토키가 배고파요.\n세션을 이어가면 밥이 돼요."}
      </AlertDialog>
    );
  } else if (alert === "retro") {
    alertNode = (
      <AlertDialog
        title="오늘의 회고"
        ok="계속" onOk={() => { setAlert(null); setMode("coaching"); }}
        cancel="닫기" onCancel={() => setAlert(null)}
        copyright="©1986 tokisoft"
      >
        {"이번 세션 회고가\n준비됐어요."}
      </AlertDialog>
    );
  } else if (alert === "newpet") {
    // The just-unlocked variant = highest unlockLv among those now unlocked.
    const newest = PET_VARIANTS.filter((v) => isUnlocked(v, lv)).reduce((a, b) => (b.unlockLv > a.unlockLv ? b : a));
    alertNode = (
      <AlertDialog
        title="토키의 메시지" sprite={newest.rows} spriteScale={4}
        ok="외형 보기" onOk={() => { setAlert(null); setMode("appearance"); }}
        cancel="닫기" onCancel={() => setAlert(null)}
        copyright="©1986 tokisoft"
      >
        {`새 친구 '${newest.name}' 도착!\nLv.${newest.unlockLv} 달성으로 해금됐어요.`}
      </AlertDialog>
    );
  } else if (alert === "update" && update) {
    alertNode = (
      <AlertDialog
        title="업데이트" width={224}
        ok={updating ? "받는 중" : "업데이트"} okDisabled={updating}
        onOk={() => {
          setUpdating(true);
          // 성공하면 앱이 스스로 종료되고 새 버전이 뜬다 — 여기서 닫지 않는다.
          invoke("update_run").catch((e) => {
            setUpdating(false);
            setUpdateError(String(e));
          });
        }}
        cancel="나중에" onCancel={() => setAlert(null)} cancelDisabled={updating}
        copyright="©1986 tokisoft"
      >
        {updateError
          ? `업데이트하지 못했어요.\n${updateError.slice(0, 60)}`
          : `새 버전 ${update.latest} 이 나왔어요.\n지금 ${update.current} 을 쓰고 있어요.`}
      </AlertDialog>
    );
  } else if (alert === "pomo") {
    // pomoPhase is already the NEW phase (we flipped before the alert), so
    // BREAK ⇒ focus just finished, FOCUS ⇒ break just finished.
    const toBreak = pomoPhase === "BREAK";
    // 버튼이 실제로 그 일을 한다. 예전엔 "쉬기"도 "닫기"도 그냥 팝업만 닫아서,
    // 쉬겠다고 눌러도 휴식 타이머가 멈춘 채 남아 있었다(2026-09-07 지적).
    const startNext = () => { setAlert(null); setPomoRunning(true); setMode("home"); };
    // 넘기기 = 이 phase를 건너뛰고 다음 phase를 기본 길이로 대기시킨다.
    // 완주가 아니므로 pomoDone은 올리지 않는다(skipPhase와 같은 규칙).
    const skipNext = () => {
      const next = toBreak ? "FOCUS" : "BREAK";
      const full = next === "FOCUS" ? pomoFocusFull : 5 * 60;
      setAlert(null); setPomoPhase(next); setPomoTotal(full); setPomoLeft(full); setPomoRunning(false);
    };
    alertNode = (
      <AlertDialog
        title="집중" sprite={pomoAlertRows} spriteScale={4}
        ok={toBreak ? "쉬기" : "집중"} onOk={startNext}
        cancel="넘기기" onCancel={skipNext}
        copyright="©1986 tokisoft"
      >
        {toBreak ? "집중 한 판 끝!\n5분 쉬어가요." : "휴식 끝!\n다시 집중해볼까요?"}
      </AlertDialog>
    );
  }

  return (
    // isolation:isolate pins this subtree to its own stacking context so the
    // floating alert overlay is unambiguously painted above the shell.
    <div style={{ width: "100%", height: "100%", display: "flex", alignItems: "center", justifyContent: "center", background: "transparent", isolation: "isolate" }}>
      {/* user-select:none is scoped HERE (no editables inside the shell),
          not on html/body — a none-ancestor kills contenteditable caret
          placement in WKWebView (memo titles live outside this subtree). */}
      <div style={{ position: "relative", zIndex: 1, WebkitUserSelect: "none", userSelect: "none" }}>
        <CassetteShell innerRef={shellRef} casing={settings?.case_color ?? "beige"} invert={settings?.invert_screen ?? false}>{screen}</CassetteShell>
      </div>
      {/* Toki's message is a modal now (was a bare floating box that read as
          disconnected): a full-screen dim backdrop + centered dialog, same
          pattern as the coaching report. The dim covers/darkens the shell to
          focus attention. Portaled to <body> so the dim + fixed positioning
          escape the desk's centering transform (which would otherwise confine
          them to the shell box). Backdrop is .desk-ctl so the desk's
          click-through makes the whole screen interactive; clicking the dim
          dismisses (= the "나중에"/"닫기" action). */}
      {alertNode && createPortal(
        <div className="desk-ctl" onClick={() => setAlert(null)}
          style={{ position: "fixed", inset: 0, zIndex: 210, display: "flex",
            alignItems: "center", justifyContent: "center", background: "rgba(14,12,18,.55)",
            WebkitUserSelect: "none", userSelect: "none" }}>
          <div onClick={(e) => e.stopPropagation()}>
            {alertNode}
          </div>
        </div>,
        document.body,
      )}
      {/* The report modal is a full-screen backdrop too — same portal escape. */}
      {deepReportOpen && deep && createPortal(
        <div className="desk-ctl" style={{ position: "fixed", inset: 0, zIndex: 220 }}>
          <DeepReportModal deep={deep} onClose={() => setDeepReportOpen(false)} onRerun={runDeepCoaching} />
        </div>,
        document.body,
      )}
      {egressOpen && createPortal(
        <div className="desk-ctl" style={{ position: "fixed", inset: 0, zIndex: 220 }}>
          <EgressModal preview={deepPreview} onClose={() => setEgressOpen(false)} />
        </div>,
        document.body,
      )}
      {/* v5 M5 기억 붙여넣기 (spec §6.4.1) — 데스크 위 패널. 셸 안 CRT는 긴
          텍스트를 담기엔 좁고, 셸의 user-select:none 조상 체인 밑에선 입력이
          죽는다(design-brief §13 함정 2종). body portal + 필드에서 선택 부활. */}
      {memoryPasteOpen && createPortal(
        <div className="desk-ctl" style={{ position: "fixed", inset: 0, zIndex: 220 }}>
          <MemoryPastePanel
            text={memoryPasteText}
            onChangeText={setMemoryPasteText}
            onClose={() => setMemoryPasteOpen(false)}
            onSaved={(bytes) => { setMemoryPasteBytes(bytes); setMemoryPasteOpen(false); }}
          />
        </div>,
        document.body,
      )}
      {retroReportOpen && createPortal(
        <div className="desk-ctl" style={{ position: "fixed", inset: 0, zIndex: 220 }}>
          <RetroReportModal body={retroBody} error={retroError} title={retroProjects.find((p) => p.dir === retroPickedDir)?.label} onClose={() => setRetroReportOpen(false)} />
        </div>,
        document.body,
      )}
    </div>
  );
}
