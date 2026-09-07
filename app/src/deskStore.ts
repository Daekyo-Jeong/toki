/* v5 M7 — 데스크 상태의 단일 진실 (spec §7.0).
 *
 * v4는 localStorage가 주인이었다. 창이 하나일 땐 그걸로 충분했지만, 모니터마다
 * 창이 뜨는 순간 창마다 다른 React 상태를 들고 갈라진다. 그래서 **파일 하나
 * (`~/.toki/desk.json`)가 주인**이고, 창들은 이벤트로 따라온다.
 *
 * 규약 셋:
 * 1. 쓰기는 항상 이 스토어를 거친다. 컴포넌트가 직접 파일을 만지지 않는다.
 * 2. 저장은 디바운스한다 — 메모를 끄는 동안 매 프레임 파일을 쓰면 안 된다.
 * 3. 다른 창의 변경은 이벤트로 받아 **통째로 교체**한다. 병합하지 않는다 —
 *    한쪽이 지운 메모가 다른 쪽 병합으로 되살아나는 게 더 나쁘다.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";

export type NoteColor = "manila" | "mint" | "rose" | "sky";
export type NoteItem = { id: string; t: string; done: boolean; chk?: boolean };
export type Note = {
  id: string; x: number; y: number; rot: number;
  color: NoteColor; z: number; title: string; items: NoteItem[];
  /** 이 메모가 붙어 있는 모니터 키. 없으면 주 모니터 몫으로 승계한다. */
  mon?: string;
};
/** 셸은 하나뿐이다 — 어느 모니터에 있는지가 상태의 일부다. */
export type ShellPos = { mon?: string; fx: number; fy: number };
export type DeskState = { notes: Note[]; shellPos: ShellPos | null };

/** Rust `desk::DragSpace` — 드래그 중 확장된 창의 좌표계. */
export type MonRect = { key: string; x: number; y: number; w: number; h: number };
export type DragSpace = { dx: number; dy: number; width: number; height: number; monitors: MonRect[] };

/** Rust `desk::MonitorInfo`. 창 하나가 맡은 모니터. */
export type MonitorInfo = {
  key: string; name: string; window: string;
  width: number; height: number; x: number; y: number;
  scale: number; primary: boolean;
};

/** 이 메모가 이 창(모니터)의 것인가.
 *
 * **판정은 여기 한 곳에만 둔다.** 렌더와 저장이 각자 같은 식을 쓰다가 한쪽만
 * 어긋나면 남의 화면 메모를 옮겨 쓴다 — 2026-09-03에 실제로 그랬다.
 * `mon`이 없는 레거시 메모는 **주 모니터**가 떠맡는다(v4엔 모니터 개념이 없었다).
 */
export function ownsNote(note: Note, monKey: string, isPrimary: boolean): boolean {
  return (note.mon ?? (isPrimary ? monKey : "\u0000")) === monKey;
}

/** 이 메모가 **지금 없는 모니터**에 묶여 있나 — 주 모니터가 떠맡을 대상.
 *
 * 모니터를 뽑았다고 메모가 사라지면 데이터를 잃은 것처럼 느껴진다(사용자
 * 2026-09-04). 저장된 `mon`은 건드리지 않으므로 그 모니터를 다시 꽂으면
 * 제자리로 돌아가고, 주 화면에서 끌면 그때 주인이 바뀐다.
 * `known`이 비어 있으면(아직 모니터 목록을 못 받음) **아무도 고아가 아니다** —
 * 모르는 상태를 "전부 고아"로 읽으면 온 메모가 주 화면으로 쏟아진다.
 */
export function isOrphanNote(note: Note, known: Set<string>): boolean {
  return !!note.mon && known.size > 0 && !known.has(note.mon);
}

/** 이 창이 그리고 저장할 메모인가. 렌더와 저장이 **같은 함수**를 쓴다. */
export function claimsNote(
  note: Note,
  monKey: string,
  isPrimary: boolean,
  known: Set<string>,
): boolean {
  return ownsNote(note, monKey, isPrimary) || (isPrimary && isOrphanNote(note, known));
}

const EMPTY: DeskState = { notes: [], shellPos: null };

let state: DeskState = EMPTY;
let loaded = false;
let loading: Promise<void> | null = null;
const subs = new Set<(s: DeskState) => void>();
let saveTimer: number | undefined;
/** 이 창의 라벨 — 자기가 쏜 이벤트를 되받지 않으려고 비교한다. */
let selfLabel = "";

function parse(raw: string): DeskState {
  if (!raw.trim()) return EMPTY;
  try {
    const b = JSON.parse(raw);
    return {
      notes: Array.isArray(b?.notes) ? b.notes : [],
      shellPos: b?.shellPos && typeof b.shellPos.fx === "number" ? b.shellPos : null,
    };
  } catch {
    return EMPTY;
  }
}

function emit() {
  for (const fn of subs) fn(state);
}

/** 최초 1회 로드 + 다른 창의 변경 구독. 창마다 한 번씩 부른다. */
export function initDeskStore(): Promise<void> {
  if (loading) return loading;
  loading = (async () => {
    try {
      selfLabel = getCurrentWindow().label;
    } catch { /* 프리뷰(브라우저)에는 창이 없다 */ }
    try {
      state = parse(await invoke<string>("desk_state_load"));
    } catch {
      state = EMPTY;
    }
    loaded = true;
    emit();
    try {
      await listen<{ from: string; json: string }>("desk-changed", (e) => {
        if (e.payload.from === selfLabel) return; // 자기 메아리
        state = parse(e.payload.json);
        emit();
      });
    } catch { /* 프리뷰 */ }
  })();
  return loading;
}

export function getDesk(): DeskState {
  return state;
}
export function isLoaded(): boolean {
  return loaded;
}

/** 상태 갱신 — 로컬에 즉시 반영하고 파일 저장은 디바운스한다.
 *
 * `immediate`는 **다른 창이 곧바로 봐야 하는 변경**에 쓴다(화면을 넘긴 메모).
 * 디바운스를 기다리면 그동안 원본도 미리보기도 없는 빈 구간이 생긴다. */
export function setDesk(
  next: DeskState | ((s: DeskState) => DeskState),
  immediate = false,
): Promise<void> {
  state = typeof next === "function" ? (next as (s: DeskState) => DeskState)(state) : next;
  emit();
  window.clearTimeout(saveTimer);
  const save = (): Promise<void> =>
    invoke("desk_state_save", { json: JSON.stringify(state) }).then(() => undefined).catch(() => undefined);
  if (immediate) return save();
  saveTimer = window.setTimeout(save, 300);
  return Promise.resolve();
}

/** React 바인딩. 창 안의 어느 컴포넌트든 같은 상태를 본다. */
export function useDesk(): [DeskState, boolean] {
  const [s, setS] = useState<DeskState>(state);
  const [ready, setReady] = useState(loaded);
  useEffect(() => {
    const fn = (next: DeskState) => { setS(next); setReady(true); };
    subs.add(fn);
    initDeskStore().then(() => { setS(state); setReady(true); });
    return () => { subs.delete(fn); };
  }, []);
  return [s, ready];
}
