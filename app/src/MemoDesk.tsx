/**
 * V4-7 — Memo desk. Ported from the Claude Design "Tokisoft" cassette-notes.jsx.
 * A larger transparent window: the cassette shell sits centered, with
 * draggable sticky notes floating around it. Each note holds a checklist.
 *
 * Click-through (the hard part): the window is transparent, so we report the
 * opaque rects (shell + every note) to Rust's forward click-through poll.
 * DURING A DRAG we instead report one full-window rect — otherwise, when the
 * cursor crosses a transparent gap mid-drag, Rust flips `ignore_cursor_events`
 * on and macOS stops delivering pointer events, killing the drag. Pinning the
 * whole window interactive for the drag's duration avoids that race.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useDesk, setDesk, claimsNote, isOrphanNote, type MonitorInfo, type ShellPos } from "./deskStore";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { emit, listen } from "@tauri-apps/api/event";

/** 단축키 표기 — mac은 ⌘, 그 외(Windows)는 Ctrl. 처리 자체는 metaKey||ctrlKey로 둘 다 받는다. */
const MOD_KEY = /Mac|iPhone|iPad/.test(navigator.platform) ? "⌘" : "Ctrl+";
import { CassetteShell, TamagotchiShell } from "./TamagotchiShell";

// Borderless transparent windows don't reliably become the key window on
// click (macOS), and without key status the webview gets mouse events but
// no keyboard — buttons work, typing doesn't. Explicitly grab focus when
// the user reaches for an editable. No-op outside Tauri (preview).
function grabKeyFocus() {
  try {
    getCurrentWindow().setFocus().catch(() => {});
  } catch { /* non-tauri preview */ }
}

/* Uncontrolled contenteditable: seeds its DOM once, commits on EVERY input.
   Saving must not wait for blur — on this desk a click often exits through
   the transparent click-through gap straight to the app behind, so the
   webview never delivers focusout and a blur-only save silently drops the
   edit (the "제목이 '메모'로 되돌아옴" bug). Because the DOM is never
   re-rendered from state (no dangerouslySetInnerHTML), per-keystroke commits
   don't disturb the caret. Enter commits (blurs) instead of inserting a
   newline. */
// 메모 본문은 <b>만 허용하는 살균 HTML로 저장한다(cmd+B 볼드 지원).
// 저장값이 곧 DOM에 들어가므로, 붙여넣기로 들어온 임의 마크업은 전부
// 텍스트로 눕히고 <b>/<strong>만 <b>로 정규화한다. 레거시 평문도 그대로 통과.
function sanitizeRich(html: string): string {
  const tpl = document.createElement("div");
  tpl.innerHTML = html;
  const walk = (node: Node): string => {
    let out = "";
    node.childNodes.forEach((n) => {
      if (n.nodeType === Node.TEXT_NODE) {
        out += (n.textContent ?? "").replace(/[&<>]/g, (ch) =>
          ch === "&" ? "&amp;" : ch === "<" ? "&lt;" : "&gt;");
      } else if (n.nodeType === Node.ELEMENT_NODE) {
        const tag = (n as Element).tagName.toLowerCase();
        const inner = walk(n);
        out += tag === "b" || tag === "strong" ? `<b>${inner}</b>` : inner;
      }
    });
    return out;
  };
  return walk(tpl).replace(/<b><\/b>/g, "");
}

function EditableText({ className, style, initial, onCommit, onEmptyBlur, onToggleCheck }: {
  className?: string;
  style?: React.CSSProperties;
  initial: string;
  onCommit: (t: string) => void;
  onEmptyBlur?: () => void;
  /** cmd+L — 이 줄을 체크리스트/일반 텍스트로 전환 */
  onToggleCheck?: () => void;
}) {
  const seed = (el: HTMLSpanElement | null) => {
    if (el && el.dataset.seeded !== "1") {
      el.dataset.seeded = "1";
      el.innerHTML = sanitizeRich(initial);
    }
  };
  return (
    <span ref={seed} className={className} style={style} contentEditable suppressContentEditableWarning
      onPointerDown={grabKeyFocus}
      onInput={(e) => onCommit(sanitizeRich(e.currentTarget.innerHTML))}
      onKeyDown={(e) => {
        if (e.key === "Enter") { e.preventDefault(); e.currentTarget.blur(); return; }
        // cmd+B: 선택 영역 볼드. execCommand는 deprecated이나 WKWebView에서
        // contenteditable 인라인 서식의 유일한 실용 수단이라 계속 쓴다.
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "b") {
          e.preventDefault();
          document.execCommand("bold");
          onCommit(sanitizeRich(e.currentTarget.innerHTML));
          return;
        }
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "l" && onToggleCheck) {
          e.preventDefault();
          onToggleCheck();
        }
      }}
      onBlur={(e) => {
        const plain = (e.currentTarget.textContent ?? "").trim();
        // WebKit leaves a stray <br> after clearing, which defeats the
        // :empty placeholder — normalize before committing the trim.
        if (!plain) e.currentTarget.innerHTML = "";
        onCommit(plain ? sanitizeRich(e.currentTarget.innerHTML) : "");
        if (!plain) onEmptyBlur?.();
      }} />
  );
}

// 노트 스키마의 정본은 deskStore다 — 창들이 같은 파일을 공유하므로
// 여기서 다시 정의하면 두 곳이 갈라진다.
export type { Note, NoteItem, NoteColor } from "./deskStore";
type NoteColor = import("./deskStore").NoteColor;
// chk: 체크박스 표시 여부. 미지정(레거시 저장분)은 체크리스트로 취급하고,
// 새로 추가하는 줄은 일반 텍스트(chk:false)로 시작한다 — cmd+L로 전환.
type NoteItem = import("./deskStore").NoteItem;
type Note = import("./deskStore").Note;

const NOTE_COLORS: Record<NoteColor, { bg: string; ink: string }> = {
  manila: { bg: "#e7dcbe", ink: "#2f2718" },
  mint: { bg: "#cfe0cb", ink: "#25352a" },
  rose: { bg: "#eccdc7", ink: "#3a2622" },
  sky: { bg: "#cdd8e6", ink: "#232f3d" },
};
const PALETTE: NoteColor[] = ["manila", "mint", "rose", "sky"];
const uid = () => Math.random().toString(36).slice(2, 9);

/* v4 잔재 — 메모는 이제 `~/.toki/desk.json`이 주인이다(deskStore). 이 키들은
   **한 번만** 읽어 옮기고 지운다. 남겨두면 창마다 다른 값이 살아남는다. */
const DK_KEY = "toki-desk-notes-v2";
const DK_INIT = "toki-desk-init-v2";
const SHELL_POS_LEGACY = "toki-desk-shell-pos-v2";

/** localStorage에 남아 있는 v4 데스크를 파일 정본으로 승계한다(1회). */
function migrateLegacyDesk(): { notes: Note[]; shellPos: ShellPos | null } | null {
  let notes: Note[] | null = null;
  let shellPos: ShellPos | null = null;
  try {
    const raw = JSON.parse(localStorage.getItem(DK_KEY) || "null");
    if (Array.isArray(raw) && raw.length) notes = raw as Note[];
  } catch { /* ignore */ }
  try {
    const p = JSON.parse(localStorage.getItem(SHELL_POS_LEGACY) || "null");
    if (p && typeof p.fx === "number") shellPos = p;
  } catch { /* ignore */ }
  if (!notes && !shellPos) return null;
  return { notes: notes ?? [], shellPos };
}
function clearLegacyDesk() {
  try {
    localStorage.removeItem(DK_KEY);
    localStorage.removeItem(DK_INIT);
    localStorage.removeItem(SHELL_POS_LEGACY);
  } catch { /* ignore */ }
}

