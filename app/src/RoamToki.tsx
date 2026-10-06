/* M10 자유 이동 — 토키가 셸 밖 데스크톱을 돌아다닌다 (spec §9.6).
 *
 * 데모 <https://claude.ai/artifact/SeJngHmPzVAbArgWRjs91G> 가 계약이다. 수치는
 * 전부 거기서 확정했다(2026-10-06): 몸 64px, 바탕 여유 20px(본체 104×96), 디더
 * 80/90/100%, 방출 6~40/s, 픽셀 소멸·등장 0.6s.
 *
 * 구조:
 *   - 바탕(포탈)은 토키와 **별개 캔버스**. 몸 중심에 고정, 크기 불변. 토키의
 *     걷기 흔들림·앉기·뒤집기·벽 회전에 안 끌려간다.
 *   - FSM: floor | plat | wallL | wallR | ceil | air. 발판은 Rust `desk_windows`
 *     (보이는 창의 타이틀바 윗선) + 셸 윗선.
 *   - 클릭스루: 몸 요소에 `desk-ctl` 를 달아 MemoDesk 의 reportRects 가 집는다.
 *     바탕·입자는 통과.
 *   - 전이: 부모가 `phase` 로 지시한다. "in" 이면 바탕→토키 순으로 한 픽셀씩
 *     나타나고, "out" 이면 토키→바탕 순으로 사라진 뒤 onOutDone 을 부른다.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type RoamPhase = "in" | "roam" | "out";
type Surface = "floor" | "plat" | "wallL" | "wallR" | "ceil" | "air";
type Plat = { x0: number; x1: number; y: number };

const S = 64;                       // 몸(16×16 도트 × 4)
const PAD_W = 40, PAD_H = 38;       // 바탕 캔버스(셀) — 본체 26×24 + 방출 여유
const CORE_W = 26, CORE_H = 24;
const OX = (PAD_W - CORE_W) / 2, OY = (PAD_H - CORE_H) / 2;
const CELL = 4;
const DARK = "12,12,18";
const G = 1800;

/* 픽셀 순서표 — 사라지고 나타날 때 한 픽셀씩. 순서가 고정이라 사라진 역순으로 돌아온다. */
const ORDER: number[] = (() => {
  const a: number[] = []; for (let i = 0; i < 256; i++) a.push(i);
  let seed = 7;
  const rnd = () => { seed = (seed * 1103515245 + 12345) & 0x7fffffff; return seed / 0x7fffffff; };
  for (let i = 255; i > 0; i--) { const j = Math.floor(rnd() * (i + 1)); [a[i], a[j]] = [a[j], a[i]]; }
  const rank = new Array<number>(256); a.forEach((v, i) => (rank[v] = i / 255)); return rank;
})();
/** 다른 컴포넌트(셸)도 같은 순서로 소멸하도록 내보낸다. */
export function pixelRank(x: number, y: number) { return ORDER[(y % 16) * 16 + (x % 16)]; }
const hash = (x: number, y: number, t: number) => { const n = Math.sin(x * 12.9898 + y * 78.233 + t * 37.7) * 43758.5453; return n - Math.floor(n); };

function drawSprite(cv: HTMLCanvasElement, rows: string[], eyes: [number, number][], gx: number, gy: number, blink: boolean, reveal: number) {
  const c = cv.getContext("2d"); if (!c) return;
  const h = rows.length, w = Math.max(...rows.map((r) => r.length));
  if (cv.width !== w || cv.height !== h) { cv.width = w; cv.height = h; }
  c.clearRect(0, 0, w, h);
  const phos = getComputedStyle(cv).getPropertyValue("--phos").trim() || "#e9ead0";
  const eyeSet = new Set(eyes.map(([r, cc]) => r * 64 + cc));
  c.fillStyle = phos;
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    const ch = rows[y][x]; if (ch !== "#" && ch !== "@" && ch !== "o") continue;
    if (pixelRank(x, y) > reveal) continue;
    const isEye = eyeSet.has(y * 64 + x);
    if (isEye) { if (blink) continue; c.fillRect(x + gx, y + gy, 1, 1); continue; }
    c.globalAlpha = ch === "o" ? 0.52 : 1; c.fillRect(x, y, 1, 1); c.globalAlpha = 1;
  }
}