// 메모 카드 기울기 배율. 0으로 두면 반듯해진다.
// 2026-08-10 실측: 회전을 0으로 껐다 켜 비교한 결과 글자 깨짐의 원인이
// 아니었고(Retina에선 오히려 손글씨 느낌이 강점), 기울기를 유지한다.
const CARD_ROT = 1;

/* ── 고스트 프로토콜 ───────────────────────────────────────────
   드래그 한 번 = `dragId` 하나. 보내는 쪽은 놓는 순간 **무조건** 종료를
   쏘고, 받는 쪽은 종료된 dragId의 미리보기를 **영구히 무시**한다.

   왜 이렇게까지: 좌표 조회가 비동기라 "손을 놓았다"와 "좌표가 도착했다"가
   엇갈린다. 전에는 플래그 하나로 지웠는데, 응답이 놓은 뒤에 오면 지울 것이
   없다고 판단하고 그 다음에 미리보기가 떠서 **유령이 영영 남았다**
   (2026-09-03 실기). 순서에 기대지 않고 id로 닫는다. */
type GhostMsg = { from: string; dragId: string; ghost: Ghost; done: boolean };
type Ghost =
  | { kind: "memo"; key: string; x: number; y: number; w: number; h: number; color: NoteColor; title: string; rot: number }
  | { kind: "shell"; key: string; x: number; y: number; w: number; h: number; casing: string }
  | null;
let dragSeq = 0;
function newDragId(): string {
  return `${getCurrentWindow().label}#${++dragSeq}`;
}
function emitGhost(dragId: string, g: Ghost) {
  const msg: GhostMsg = { from: getCurrentWindow().label, dragId, ghost: g, done: false };
  emit("desk-ghost", msg).catch(() => {});
}
/** 드래그 종료 — 조건 없이 부른다. 이 호출이 유령을 막는 유일한 보증이다. */
function endGhost(dragId: string) {
  const msg: GhostMsg = { from: getCurrentWindow().label, dragId, ghost: null, done: true };
  emit("desk-ghost", msg).catch(() => {});
}

/** 넘어오는 중인 것의 자리 표시. 실루엣만 빌려 쓴다 — 내용까지 그리면
    "이미 왔다"로 읽혀서 놓기 전인지 후인지 헷갈린다. */
/** 종료된 드래그 id — 늦게 도착한 미리보기를 걸러낸다(모듈 전역, 창마다 하나). */
const endedDrags = new Set<string>();

function GhostCard({ g }: { g: NonNullable<Ghost> }) {
  if (g.kind === "shell") {
    /* 실루엣을 손으로 그리지 않는다 — **실제 CassetteShell을 그대로 렌더**한다.
       손으로 그리면 마진 하나만 어긋나도 "다른 물건"으로 보이고, 셸 디자인이
       바뀔 때마다 여기도 같이 고쳐야 한다(2026-09-03 대조에서 실제로 어긋났다).
       화면 안은 비운다 — 내용까지 있으면 "이미 왔다"로 읽힌다. */
    return (
      <div style={{ position: "absolute", left: g.x, top: g.y, width: g.w,
        zIndex: 999, pointerEvents: "none", opacity: 0.72 }}>
        <CassetteShell casing={g.casing}><span /></CassetteShell>
        {/* 놓기 전이라는 표시 — 셸 위에 점선만 얹는다 */}
        <div style={{ position: "absolute", inset: 0, borderRadius: 15,
          border: "2px dashed var(--case-ink)", opacity: 0.85 }} />
      </div>
    );
  }
  const c = NOTE_COLORS[g.color];
  return (
    <div style={{
      position: "absolute", left: g.x, top: g.y, width: g.w, height: g.h, zIndex: 999,
      pointerEvents: "none", transformOrigin: "50% 0", transform: `rotate(${g.rot * CARD_ROT}deg)`,
      background: c.bg, opacity: 0.72,
      border: `2px dashed ${c.ink}`,
      boxShadow: "3px 5px 0 rgba(42,35,32,.14)",
    }}>
      <div style={{
        fontFamily: "var(--pixel)", fontSize: 12, color: c.ink, opacity: 0.75,
        padding: "13px 12px", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis",
      }}>
        {(g.title || "메모").replace(/<[^>]*>/g, "")}
      </div>
      <div style={{
        position: "absolute", left: 12, right: 12, bottom: 12, height: 1,
        background: c.ink, opacity: 0.3,
      }} />
    </div>
  );
}

/* ── Single draggable memo card ── */
function MemoCard({
  note, onChange, onRemove, onFront, onDragState, monKey, onHandoff, orphan,
}: {
  note: Note;
  onChange: (id: string, patch: Partial<Note>) => void;
  onRemove: (id: string) => void;
  onFront: (id: string) => void;
  onDragState: (dragging: boolean) => void;
  /** 이 창이 맡은 모니터 키. 화면을 넘기는 판정에 쓴다. */
  monKey: string | null;
  onHandoff: (id: string, target: { key: string; x: number; y: number }) => void;
  /** 뽑힌 모니터에서 넘어와 잠시 얹혀 있는 메모 — 여기서 끌면 이 화면 것이 된다. */
  orphan?: boolean;
}) {
  const c = NOTE_COLORS[note.color];
  const [draft, setDraft] = useState("");
  const [dragging, setDragging] = useState(false);
  /* 미리보기가 옆 화면에 떠 있는 동안 **원본은 감춘다.** 둘 다 보이면
     같은 메모가 두 장인 것처럼 읽힌다 — 넘어가는 중이라는 느낌이 깨진다. */
  const [gone, setGone] = useState(false);

  const startDrag = (e: React.PointerEvent) => {
    if ((e.target as HTMLElement).closest("button, input, [contenteditable], .mc-cb, .mc-sw")) return;
    e.preventDefault();
    // 포인터 캡처 — 커서가 카드 밖으로 나가도 이 요소가 계속 이벤트를 받는다.
    try { (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId); } catch { /* ignore */ }
    onFront(note.id);
    setDragging(true);
    onDragState(true);

    /* 좌표계가 두 개다.
       - **저장 좌표**: 그 메모가 속한 모니터 안에서의 위치.
       - **확장 좌표**: 드래그 중 창이 전 화면을 덮을 때의 창 안 위치.
       창이 늘어나는 건 `desk_drag_begin`이 끝난 뒤라 비동기다 — 매 move마다
       `spaceRef`를 다시 읽어 그 시점의 오프셋으로 계산한다. */
    const grabX = e.clientX - note.x;
    const grabY = e.clientY - note.y;
    const sx = grabX, sy = grabY;
    // 카드 안에서 어디를 잡았나 — 미리보기·착지 위치를 커서에 맞추는 데 쓴다.
    const grabDX = e.clientX - note.x;
    const grabDY = e.clientY - note.y;
    /* 미리보기 크기는 **실측**이다. 상수로 박으면 체크리스트가 긴 메모가
       최소 크기로 나와 "여기 놓인다"는 표시가 거짓말이 된다. */
    /* 크기는 `offsetWidth/Height` — **회전 전** 값이다. `getBoundingClientRect`는
       회전된 축정렬 외곽 상자라 실제보다 크고, 미리보기도 같은 각도로 회전하니
       두 번 부풀어 실물보다 커진다. */
    const cardEl = (e.target as HTMLElement).closest(".mc") as HTMLElement | null;
    const gw = cardEl?.offsetWidth ?? 190;
    const gh = cardEl?.offsetHeight ?? 92;

    /* 창 밖으로 나가면 **목적지 화면에 미리보기 메모**를 띄운다.
       창이 갈려 있어 실물은 경계에서 멈추지만, 어디에 놓일지가 보여야
       "옮길 수 있다"는 것도 알고 자리도 고를 수 있다(사용자 제안 2026-09-03). */
    let lastGhost = 0;
    const dragId = newDragId();
    let lastX = note.x, lastY = note.y;
    const move = (ev: PointerEvent) => {
      const nx = Math.round(Math.max(-40, Math.min(window.innerWidth - 60, ev.clientX - sx)));
      const ny = Math.round(Math.max(-20, Math.min(window.innerHeight - 40, ev.clientY - sy)));
      lastX = nx; lastY = ny;
      onChange(note.id, { x: nx, y: ny });

      const out =
        ev.clientX < 0 || ev.clientY < 0 ||
        ev.clientX > window.innerWidth || ev.clientY > window.innerHeight;
      if (!out) {
        setGone(false);
        emitGhost(dragId, null);   // 창 안으로 돌아오면 거둔다
        return;
      }
      setGone(true);
      const now = performance.now();
      if (now - lastGhost < 60) return;   // 전역 좌표는 IPC라 60ms로 조인다
      lastGhost = now;
      invoke<{ key: string; x: number; y: number } | null>("desk_cursor_target")
        .then((t) => {
          if (!t) return;
          // 잡았던 지점(grab offset)을 그대로 따라간다 — 미리보기와 실물이 같은 자리.
          emitGhost(dragId, { kind: "memo", key: t.key, x: Math.round(t.x - grabDX), y: Math.round(t.y - grabDY),
            w: gw, h: gh, color: note.color, title: note.title, rot: note.rot });
        })
        .catch(() => {});
    };

    const up = (ev: PointerEvent) => {
      setDragging(false);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      onDragState(false);
      endGhost(dragId);   // 조건 없이 — 늦게 오는 미리보기까지 여기서 무효가 된다
      // 창 밖에서 놓았으면 그 화면으로 넘긴다. 전역 변환은 Rust가 한다.
      const out =
        ev.clientX < -8 || ev.clientY < -8 ||
        ev.clientX > window.innerWidth + 8 || ev.clientY > window.innerHeight + 8;
      if (!out) {
        setGone(false);
        /* 얹혀 있던 메모를 이 화면에서 끌었다 = **이제 이 화면 것이다.**
           안 그러면 소속이 사라진 모니터로 남아, 다음 렌더에 다시 모임
           자리로 되돌아간다(끌어도 안 옮겨지는 것처럼 보인다). */
        if (orphan && monKey) onHandoff(note.id, { key: monKey, x: lastX, y: lastY });
        return;
      }
      /* **감춘 채로 둔다.** 여기서 되살리면 이동이 끝나기 전이라 원본이 원래
         화면에 한 프레임 번쩍인다(2026-09-03 실기). 넘기기가 성공하면 이
         카드는 목록에서 빠져 사라지고, 실패했을 때만 되살린다. */
      invoke<{ key: string; x: number; y: number } | null>("desk_cursor_target")
        .then((t) => {
          if (t && t.key !== monKey) onHandoff(note.id, { key: t.key, x: t.x - grabDX, y: t.y - grabDY });
          else setGone(false);
        })
        .catch(() => setGone(false));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const setItems = (items: NoteItem[]) => onChange(note.id, { items });
  const toggle = (id: string) => setItems(note.items.map((it) => (it.id === id ? { ...it, done: !it.done } : it)));
  const editItem = (id: string, t: string) => setItems(note.items.map((it) => (it.id === id ? { ...it, t } : it)));
  const removeItem = (id: string) => setItems(note.items.filter((it) => it.id !== id));
  // cmd+L — 일반 텍스트 ↔ 체크리스트. 레거시(chk 미지정)는 체크리스트였으므로
  // 첫 토글에서 일반 텍스트가 된다. 체크박스를 떼면 done도 함께 정리.
  const toggleCheck = (id: string) => setItems(note.items.map((it) => {
    if (it.id !== id) return it;
    const nowCheck = !(it.chk ?? true);
    return { ...it, chk: nowCheck, done: nowCheck ? it.done : false };
  }));
  const addItem = () => {
    const t = draft.trim();
    if (!t) return;
    setItems([...note.items, { id: uid(), t: sanitizeRich(t), done: false, chk: false }]);
    setDraft("");
  };

  // transform-origin at the tape pin (top-center), NOT the default center: a
  // note grows downward as its checklist changes, and center-origin rotation
  // would swing the whole card on every height change ("튐"). Pivoting at the
  // pin keeps the top fixed — growth only extends downward.
  return (
    <div
      className={"memo-card mc" + (dragging ? " drag" : "")}
      style={{ ["--note-ink" as string]: c.ink,
        left: note.x, top: note.y, width: 190, zIndex: note.z || 5,
        transformOrigin: "50% 0", transform: `rotate(${note.rot * CARD_ROT}deg)`,
        transition: dragging ? "none" : "transform .1s",
        // 미리보기가 옆 화면에 떠 있는 동안엔 원본을 감춘다.
        opacity: gone ? 0 : 1 }}
      onPointerDown={() => onFront(note.id)}
    >
      <div className="mc-tape" style={{ transform: `translateX(-50%) rotate(${note.rot > 0 ? -5 : 4}deg)` }} />
      <button className="mc-x" title="메모 삭제" onClick={() => onRemove(note.id)}>✕</button>
      <div className="mc-body" onPointerDown={startDrag}
        style={{ position: "relative", backgroundColor: c.bg, padding: "15px 13px 24px", color: c.ink,
          boxShadow: "3px 5px 0 rgba(42,35,32,.22)",
          backgroundImage: "repeating-linear-gradient(0deg, transparent 0 21px, rgba(42,35,32,0.05) 21px 22px)" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 7, marginBottom: 9 }}>
          <EditableText className={"mc-title" + (!(note.title || "").trim() ? " empty" : "")}
            initial={note.title || ""}
            onCommit={(t) => onChange(note.id, { title: t })} />
          <span style={{ flex: 1, height: 1, background: c.ink, opacity: 0.4 }} />
          <span style={{ display: "flex", flex: "0 0 auto" }}>
            <span style={{ width: 6, height: 6, background: "var(--rust)" }} />
            <span style={{ width: 6, height: 6, background: "var(--amber)" }} />
            <span style={{ width: 6, height: 6, background: "var(--teal)" }} />
          </span>
        </div>
        <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
          {note.items.map((it) => (
            <div key={it.id} className="mc-row" style={{ display: "flex", alignItems: "flex-start", gap: 7 }}>
              {(it.chk ?? true) ? (
                <button className={"mc-cb" + (it.done ? " on" : "")} onClick={() => toggle(it.id)}>
                  {it.done && (
                    <svg width="9" height="9" viewBox="0 0 10 10">
                      <polyline points="2,5.2 4,7.2 8,2.8" fill="none" stroke={c.bg} strokeWidth="1.8" strokeLinecap="square" />
                    </svg>
                  )}
                </button>
              ) : null}
              <EditableText className="mc-tx" initial={it.t}
                style={{ opacity: it.done ? 0.45 : 1, textDecoration: it.done ? "line-through" : "none" }}
                onCommit={(t) => editItem(it.id, t)}
                onEmptyBlur={() => removeItem(it.id)}
                onToggleCheck={() => toggleCheck(it.id)} />
              <button className="mc-rm" title="삭제" onClick={() => removeItem(it.id)}>✕</button>
            </div>
          ))}
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 7, marginTop: 6, borderTop: `1px dashed ${c.ink}55`, paddingTop: 6 }}>
          <span style={{ fontFamily: "var(--pixel)", fontSize: 12, color: c.ink, opacity: 0.5 }}>＋</span>
          <input className="mc-add" value={draft} placeholder={`내용 추가 (${MOD_KEY}L 체크박스)`}
            onPointerDown={grabKeyFocus}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") addItem(); }} />
        </div>
      </div>
      <div className="mc-swatches">
        {PALETTE.map((p) => (
          <span key={p} className={"mc-sw" + (p === note.color ? " on" : "")}
            style={{ background: NOTE_COLORS[p].bg }} title={p}
            onClick={() => onChange(note.id, { color: p })} />
        ))}
      </div>
    </div>
  );
}