/** 걸어서 넘어오면 side+y, 드래그로 떨어뜨리면 x+y(그 창 CSS px). */
export type RoamEntry = { side?: "left" | "right"; x?: number; y: number };
export function RoamToki({ rows, eyes, phase, entry, shellRect, canCross, onCross, onDragState, onDropOutside, onDragOutside, onOutDone, onHome, onFeed, onFocus, onMenu }: {
  rows: string[]; eyes: [number, number][];
  phase: RoamPhase;
  /** 옆 모니터에서 넘어온 경우의 진입 변(side)과 y(이 창 CSS px). */
  entry?: RoamEntry | null;
  /** 이 변에 옆 모니터가 있나 — 있으면 벽을 타지 않고 넘어간다. */
  canCross: (side: "left" | "right", y: number) => boolean;
  onCross: (side: "left" | "right", y: number) => void;
  /** 드래그 중엔 데스크가 창 전체를 조작 가능으로 묶어야 커서가 창 밖으로 나가도 드래그가 산다. */
  onDragState: (dragging: boolean) => void;
  /** 창 밖(옆 모니터)에 떨어뜨렸다 — 데스크가 어느 모니터인지 알아내 넘긴다. */
  onDropOutside: (clientX: number, clientY: number) => void;
  /** 드래그 중 창 밖에 있을 때(옆 모니터 미리보기용). 안이면 null. */
  onDragOutside: (outside: boolean) => void;
  /** 셸의 현재 사각형(데스크 창 CSS px). 발판·귀가 판정에 쓴다. */
  shellRect: () => DOMRect | null;
  onOutDone: () => void;
  onHome: () => void; onFeed: () => void; onFocus: () => void; onMenu: () => void;
}) {
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const cvRef = useRef<HTMLCanvasElement | null>(null);
  const padRef = useRef<HTMLCanvasElement | null>(null);
  const bubbleRef = useRef<HTMLDivElement | null>(null);
  const [bubble, setBubble] = useState(false);
  const [say, setSay] = useState<string | null>(null);

  // 모든 가변 상태는 ref — 60fps 루프가 React 렌더를 안 탄다.
  const T = useRef({
    x: 0, y: 0, vx: 0, vy: 0, dir: 1 as 1 | -1, surface: "floor" as Surface, mode: "idle", t: 0, until: 0,
    goal: null as null | "wallL" | "wallR" | { plat: Plat; tx: number }, goalCeil: false, plat: null as Plat | null,
    drag: false, gx: 0, gy: 0, blink: false, reveal: 0, padReveal: 0,
  }).current;
  const plats = useRef<Plat[]>([]);
  const cursor = useRef({ x: -9999, y: -9999 });
  const wisps = useRef<{ x: number; y: number; vx: number; vy: number; life: number; age: number; seed: number }[]>([]);
  const padT = useRef(0); const padSpeed = useRef(0);
  const W = () => window.innerWidth, H = () => window.innerHeight;
  const now = () => performance.now() / 1000;
  const rnd = (a: number, b: number) => a + Math.random() * (b - a);

  /* 발판: 보이는 창 타이틀바 윗선 + 셸 윗선. 2초 폴링. */
  useEffect(() => {
    let alive = true;
    const pull = () => invoke<{ x: number; y: number; w: number; h: number }[]>("desk_windows")
      .then((ws) => { if (!alive) return; const ps: Plat[] = ws.map((w) => ({ x0: w.x, x1: w.x + w.w, y: w.y }));
        const sr = shellRect(); if (sr) ps.push({ x0: sr.left, x1: sr.right, y: sr.top });
        plats.current = ps.filter((p) => p.y > 60 && p.y < H() - 20); })
      .catch(() => {});
    pull(); const iv = window.setInterval(pull, 2000);
    return () => { alive = false; window.clearInterval(iv); };
  }, [shellRect]);

  /* 커서 — 데스크는 클릭스루라 pointermove 가 안 온다. Rust 의 cursor-pos(창 좌표). */
  useEffect(() => {
    let target: { kind: "AnyLabel"; label: string } | undefined;
    try { target = { kind: "AnyLabel", label: getCurrentWindow().label }; } catch { target = undefined; }
    const un = listen<[number, number]>("cursor-pos", (e) => { cursor.current = { x: e.payload[0], y: e.payload[1] }; }, target ? { target } : undefined);
    return () => { un.then((f) => f()).catch(() => {}); };
  }, []);
  useEffect(() => { const id = window.setInterval(() => { T.blink = true; window.setTimeout(() => (T.blink = false), 120); }, 3400); return () => window.clearInterval(id); }, [T]);

  const speak = useCallback((m: string, ms = 1500) => { setSay(m); window.setTimeout(() => setSay((s) => (s === m ? null : s)), ms); }, []);

  /* ── 의도 ── Shimeji 식: 걷기·앉기·보기·벽·천장·창 위 */
  const canCrossRef = useRef(canCross); canCrossRef.current = canCross;
  const onCrossRef = useRef(onCross); onCrossRef.current = onCross;
  const pick = useCallback((force?: string) => {
    const r = force ?? ["walk", "walk", "walk", "sit", "climb", "window", "ceiling", "look"][Math.floor(rnd(0, 8))];
    T.mode = "idle"; T.until = now() + rnd(1.2, 3.5); T.goal = null; T.goalCeil = false;
    if (r === "walk") { T.mode = "walk"; T.dir = Math.random() < .5 ? -1 : 1; T.until = now() + rnd(1.5, 4); }
    else if (r === "sit") { T.mode = "sit"; T.until = now() + rnd(2.5, 5); }
    else if (r === "look") { T.mode = "look"; T.until = now() + rnd(2, 4); }
    else if (r === "climb" || r === "ceiling") {
      // 벽은 옆에 모니터가 **없는** 변만 — 있는 쪽은 벽이 아니라 문이다.
      const sides = (["wallL", "wallR"] as const).filter((w) => !canCrossRef.current(w === "wallL" ? "left" : "right", T.y));
      if (!sides.length) { pick("walk"); return; }
      T.goal = sides.length === 2 ? (T.x < W() / 2 ? "wallL" : "wallR") : sides[0];
      T.mode = "walk"; T.dir = T.goal === "wallL" ? -1 : 1; T.goalCeil = r === "ceiling"; T.until = now() + 30;
    }
    else if (r === "window") {
      const ps = plats.current.filter((p) => p.y < T.y - 40); if (!ps.length) { pick("walk"); return; }
      const p = ps[Math.floor(rnd(0, ps.length))]; const tx = Math.min(Math.max(p.x0 + 40, T.x), p.x1 - 40);
      T.mode = "walk"; T.goal = { plat: p, tx }; T.dir = tx < T.x ? -1 : 1; T.until = now() + 30;
    }
  }, [T]);
  const jumpTo = (x: number, y: number) => { const dt = 0.55; T.surface = "air"; T.vx = (x - T.x) / dt; T.vy = (y - T.y - 0.5 * G * dt * dt) / dt; T.mode = "jump"; };

  const step = (dt: number) => {
    T.t += dt; if (T.drag) return;
    const floorY = H();
    if (T.surface === "air") {
      T.vy += G * dt; T.x += T.vx * dt; T.y += T.vy * dt;
      if (T.vy > 0) {
        for (const p of plats.current) if (T.x > p.x0 && T.x < p.x1 && T.y >= p.y && T.y - T.vy * dt <= p.y) { T.y = p.y; T.surface = "plat"; T.plat = p; T.vx = T.vy = 0; pick("sit"); return; }
        if (T.y >= floorY) { T.y = floorY; T.surface = "floor"; T.vx = T.vy = 0; pick("walk"); }
      }
      T.x = Math.max(S / 2, Math.min(W() - S / 2, T.x)); return;
    }
    if (T.surface === "floor" || T.surface === "plat") {
      if (T.mode === "walk") {
        T.x += T.dir * 70 * dt;
        if (T.goal === "wallL" && T.x <= S / 2 + 2) { T.x = S / 2 + 2; T.surface = "wallL"; T.mode = "climb"; T.until = now() + rnd(2, 4); T.goal = null; }
        else if (T.goal === "wallR" && T.x >= W() - S / 2 - 2) { T.x = W() - S / 2 - 2; T.surface = "wallR"; T.mode = "climb"; T.until = now() + rnd(2, 4); T.goal = null; }
        else if (T.goal && typeof T.goal === "object" && Math.abs(T.x - T.goal.tx) < 4) { const g = T.goal; T.goal = null; jumpTo(g.tx, g.plat.y); }
        if (T.surface === "plat" && T.plat) { const p = T.plat; if (!plats.current.includes(p) || T.x < p.x0 + 8 || T.x > p.x1 - 8) { T.surface = "air"; T.vx = T.dir * 60; T.vy = 0; } }
        // 화면 끝: 옆 모니터가 있으면 넘어간다(메모와 같은 길), 없으면 돌아선다
        if (T.x <= S / 2 && T.dir < 0 && canCrossRef.current("left", T.y)) { T.mode = "idle"; onCrossRef.current("left", T.y); return; }
        if (T.x >= W() - S / 2 && T.dir > 0 && canCrossRef.current("right", T.y)) { T.mode = "idle"; onCrossRef.current("right", T.y); return; }
        if (T.x <= S / 2 || T.x >= W() - S / 2) T.dir = (T.dir * -1) as 1 | -1;
      } else if (T.surface === "plat" && T.plat && !plats.current.includes(T.plat)) { T.surface = "air"; T.vx = 0; T.vy = 0; } // 창이 닫혔다
      if (now() > T.until && !T.goal) pick();
    } else if (T.surface === "wallL" || T.surface === "wallR") {
      T.y -= 60 * dt;
      if (T.y - S <= 0) {
        if (T.goalCeil) { T.goalCeil = false; T.surface = "ceil"; T.y = S; T.dir = T.x < W() / 2 ? 1 : -1; T.mode = "walk"; T.until = now() + rnd(3, 6); }
        else { T.surface = "air"; T.vx = T.x < W() / 2 ? 140 : -140; T.vy = -200; }
      } else if (now() > T.until) { T.surface = "air"; T.vx = T.x < W() / 2 ? 140 : -140; T.vy = -150; }
    } else if (T.surface === "ceil") {
      T.x += T.dir * 60 * dt;
      if (T.x <= S / 2 || T.x >= W() - S / 2 || now() > T.until) { T.surface = "air"; T.vx = 0; T.vy = 0; }
    }
  };

  /* ── 바탕(포탈) + 방출 ── */
  const drawPad = (dt: number, speed: number, vel: { x: number; y: number }) => {
    const pc = padRef.current?.getContext("2d"); if (!pc) return;
    padT.current += dt; padSpeed.current += (speed - padSpeed.current) * 0.15;
    const v = Math.min(1, padSpeed.current / 160);
    const rate = T.padReveal < 1 ? 0 : 6 + v * 40;
    if (Math.random() < rate * dt) {
      const ang = Math.random() * Math.PI * 2;
      const sp = Math.hypot(vel.x, vel.y);
      const back = sp > 1 ? { x: -vel.x / sp, y: -vel.y / sp } : { x: 0, y: 0 };
      const bias = Math.cos(ang) * back.x + Math.sin(ang) * back.y;
      if (!(v > 0.2 && bias < -0.2 && Math.random() < 0.7)) wisps.current.push({
        x: PAD_W / 2 + Math.cos(ang) * (CORE_W / 2) * 0.8, y: PAD_H / 2 + Math.sin(ang) * (CORE_H / 2) * 0.8,
        vx: Math.cos(ang) * (3 + v * 4) + back.x * v * 10, vy: Math.sin(ang) * (3 + v * 4) + back.y * v * 10 - 2,
        life: 0.9 + Math.random() * 0.5, age: 0, seed: Math.random() });
    }
    for (const w of wisps.current) { w.age += dt; w.x += w.vx * dt; w.y += w.vy * dt; w.vx *= 0.97; w.vy = w.vy * 0.97 - 1.5 * dt; }
    wisps.current = wisps.current.filter((w) => w.age < w.life);
    pc.clearRect(0, 0, PAD_W, PAD_H); pc.fillStyle = `rgba(${DARK},.9)`;
    for (let y = 0; y < CORE_H; y++) for (let x = 0; x < CORE_W; x++) {
      const dx = (x + .5 - CORE_W / 2) / (CORE_W / 2), dy = (y + .5 - CORE_H / 2) / (CORE_H / 2); const d = Math.sqrt(dx * dx + dy * dy);
      if (hash(x, y, 7) > T.padReveal) continue;
      const px = x + OX, py = y + OY;
      if (d < .8) { pc.fillRect(px, py, 1, 1); continue; }
      if (d < .9 && (x + y) % 2 === 0) { pc.fillRect(px, py, 1, 1); continue; }
      if (d < 1 && x % 2 === 0 && y % 2 === 0) pc.fillRect(px, py, 1, 1);
    }
    for (const w of wisps.current) {
      const k = 1 - w.age / w.life, cx = Math.floor(w.x), cy = Math.floor(w.y);
      if (k < 0.5 && (cx + cy + Math.floor(w.seed * 2)) % 2) continue;
      if (k < 0.25 && cx % 2) continue;
      pc.fillStyle = `rgba(${DARK},${0.9 * Math.min(1, k * 1.4)})`; pc.fillRect(cx, cy, 1, 1);
    }
  };

  const place = () => {
    const el = bodyRef.current, pad = padRef.current; if (!el || !pad) return;
    el.style.left = `${T.x - S / 2}px`; el.style.top = `${T.y - S}px`;
    let tf = `scaleX(${T.dir})`;
    if (T.surface === "wallL") tf = "rotate(90deg)";
    if (T.surface === "wallR") tf = "rotate(-90deg)";
    if (T.surface === "ceil") tf = `scaleX(${T.dir}) scaleY(-1)`;
    if (T.mode === "walk" || T.mode === "climb") tf += ` translateY(${Math.sin(T.t * 14) * 2}px)`;
    if (T.mode === "sit") tf += " scaleY(.92)";
    el.style.transform = tf;
    pad.style.left = `${T.x - (PAD_W * CELL) / 2}px`; pad.style.top = `${T.y - S / 2 - (PAD_H * CELL) / 2}px`;
    const b = bubbleRef.current; if (b) { b.style.left = `${T.x - 70}px`; b.style.top = `${T.y - S - 36}px`; }
  };

  /* ── 전이: 부모의 phase 를 따른다 ── */
  const phaseRef = useRef(phase); phaseRef.current = phase;
  const outDoneRef = useRef(onOutDone); outDoneRef.current = onOutDone;
  useEffect(() => {
    if (entry && phase === "roam") {
      T.reveal = 1; T.padReveal = 1; T.drag = false; T.goal = null; T.goalCeil = false;
      if (entry.x !== undefined) { // 드래그로 떨어뜨림 — 그 자리에서 낙하
        T.x = Math.max(S / 2, Math.min(W() - S / 2, entry.x)); T.y = entry.y + S / 2; T.surface = "air"; T.vx = 0; T.vy = 0; T.mode = "idle"; return;
      }
      T.x = entry.side === "left" ? S / 2 + 1 : W() - S / 2 - 1; T.dir = entry.side === "left" ? 1 : -1;
      // 바닥 높이에 가까우면 바닥, 아니면 공중(발판이 있으면 거기 착지, 없으면 낙하)
      if (entry.y >= H() - 4) { T.y = H(); T.surface = "floor"; } else { T.y = entry.y; T.surface = "air"; T.vx = T.dir * 60; T.vy = 0; }
      T.mode = "walk"; T.until = now() + rnd(1.5, 4);
      return;
    }
    if (phase === "in") {
      // 셸 아래 바닥에서 시작 — 바탕 → 토키 순으로 한 픽셀씩
      const sr = shellRect(); T.x = Math.min(W() - 80, Math.max(80, sr ? sr.left + sr.width / 2 : W() / 2)); T.y = H();
      T.surface = "floor"; T.mode = "idle"; T.until = now() + 1.6; T.reveal = 0; T.padReveal = 0; T.drag = false;
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase]);

  /* ── 루프 ── */
  useEffect(() => {
    let raf = 0, last = now(), px = T.x, py = T.y;
    const tick = () => {
      const t = now(), dt = Math.min(0.05, t - last); last = t;
      const ph = phaseRef.current;
      if (ph === "in") { T.padReveal = Math.min(1, T.padReveal + dt / 0.5); if (T.padReveal >= 1) T.reveal = Math.min(1, T.reveal + dt / 0.6); if (T.reveal >= 1 && T.mode === "idle" && now() > T.until) { /* 바로 활동 */ } }
      else if (ph === "out") { T.reveal = Math.max(0, T.reveal - dt / 0.6); if (T.reveal <= 0) { T.padReveal = Math.max(0, T.padReveal - dt / 0.5); if (T.padReveal <= 0) { outDoneRef.current(); last = now(); raf = requestAnimationFrame(tick); return; } } }
      if (ph === "roam" || (ph === "in" && T.reveal >= 1)) step(dt);
      place();
      const sp = dt > 0 ? Math.hypot(T.x - px, T.y - py) / dt : 0;
      drawPad(dt, sp, { x: (T.x - px) / Math.max(dt, 1e-3), y: (T.y - py) / Math.max(dt, 1e-3) }); px = T.x; py = T.y;
      // 시선
      const c = cursor.current, cx = T.x, cy = T.y - S / 2, ux = c.x - cx, uy = c.y - cy;
      const flip = T.surface === "ceil" ? -1 : 1;
      T.gx = Math.abs(ux) < 10 ? 0 : Math.sign(ux) * T.dir * flip; T.gy = Math.abs(uy) < 10 ? 0 : Math.sign(uy) * flip;
      if (cvRef.current) drawSprite(cvRef.current, rows, eyes, T.gx, T.gy, T.blink, T.reveal);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rows, eyes]);

  /* ── 클릭·드래그 ── */
  const down = useRef<{ x: number; y: number } | null>(null);
  const onDown = (e: React.PointerEvent) => { down.current = { x: e.clientX, y: e.clientY }; (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId); };
  const onMove = (e: React.PointerEvent) => {
    if (down.current && !T.drag && Math.hypot(e.clientX - down.current.x, e.clientY - down.current.y) > 6) { T.drag = true; T.surface = "air"; T.mode = "idle"; setBubble(false); onDragState(true); }
    if (T.drag) { T.x = e.clientX; T.y = e.clientY + S / 2; onDragOutside(e.clientX < 0 || e.clientY < 0 || e.clientX > window.innerWidth || e.clientY > window.innerHeight); }
    cursor.current = { x: e.clientX, y: e.clientY };
  };
  const onUp = (e: React.PointerEvent) => {
    if (T.drag) {
      T.drag = false; onDragState(false); onDragOutside(false);
      const out = e.clientX < -8 || e.clientY < -8 || e.clientX > window.innerWidth + 8 || e.clientY > window.innerHeight + 8;
      if (out) { onDropOutside(e.clientX, e.clientY); return; }
      const r = shellRect();
      if (r && e.clientX > r.left && e.clientX < r.right && e.clientY > r.top && e.clientY < r.bottom) { onHome(); return; } // 셸 위에 놓으면 귀가
      T.surface = "air"; T.vx = 0; T.vy = 0;
    } else if (down.current && phaseRef.current === "roam") setBubble((b) => !b);
    down.current = null;
  };

  return (
    <>
      <canvas ref={padRef} width={PAD_W} height={PAD_H} aria-hidden="true"
        style={{ position: "absolute", left: 0, top: 0, width: PAD_W * CELL, height: PAD_H * CELL, imageRendering: "pixelated", pointerEvents: "none", zIndex: 25 }} />
      <div ref={bodyRef} className="desk-ctl" role="img" aria-label="토키"
        onPointerDown={onDown} onPointerMove={onMove} onPointerUp={onUp}
        style={{ position: "absolute", left: 0, top: 0, width: S, height: S, cursor: T.drag ? "grabbing" : "grab", zIndex: 26, touchAction: "none", transformOrigin: "50% 50%" }}>
        <canvas ref={cvRef} width={16} height={16} style={{ width: S, height: S, imageRendering: "pixelated", display: "block" }} />
        {say && <div className="px9" style={{ position: "absolute", left: S + 6, top: -8, background: "#efe4c2", color: "#3a3122", fontFamily: "var(--pixel-9)", fontSize: 10, padding: "3px 6px", whiteSpace: "nowrap", pointerEvents: "none" }}>{say}</div>}
      </div>
      {bubble && (
        <div ref={bubbleRef} className="desk-ctl" role="menu" style={{ position: "absolute", display: "flex", gap: 4, background: "var(--scr)", border: "1.5px solid var(--phos)", padding: 4, zIndex: 27 }}>
          {[["밥", () => { setBubble(false); onFeed(); speak("밥!"); }], ["집중", () => { setBubble(false); onFocus(); }], ["메뉴", () => { setBubble(false); onMenu(); }], ["귀가", () => { setBubble(false); onHome(); }]].map(([l, f]) => (
            <button key={l as string} className="cf-soft auto" style={{ fontSize: 10, padding: "3px 8px", color: "var(--phos)" }} onClick={f as () => void}>{l as string}</button>
          ))}
        </div>
      )}
    </>
  );
}