// v2: store the shell's top-left as a FRACTION of the viewport (0..1), not
// absolute px. Absolute px broke when moving to a different-resolution monitor
// or restarting there — the pixel spot no longer meant the same place. A
// fraction re-projects to the right spot at any resolution. (v1 absolute keys
// are ignored; the shell just re-centers once, then persists as a fraction.)
// Shell display size. The cassette is designed at 300px; the user found that a
// bit big, so render at 80%. CSS scale (not a dimension refactor) keeps it one
// knob — click-through/drag/gaze all read getBoundingClientRect, which already
// reflects the transform, so they stay aligned. Downscale on retina is
// supersampled so the bitmap font stays acceptably crisp.
// 셸 스케일 없음 (2026-08-10). 과거 scale(0.8)로 크기를 줄였는데, 그게
// 아래 NOTE가 금지한 바로 그 짓이었다: 11px 글자가 8.8px로, 3px 스캔라인이
// 2.4px로 렌더돼 1x 디스플레이에서 지글거림(모아레)을 만들었다. 크기는
// 셸 내부 치수를 직접 줄여서 맞춘다(TamagotchiShell CaseFrame).


export function MemoDesk({
  lv, hungry, hasCoaching,
}: {
  lv: number; hungry: boolean; hasCoaching: boolean;
}) {
  const stageRef = useRef<HTMLDivElement | null>(null);
  const shellWrapRef = useRef<HTMLDivElement | null>(null);
  const zTop = useRef(20);
  const draggingRef = useRef(false);
  /* M7: 이 창이 맡은 모니터와, 지금 붙어 있는 모니터 목록.
   *
   * **한 소스에서 같이 받는다.** 예전엔 자기 모니터를 마운트 때 한 번만
   * 물어봤는데(`desk_monitor`), 모니터를 뽑으면 창 라벨이 재배치되면서
   * 그 답이 낡는다 — 창은 여전히 사라진 모니터를 자기 것이라 믿고, 그
   * 모니터의 메모를 자기 소유로 그린다. 그래서 C를 뽑았을 때 A와 B에
   * 메모가 **둘 다** 떴다(2026-09-04). 목록을 주기적으로 받아 거기서
   * 자기 라벨을 찾으면 항상 최신이고 소스도 하나다.
   *
   * 프리뷰(브라우저)에는 창이 없어 목록이 비는데, 그때는 "모니터 구분 없음"
   * 으로 취급해 전부 보여준다.
   */
  const selfLabel = useMemo(() => {
    try { return getCurrentWindow().label; } catch { return null; }
  }, []);
  const [monList, setMonList] = useState<MonitorInfo[]>([]);
  /* 자기 모니터를 **알기 전**에는 어떤 메모도 소유하지 않는다.
     예전엔 `monKey`가 null이면 "전부 내 것"으로 쳤는데, 그게 부팅 때마다
     메모를 옮겨 놓는 진범이었다(2026-09-03 실측): 창은 기본 크기로 만들어졌다가
     자기 모니터 크기로 맞춰지는데, 그 리사이즈가 응답보다 먼저 오면 세로 델
     창이 **주 모니터 메모까지** 자기 비율로 재배치했다. */
  const [monReady, setMonReady] = useState(false);
  useEffect(() => {
    let alive = true;
    const pull = () => {
      invoke<MonitorInfo[]>("desk_monitors")
        .then((l) => { if (alive) setMonList(l ?? []); })
        .catch(() => { if (alive) setMonList([]); })
        .finally(() => { if (alive) setMonReady(true); });
    };
    pull();
    // Rust의 창 동기화와 같은 주기 — 모니터를 꽂고 뽑는 걸 3초 안에 따라간다.
    const iv = window.setInterval(pull, 3000);
    return () => { alive = false; window.clearInterval(iv); };
  }, []);
  const mon = useMemo(
    () => monList.find((m) => m.window === selfLabel) ?? null,
    [monList, selfLabel],
  );
  const monKey = mon?.key ?? null;
  const knownKeys = useMemo(() => new Set(monList.map((m) => m.key)), [monList]);

  const [desk, deskReady] = useDesk();
  /* 이 창이 그릴 메모만. `mon`이 없는 레거시 메모는 **주 모니터** 몫이다 —
     v4에는 모니터 개념이 없었으니 어딘가 하나로 몰아야 하고, 주 모니터가
     사용자가 마지막으로 본 화면일 확률이 가장 높다. */
  /* 창의 **실제** 크기. 부팅 직후엔 작다(tauri.conf.json의 328×340) — 창이
     자기 모니터로 맞춰지기 전이다. */
  const [live, setLive] = useState(() => ({ w: window.innerWidth, h: window.innerHeight }));
  /* 좌표를 펼 기준은 **모니터 논리 크기**다. 창 크기를 쓰면 부팅 직후 한 번
     반드시 틀린다 — 창은 나중에 맞춰지기 때문이다. */
  const vp = mon ? { w: mon.width / mon.scale, h: mon.height / mon.scale } : live;
  /* 창이 아직 자기 모니터 크기가 아니면 **그리지 않는다.**
     기준이 옳아도 창이 작으면 셸이 창 밖(오른쪽)에 놓였다가 창이 커질 때
     나타난다 — 사용자가 본 "튐"의 나머지 절반이다. 부팅 328 vs 모니터 2056은
     16%라 60% 문턱이 깔끔히 가른다. */
  const geomReady = !mon || live.w >= (mon.width / mon.scale) * 0.6;

  const notes = useMemo(() => {
    if (!monReady || !geomReady) return [];   // 모니터를 모르거나 창이 아직 안 맞았으면 그리지 않는다
    const mine = monKey
      ? desk.notes.filter((n) => claimsNote(n, monKey, !!mon?.primary, knownKeys))
      : desk.notes;   // 프리뷰(창 없음) — 전부 보여준다
    /* 화면 밖 메모는 잡을 수 없다 — 저장값은 두고 그릴 때만 안으로 당긴다.
       해상도가 줄었거나(모니터 교체) 좌표가 어긋난 경우의 안전망이다.

       기준은 **모니터 논리 크기**다. 창 크기를 쓰면 부팅 직후 1~2초 동안
       엉뚱한 자리에 그려진다 — 창은 328×340으로 태어나 자기 모니터로 맞춰지는데,
       그 사이 클램프가 메모를 전부 좌상단으로 끌어당겼다가 창이 커지면
       제자리로 튀어 돌아온다(2026-09-04 사용자 관찰). 모니터 크기는 그
       중간 단계와 무관하게 처음부터 옳다. */
    const w = mon ? mon.width / mon.scale : window.innerWidth;
    const h = mon ? mon.height / mon.scale : window.innerHeight;
    /* 뽑힌 모니터에서 넘어온 메모는 **찾기 쉬운 자리에 모은다.** 원래 좌표를
       그대로 쓰면 다른 크기의 화면에서 온 값이라 구석에 흩어진다 — 셸은
       중앙으로 오는데 메모만 변두리에 있으면 "왔다"는 게 안 읽힌다.
       셸(240×300)을 가리지 않게 중앙 오른쪽 아래로 계단식. **표시 전용이라
       저장값은 그대로**이고, 이 화면에서 끌면 그때 이 모니터 것이 된다. */
    let orphanN = 0;
    return mine.map((n) => {
      if (isOrphanNote(n, knownKeys)) {
        const i = orphanN++;
        const ox = w / 2 + 150 + i * 20;
        const oy = h / 2 - 130 + i * 26;
        return {
          ...n,
          x: Math.round(Math.max(8, Math.min(w - 200, ox))),
          y: Math.round(Math.max(8, Math.min(h - 60, oy))),
          _orphan: true as const,
        };
      }
      const x = Math.max(-40, Math.min(w - 60, n.x));
      const y = Math.max(-20, Math.min(h - 40, n.y));
      return x === n.x && y === n.y ? n : { ...n, x, y };
    });
  }, [desk.notes, monKey, mon, monReady, geomReady, knownKeys]);
  /* **정체성이 변하지 않는다**(deps `[]`). 소유 정보는 ref로 읽는다.
     의존성에 넣었더니 `setNotes`가 매번 새 함수가 됐고, 이걸 `[]`로 붙잡은
     `change`/`remove`/`front`가 **`monReady=false` 시절의 것을 영구히 들고**
     있어 모든 편집이 조용히 버려졌다(2026-09-04: 메모 드래그가 죽었다).
     리스너 누수(M7)와 같은 낡은 클로저 계열이다 — 이번엔 아예 안 변하게 만든다. */
  const ownRef = useRef({
    ready: false, key: null as string | null, primary: false, known: new Set<string>(),
  });
  ownRef.current = { ready: monReady, key: monKey, primary: !!mon?.primary, known: knownKeys };
  const setNotes = useCallback((upd: Note[] | ((ns: Note[]) => Note[])) => {
    const { ready, key, primary, known } = ownRef.current;
    if (!ready) return;   // 소유를 모르는 채로 쓰지 않는다 — 남의 메모를 옮긴다
    setDesk((st) => {
      const mine = key ? st.notes.filter((n) => claimsNote(n, key, primary, known)) : st.notes;
      const others = st.notes.filter((n) => !mine.includes(n));
      const next = (typeof upd === "function" ? upd(mine) : upd).map((n) =>
        key ? { ...n, mon: n.mon ?? key } : n,
      );
      return { ...st, notes: [...others, ...next] };
    });
  }, []);
  // Shell position as a viewport fraction (null = centered). Draggable from its
  // case body; persisted as a fraction so it survives resolution changes.
  /* 셸은 하나뿐이다. `shellPos.mon`이 이 창의 모니터일 때만 그린다.
     레거시(mon 없음)는 주 모니터가 갖는다. */
  /* 셸이 화면 밖으로 나가면 잡을 방법이 없다 — 되찾을 수단이 없는 상태는
     만들지 않는다(2026-09-03: 좌표가 fy=-1.28까지 나가 손으로 파일을 고쳐야
     했다). 저장값은 건드리지 않고 **그릴 때만** 화면 안으로 당긴다. */
  const shellPos = useMemo(() => {
    const p = desk.shellPos;
    if (!p) return null;
    const clamp = (v: number) => Math.max(-0.02, Math.min(0.92, v));
    return { ...p, fx: clamp(p.fx), fy: clamp(p.fy) };
  }, [desk.shellPos]);
  const shellMon = shellPos?.mon ?? (mon?.primary ? monKey : null);
  /* 셸이 묶인 모니터가 지금 없으면 **주 모니터가 떠맡는다.**
     셸은 앱 본체다 — 모니터를 뽑았다고 어디에서도 못 찾으면 앱을 못 쓴다
     (2026-09-04: C 모니터를 뽑자 셸이 실종됐다). 저장된 `mon`은 그대로 두므로
     그 모니터를 다시 꽂으면 원래 자리로 돌아간다. 여기서 끌면 그때 주인이 바뀐다.
     메모는 이 대접을 안 한다 — 여러 장이 주 화면에 쏟아지면 그게 더 혼란이고,
     메모는 없어도 앱을 쓸 수 있다. */
  const shellOrphan = !!shellMon && knownKeys.size > 0 && !knownKeys.has(shellMon);
  /* **자기 모니터를 알기 전에는 셸도 그리지 않는다.**
     예전엔 monKey가 null이면 일단 그렸는데, 그 시점의 기준 크기는 어림값이라
     셸이 잠깐 엉뚱한 자리에 놓였다가 제자리로 튀었다(2026-09-04 사용자 관찰).
     메모는 이미 `monReady`로 막혀 있었고, 셸만 새어 나가고 있었다.
     감지는 밀리초 단위라 사용자에겐 "그냥 제자리에 뜬다"로 보인다. */
  const hasShell =
    monReady && geomReady &&
    (shellOrphan ? !!mon?.primary : (!monKey || shellMon === monKey));
  // setNotes와 같은 이유로 정체성을 고정한다.
  const setShellPos = useCallback((upd: ShellPos | null | ((p: ShellPos | null) => ShellPos | null)) => {
    setDesk((st) => {
      const next = typeof upd === "function" ? upd(st.shellPos) : upd;
      const key = ownRef.current.key;
      return { ...st, shellPos: next ? { ...next, mon: next.mon ?? key ?? undefined } : null };
    });
  }, []);
  // Live viewport size — drives fraction→px projection, and reprojects notes
  // when the resolution changes. Seed from the MONITOR size (screen.*), not the
  // possibly-tiny boot window: the desk always ends up fullscreen, so seeding
  // from innerWidth (small at launch, before App resizes to fullscreen)
  // projected the saved shell fraction to the wrong spot for a frame, which
  // looked like "the position wasn't saved". max() picks whichever is larger.

  // NOTE: no CSS `transform: scale` for a consistent "크기감" — Galmuri is a
  // bitmap font with smoothing OFF (tokens.css), so a non-integer container
  // scale (e.g. 1.2 at 1920×1080) blurs the text and sprites, which is the
  // opposite of the "일관되게 표현" the user wants. Crisp size scaling means
  // stepping the integer sprite scale + font sizes — a separate change.

  /* 창 크기를 따라간다 — 셸의 분수 좌표를 px로 투영하는 데 쓴다.
   *
   * **메모를 비율로 재배치하지 않는다.** v4엔 "해상도가 바뀌면 메모를 같은
   * 상대 위치로 옮긴다"가 있었는데, 그건 창이 하나뿐이고 그 창이 곧 모니터였던
   * 시절의 장치다. M7부터 창은 기본 크기로 만들어졌다가 자기 모니터로 맞춰지고,
   * 그 중간 리사이즈가 매번 "해상도 변경"으로 읽혀 **부팅마다 메모가 옮겨졌다**
   * (2026-09-03 실측: 재시작 한 번에 x×0.5625, y×1.7778).
   * 이제 메모 좌표는 모니터 키에 매인 상대값이고, 화면 밖으로 나간 것은
   * 그릴 때 안으로 당기는 안전망이 잡는다(위 `notes` useMemo). */
  useEffect(() => {
    const onResize = () => setLive({ w: window.innerWidth, h: window.innerHeight });
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  /* 다른 창에서 넘어오는 중인 미리보기. 내 화면이 목적지일 때만 그린다.
   *
   * **리스너는 창 생애에 딱 하나다.** 예전엔 `[monKey]`에 걸어뒀는데,
   * `listen()`이 해제 함수를 비동기로 주는 탓에 monKey가 null→값으로 바뀌는
   * 순간 이전 리스너를 못 걷고 둘이 같이 살았다. 낡은 쪽(monKey=null)이 뒤에
   * 실행되면 방금 띄운 미리보기를 도로 지운다 — "특정 화면에서만 안 보인다"의
   * 정체가 이거였다(2026-09-03 로그로 확인: 같은 메시지가 match=true 한 번,
   * match=false 한 번). 최신 monKey는 ref로 읽는다.
   */
  const [ghost, setGhost] = useState<(NonNullable<Ghost> & { dragId: string }) | null>(null);
  /* 셸 미리보기를 목적지 화면에서 같은 색으로 그리려면 케이스 색이 필요하다.
     설정을 두 창이 각자 읽는 대신 신호에 실어 보낸다. */
  const settingsCasing = useRef("beige");
  useEffect(() => {
    invoke<{ case_color?: string }>("get_settings")
      .then((st) => { settingsCasing.current = st?.case_color || "beige"; })
      .catch(() => {});
  }, []);

  // 셸이 옆 화면으로 넘어가는 중 — 원본을 감춘다(메모와 같은 규칙).
  const [shellGone, setShellGone] = useState(false);
  const monKeyRef = useRef<string | null>(null);
  monKeyRef.current = monKey;
  useEffect(() => {
    let cancelled = false;
    let un: (() => void) | undefined;
    listen<GhostMsg>("desk-ghost", (e) => {
      const { from, dragId, ghost: g, done } = e.payload;
      if (from === getCurrentWindow().label) return;  // 보낸 창은 실물이 있다
      if (done) {
        endedDrags.add(dragId);
        if (endedDrags.size > 40) {
          for (const k of Array.from(endedDrags).slice(0, 20)) endedDrags.delete(k);
        }
        setGhost((cur) => (cur && cur.dragId === dragId ? null : cur));
        return;
      }
      if (endedDrags.has(dragId)) return;   // 놓은 뒤 늦게 온 미리보기
      const me = monKeyRef.current;
      setGhost(g && me && g.key === me ? { ...g, dragId } : null);
    }).then((f) => {
      // 등록이 끝나기 전에 언마운트됐으면 곧바로 걷는다.
      if (cancelled) f(); else un = f;
    }).catch(() => {});
    return () => { cancelled = true; un?.(); };
  }, []);

  /* 첫 열기: 파일 정본이 비어 있으면 (1) v4 localStorage를 승계하고
     (2) 그래도 없으면 예시 메모를 심는다. 승계가 시드보다 먼저다 —
     사용자의 진짜 메모 위에 데모를 덮으면 안 된다. */
  const seeded = useRef(false);
  useEffect(() => {
    if (!deskReady || seeded.current || !monKey) return;
    if (desk.notes.length || desk.shellPos) {
      seeded.current = true;
      clearLegacyDesk();
      // v4에서 넘어온 메모엔 모니터가 없다. **주 모니터 창이 한 번 도장을 찍는다** —
      // 안 찍으면 나중에 주 모니터가 바뀌는 순간 메모가 통째로 다른 화면으로 뛴다.
      // 1.0.2 는 같은 기종 둘일 때 주 모니터 키에도 `#1` 을 붙였다. 이제 주 모니터는
      // 접미사 없는 키라, 그때 찍힌 `base#n` 이 지금 안 보이면 **주 창이 거둬 간다**
      // — 안 그러면 그 메모·셸이 영영 고아다(2026-09-11 판정 수정의 후속).
      const stale = (k: string | undefined) =>
        !!k && !knownKeys.has(k) && k.replace(/#\d+$/, "") === monKey;
      if (
        mon?.primary &&
        (desk.notes.some((n) => !n.mon || stale(n.mon)) || (desk.shellPos && (!desk.shellPos.mon || stale(desk.shellPos.mon))))
      ) {
        setDesk((st) => ({
          notes: st.notes.map((n) => (n.mon && !stale(n.mon) ? n : { ...n, mon: monKey })),
          shellPos: st.shellPos
            ? { ...st.shellPos, mon: st.shellPos.mon && !stale(st.shellPos.mon) ? st.shellPos.mon : monKey }
            : null,
        }));
      }
      return;
    }
    seeded.current = true;
    const legacy = migrateLegacyDesk();
    if (legacy) {
      setDesk((st) => ({
        notes: legacy.notes.map((n) => ({ ...n, mon: n.mon ?? monKey ?? undefined })),
        shellPos: legacy.shellPos
          ? { ...legacy.shellPos, mon: legacy.shellPos.mon ?? monKey ?? undefined }
          : st.shellPos,
      }));
      clearLegacyDesk();
      return;
    }
    const r = stageRef.current?.getBoundingClientRect();
    const w = r?.width ?? 900, h = r?.height ?? 640;
    setNotes([
      { id: uid(), x: Math.round(Math.max(20, w / 2 - 340)), y: Math.round(Math.max(60, h * 0.30)), rot: -3, color: "manila", z: 6, title: "TODAY",
        items: [{ id: uid(), t: "토키 밥 주기", done: true }, { id: uid(), t: "리팩터 PR 리뷰", done: false }] },
      { id: uid(), x: Math.round(Math.min(w - 210, w / 2 + 170)), y: Math.round(Math.max(80, h * 0.4)), rot: 3, color: "mint", z: 6, title: "이번 주",
        items: [{ id: uid(), t: "펫 12종 정리", done: false }, { id: uid(), t: "뽀모 4세션", done: false }] },
    ]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deskReady, desk.notes.length, desk.shellPos, monKey, knownKeys]);

  /* 메모를 다른 화면으로 넘긴다. 좌표는 목적지 화면 기준으로 바뀌므로
     `mon`과 `x/y`를 같이 갈아야 한다 — 하나만 바꾸면 엉뚱한 자리에 나타난다.
     메모 폭의 절반을 빼서 커서가 쥔 지점 근처에 놓이게 한다. */
  const handoffNote = useCallback((id: string, t: { key: string; x: number; y: number }) => {
    // 즉시 저장 — 목적지 창이 곧바로 그려야 원본도 미리보기도 없는 틈이 안 생긴다.
    setDesk((st) => ({
      ...st,
      notes: st.notes.map((n) =>
        n.id === id
          ? { ...n, mon: t.key, x: Math.round(t.x), y: Math.round(t.y), z: ++zTop.current }
          : n,
      ),
    }), true);
  }, []);

  const change = useCallback((id: string, patch: Partial<Note>) =>
    setNotes((ns) => ns.map((n) => (n.id === id ? { ...n, ...patch } : n))), []);
  const remove = useCallback((id: string) => setNotes((ns) => ns.filter((n) => n.id !== id)), []);
  const front = useCallback((id: string) =>
    setNotes((ns) => ns.map((n) => (n.id === id ? { ...n, z: ++zTop.current } : n))), []);

  // New memos appear neatly beside the shell — alternating right/left,
  // cascading down each side (not random). Placed relative to the shell's
  // *current* position so it follows the shell after a drag.
  const addMemo = () => {
    const shell = shellWrapRef.current?.getBoundingClientRect();
    if (!shell) return;
    const NW = 190, gap = 22, step = 46;
    setNotes((ns) => {
      const idx = ns.length;
      const right = idx % 2 === 0;          // 0→right, 1→left, 2→right, …
      const sideIdx = Math.floor(idx / 2);  // 0,1,2… down each side
      const x = right ? shell.right + gap : shell.left - NW - gap;
      const y = shell.top + sideIdx * step;
      const cx = Math.max(8, Math.min(window.innerWidth - 60, x));
      const cy = Math.max(8, Math.min(window.innerHeight - 60, y));
      return [...ns, {
        id: uid(), x: Math.round(cx), y: Math.round(cy), rot: right ? 2 : -2, color: PALETTE[idx % 4],
        z: ++zTop.current, title: "메모", items: [],
      }];
    });
  };

  /* 온보딩 표시 여부 — DOM 속성 하나로 전달받는다. MemoDesk와 온보딩은
     형제라 props로 엮으면 App까지 배선이 번지고, 이 신호는 "지금 모달이 떠
     있나" 한 비트가 전부다. */
  const [onboarding, setOnboarding] = useState(
    () => document.documentElement.dataset.onboarding === "1",
  );
  useEffect(() => {
    const mo = new MutationObserver(() =>
      setOnboarding(document.documentElement.dataset.onboarding === "1"),
    );
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-onboarding"] });
    return () => mo.disconnect();
  }, []);

  /* ── multi-rect click-through ── */
  const reportRects = useCallback(() => {
    if (draggingRef.current) {
      // 드래그 중엔 창을 **무조건** 조작 가능으로 묶는다.
      // 창 크기만큼만 잡아두면, 커서가 창 밖(=옆 모니터)으로 나가는 순간
      // Rust 폴링이 "사각형 밖"으로 판정해 클릭스루로 돌린다 → 창이 마우스를
      // 못 받아 드래그가 그 자리에서 죽는다. 화면을 넘기는 드래그가 애초에
      // 성립하지 않았던 이유다(2026-09-03). 전역 좌표를 통째로 덮는다.
      // Rects는 CSS/논리 px — Rust가 커서를 같은 공간으로 환산해 비교한다.
      invoke("set_click_rects", { rects: [[-100000, -100000, 200000, 200000]] }).catch(() => {});
      return;
    }
    const rects: number[][] = [];
    const push = (el: Element | null | undefined) => {
      if (!el) return;
      const r = el.getBoundingClientRect();
      rects.push([r.left, r.top, r.width, r.height]);
    };
    // 온보딩 중엔 셸·메모를 클릭 렉트에서 뺀다 — 얼러트만 조작 가능해야 한다.
    if (!onboarding) {
      // 셸이 없는 창에서는 shellWrapRef가 비어 있다(push가 알아서 무시).
      push(shellWrapRef.current);
      document.querySelectorAll(".memo-card").forEach(push);
    }
    document.querySelectorAll(".desk-ctl").forEach(push);
    invoke("set_click_rects", { rects }).catch(() => {});
  }, [onboarding]);

  /* 드래그 중 창을 전 화면으로 늘리는 방식은 **철회했다**(2026-09-03 실기).
     창이 커지는 순간 기존 "해상도 바뀜" 처리가 메모 좌표를 비례 확대하고
     셸은 뷰포트 분수라 같이 튄다 — 데스크의 좌표 규약과 정면으로 부딪힌다.
     Rust 쪽 `desk_drag_begin`은 남아 있지만 호출하지 않는다. */
  const onDragState = useCallback((dragging: boolean) => {
    draggingRef.current = dragging;
    reportRects();
  }, [reportRects]);

  // Drag the shell by its case body (not the soft buttons or the memo pad).
  const startShellDrag = (e: React.PointerEvent) => {
    // 팝업(얼러트·리포트 모달)은 createPortal로 body에 붙지만 React 합성
    // 이벤트는 DOM이 아니라 React 트리를 따라 올라와 여기까지 닿는다.
    // 그러면 팝업 타이틀바를 끌어도, 본문을 긁어도 셸이 움직인다 (2026-09-02).
    // 포인터가 셸의 실제 DOM 위에 떨어진 경우만 셸 드래그로 친다.
    const wrap = shellWrapRef.current;
    if (!wrap || !wrap.contains(e.target as Node)) return;
    if ((e.target as HTMLElement).closest(".cf-soft, .pad")) return;
    e.preventDefault();
    onDragState(true);
    const rect = shellWrapRef.current!.getBoundingClientRect();
    const ox = e.clientX - rect.left;
    const oy = e.clientY - rect.top;
    const gw = Math.round(rect.width), gh = Math.round(rect.height);
    // 셸도 같은 규칙 — 창 밖으로 나가면 목적지 화면에 자리 표시가 따라다니고,
    // 놓으면 그 화면으로 넘어간다.
    let lastGhost = 0;
    const dragId = newDragId();
    const move = (ev: PointerEvent) => {
      const nx = Math.round(Math.max(-40, Math.min(window.innerWidth - 60, ev.clientX - ox)));
      const ny = Math.round(Math.max(-10, Math.min(window.innerHeight - 40, ev.clientY - oy)));
      setShellPos({ fx: nx / (window.innerWidth || 1), fy: ny / (window.innerHeight || 1), mon: monKey ?? undefined });

      const out =
        ev.clientX < 0 || ev.clientY < 0 ||
        ev.clientX > window.innerWidth || ev.clientY > window.innerHeight;
      if (!out) {
        setShellGone(false);
        emitGhost(dragId, null);
        return;
      }
      setShellGone(true);
      const now = performance.now();
      if (now - lastGhost < 60) return;
      lastGhost = now;
      invoke<{ key: string; x: number; y: number } | null>("desk_cursor_target")
        .then((t) => {
          if (!t) return;
          emitGhost(dragId, { kind: "shell", key: t.key, x: Math.round(t.x - ox), y: Math.round(t.y - oy),
            w: gw, h: gh, casing: settingsCasing.current });
        })
        .catch(() => {});
    };
    const up = (ev: PointerEvent) => {
      onDragState(false);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      endGhost(dragId);
      const out =
        ev.clientX < -8 || ev.clientY < -8 ||
        ev.clientX > window.innerWidth + 8 || ev.clientY > window.innerHeight + 8;
      if (!out) { setShellGone(false); return; }
      invoke<{ key: string; x: number; y: number } | null>("desk_cursor_target")
        .then((t) => {
          if (!t || t.key === monKey) { setShellGone(false); return; }
          invoke<MonitorInfo[]>("desk_monitors").then((all) => {
            const dest = all.find((m) => m.key === t.key);
            if (!dest) { setShellGone(false); return; }
            const w = dest.width / dest.scale, h = dest.height / dest.scale;
            // 잡았던 지점을 그대로 따라간다 — 미리보기와 같은 자리에 앉게.
            setDesk((st) => ({
              ...st,
              shellPos: { mon: t.key, fx: (t.x - ox) / w, fy: (t.y - oy) / h },
            }), true);
            /* 넘긴 뒤 **반드시** 감춤을 푼다. 셸은 메모와 달리 창에 계속 사는
               요소라 상태가 남는다 — 안 풀면 나중에 셸이 이 화면으로 돌아왔을 때
               투명한 채로 있어 "사라진" 것처럼 보인다(2026-09-03 실기 b→a).
               이 시점엔 이미 다른 화면 소유라 여기선 그려지지 않으므로 번쩍임도 없다. */
            setShellGone(false);
          }).catch(() => {});
        })
        .catch(() => {});
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  // P3-11: report the opaque rects when they actually CHANGE, not on a 300ms
  // poll. Rust only needs the rect list kept current (it polls the cursor), so
  // re-reporting when nothing moved was pure waste. Change sources: memo
  // add/remove/move + content (notes), shell position (shellPos), viewport
  // (vp/resize), and — the one not tied to MemoDesk state — the shell's height
  // changing on mode navigation, caught by a ResizeObserver on its wrapper. A
  // slow 2s net backstops any unforeseen reflow (font load, etc.).
  useEffect(() => {
    reportRects();
    window.addEventListener("resize", reportRects);
    const ro = new ResizeObserver(() => reportRects());
    if (shellWrapRef.current) ro.observe(shellWrapRef.current);
    const net = window.setInterval(reportRects, 2000);
    // Shell modals (coach report / retro report / alert) portal .desk-ctl
    // overlays into document.body — without this observer they'd only become
    // clickable on the next 2s net tick (dead close/copy buttons meanwhile).
    const mo = new MutationObserver(() => reportRects());
    mo.observe(document.body, { childList: true });
    return () => {
      window.removeEventListener("resize", reportRects);
      ro.disconnect();
      mo.disconnect();
      window.clearInterval(net);
    };
  }, [reportRects]);
  useEffect(() => { reportRects(); }, [reportRects, notes, shellPos, vp]);

  return (
    /* 온보딩이 떠 있는 동안 데스크는 **비활성**이다(`data-onboarding`).
       안 그러면 사용자가 뒤의 셸을 눌러 포커스를 가져가고, 얼러트로 돌아올
       길이 없어 앱을 강제 종료해야 했다(2026-09-03 실기). inert는 포인터·
       포커스·접근성 트리를 한꺼번에 막는다. */
    <div ref={stageRef} inert={onboarding || undefined}
      style={{ position: "fixed", inset: 0, overflow: "hidden", background: "transparent" }}>
      {/* cassette shell + the memo-pad stack on its top edge. Draggable by its
          case body. Desk owns click-through, so the shell doesn't self-report.
          M7: 셸은 **하나뿐**이라 소유 모니터의 창에서만 그린다. */}
      {hasShell && (
      <div className="shell-wrap"
        onPointerDown={startShellDrag}
        style={shellPos && !shellOrphan
          ? { position: "absolute", left: Math.round(shellPos.fx * vp.w), top: Math.round(shellPos.fy * vp.h),
              cursor: "grab", opacity: shellGone ? 0 : 1 }
          : { position: "absolute", left: "50%", top: "50%",
              transform: "translate(-50%,-50%)", cursor: "grab", opacity: shellGone ? 0 : 1 }}>
        {/* post-it stack — press to peel off a new memo (design: MemoPad) */}
        <div className="desk-ctl pad" title="눌러서 새 메모 붙이기" onClick={addMemo}
          style={{ position: "absolute", top: -18, right: 34, width: 72, height: 20, zIndex: 3, cursor: "pointer" }}>
          <div style={{ position: "absolute", inset: 0,
            background: "repeating-linear-gradient(0deg, rgba(60,80,55,0.16) 0 1px, transparent 1px 3px), linear-gradient(180deg, #dcead2 0%, #cfe0c5 40%, #bcd0b0 100%)",
            boxShadow: "inset 0 1px 0 rgba(255,255,255,.55), inset 0 -1px 0 rgba(0,0,0,.12), 0 1px 2px rgba(0,0,0,.14)" }} />
        </div>
        <div ref={shellWrapRef}>
          <TamagotchiShell lv={lv} hungry={hungry} hasCoaching={hasCoaching} reportClickRects={false} />
        </div>
      </div>
      )}

      {/* 확장 중엔 창 원점이 옮겨졌으므로 렌더 좌표에 오프셋을 더한다. */}
      {/* 넘어오는 중인 메모 — 놓으면 여기 앉는다는 표시. 클릭은 통과시킨다. */}
      {ghost && <GhostCard g={ghost} />}

      {notes.map((n: Note) => (
        <MemoCard key={n.id} note={n} monKey={monKey} orphan={(n as { _orphan?: boolean })._orphan}
          onChange={change} onRemove={remove} onFront={front} onDragState={onDragState}
          onHandoff={handoffNote} />
      ))}

      <style>{`
        /* No user-select:none on .mc itself — it's an ancestor of the
           editable title/items, which WKWebView refuses to caret into
           under a none-ancestor. Drag-selection is already prevented by
           startDrag's preventDefault; only the non-text controls opt out. */
        .mc { position: absolute; touch-action: none; }
        .mc-tape, .mc-x, .mc-rm, .mc-cb, .mc-sw, .mc-swatches { -webkit-user-select: none; user-select: none; }
        .mc .mc-body { cursor: grab; }
        .mc.drag .mc-body { cursor: grabbing; }
        .mc:hover { z-index: 50 !important; }
        .mc:hover .mc-x, .mc:hover .mc-swatches { opacity: 1; }
        .mc-tape { position: absolute; top: -11px; left: 50%; width: 58px; height: 20px; z-index: 5; pointer-events: none;
          background: repeating-linear-gradient(90deg, rgba(228,214,176,.66) 0 5px, rgba(214,198,156,.66) 5px 6px);
          box-shadow: inset 0 0 0 1px rgba(255,255,255,.28), 0 1px 2px rgba(0,0,0,.14); }
        .mc-x { position: absolute; top: 6px; right: 7px; z-index: 6; width: 16px; height: 16px; border: none; cursor: pointer;
          background: transparent; color: var(--note-ink); opacity: 0; font-family: var(--pixel); font-size: 12px;
          line-height: 1; display: flex; align-items: center; justify-content: center; transition: opacity .12s; }
        .mc-x:hover { background: rgba(0,0,0,.12); }
        .mc-swatches { position: absolute; left: 9px; bottom: 7px; z-index: 6; display: flex; gap: 5px; opacity: 0; transition: opacity .12s; }
        .mc-sw { width: 12px; height: 12px; border-radius: 50%; cursor: pointer; box-shadow: inset 0 0 0 1px rgba(0,0,0,.25); }
        .mc-sw.on { box-shadow: inset 0 0 0 1px rgba(0,0,0,.25), 0 0 0 2px rgba(0,0,0,.4); }
        .mc-cb { flex: 0 0 auto; width: 14px; height: 14px; border: 1.6px solid var(--note-ink); cursor: pointer;
          background: transparent; padding: 0; margin-top: 1px; display: flex; align-items: center; justify-content: center; }
        .mc-cb.on { background: var(--note-ink); }
        .mc-tx { flex: 1; font-family: var(--pixel); font-size: 12px; line-height: 18px; color: var(--note-ink);
          outline: none; word-break: break-word; cursor: text; }
        .mc-tx:empty:before { content: "할 일…"; color: rgba(0,0,0,.32); }
        /* html/body + .mc set user-select:none (drag-ghost kill); in WKWebView
           that also blocks contentEditable focus entirely — the title looked
           editable in Chromium previews but was dead in the native app.
           Editable islands must re-enable selection on themselves. */
        .mc-title, .mc-tx, .mc-add { -webkit-user-select: text; user-select: text; }
        .mc-title { font-family: var(--pixel); font-size: 12px; letter-spacing: 0; color: var(--note-ink); outline: none; cursor: text; min-width: 20px; }
        /* Empty title (default "메모" cleared): collapse to 0 so the divider
           line fills all the way to the tricolor dots. The "제목" placeholder
           only appears on hover/focus (re-expanding a click target) — at rest
           a clean, full-width line. The .empty class is driven by note.title
           (live via onInput), not the :empty pseudo, which a stray WebKit
           br would defeat. */
        .mc-title.empty { min-width: 0; }
        .mc-title.empty::before { content: ""; }
        .mc:hover .mc-title.empty, .mc-title.empty:focus { min-width: 20px; }
        .mc:hover .mc-title.empty::before, .mc-title.empty:focus::before { content: "제목"; color: rgba(0,0,0,.32); }
        .mc-rm { opacity: 0; border: none; background: transparent; cursor: pointer; color: var(--note-ink);
          font-family: var(--pixel); font-size: 12px; line-height: 1; padding: 0 2px; transition: opacity .12s; }
        .mc-row:hover .mc-rm { opacity: .7; }
        .mc-add { all: unset; box-sizing: border-box; width: 100%; font-family: var(--pixel); font-size: 12px; color: var(--note-ink); padding: 3px 0; cursor: text; }
        .mc-add::placeholder { color: rgba(0,0,0,.32); }
        .pad { transition: transform .14s ease; }
        .pad:hover { transform: translateY(-4px); }
        .pad:active { transform: translateY(-1px); }
      `}</style>
    </div>
  );
}
