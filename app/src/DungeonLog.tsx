/**
 * Real Dungeon Log — newest-first timeline of hook events.
 *
 * Renamed concept: the prior "Dungeon Log" listed dungeons (= sessions).
 * That now lives in Dungeons.tsx behind the ▲ button. THIS view is what
 * the name actually suggests: a live tail of tool activity (PostToolUse
 * events) annotated with session lifecycle markers (SessionStart/End).
 */

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Frame, HeaderBar, SubHeader } from "./components";

type RecentHookEvent = {
  id: number;
  ts: string;
  event_type: string;
  session_id: string | null;
  tool: string | null;
  file_path: string | null;
  command: string | null;
  duration_ms: number | null;
  interrupted: boolean;
};

function shortSession(id: string | null): string {
  return id ? id.slice(0, 6) : "------";
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

/** One-line summary of what happened. Tool-aware:
 *   Bash    → first 60 chars of the command
 *   Edit/Write/Read → file_path basename
 *   else    → tool name plus a hint */
function eventSummary(e: RecentHookEvent): string {
  if (e.event_type === "SessionStart") return "▶ session opened";
  if (e.event_type === "SessionEnd") return "■ session ended";
  if (e.event_type === "Stop") return "· turn boundary";
  if (e.event_type === "UserPromptSubmit") return "▷ user prompt";
  if (e.event_type === "PreToolUse") return `· ${e.tool ?? "?"} (about to fire)`;
  if (e.event_type === "PostToolUse") {
    const t = e.tool ?? "?";
    if (t === "Bash" && e.command) return `$ ${e.command.slice(0, 60)}`;
    if (e.file_path) {
      const base = e.file_path.split("/").pop() ?? e.file_path;
      return `${t} ${base}`;
    }
    return t;
  }
  return e.event_type;
}

/** Pick the glyph + color treatment for a row. */
function eventChrome(e: RecentHookEvent): { glyph: string; tone: "post" | "lifecycle" | "skip" } {
  if (e.interrupted) return { glyph: "✗", tone: "lifecycle" };
  switch (e.event_type) {
    case "SessionStart": return { glyph: "▶", tone: "lifecycle" };
    case "SessionEnd": return { glyph: "■", tone: "lifecycle" };
    case "Stop": return { glyph: "·", tone: "skip" };
    case "UserPromptSubmit": return { glyph: "▷", tone: "lifecycle" };
    case "PreToolUse": return { glyph: "·", tone: "skip" };
    case "PostToolUse": return { glyph: "◆", tone: "post" };
    default: return { glyph: "?", tone: "skip" };
  }
}

type NavKey = "stat" | "log" | "map" | "inv" | "cfg";
export function DungeonLogView({
  onClose,
  onNav,
  pinned,
}: {
  onClose: () => void;
  onNav?: (k: NavKey) => void;
  pinned?: boolean;
}) {
  const [events, setEvents] = useState<RecentHookEvent[] | null>(null);

  useEffect(() => {
    let stale = false;
    const refresh = () =>
      invoke<RecentHookEvent[]>("get_recent_events", { limit: 150 })
        .then((rows) => { if (!stale) setEvents(rows); })
        .catch(console.error);
    refresh();
    // Poll every 2s + react to known event types so the log stays live.
    const id = window.setInterval(refresh, 2000);
    const unlistens = [
      listen("battle-resolved", refresh),
      listen("dungeon-opened", refresh),
      listen("dungeon-closed", refresh),
      listen("dungeon-abandoned", refresh),
    ];
    return () => {
      stale = true;
      window.clearInterval(id);
      unlistens.forEach((p) => p.then((fn) => fn()));
    };
  }, []);

  // Filter chip — default "all PostToolUse-and-lifecycle", optional
  // "verbose" reveals PreToolUse / Stop noise.
  const [verbose, setVerbose] = useState(false);
  const filtered = (events ?? []).filter((e) => {
    if (verbose) return true;
    if (e.event_type === "PreToolUse" || e.event_type === "Stop") return false;
    return true;
  });

  return (
    <Frame>
      <HeaderBar active="log" onNav={onNav} pinned={pinned} />
      <SubHeader title="DUNGEON LOG" code="LG" back="MAIN" onBack={onClose} />

      {/* Filter / status strip */}
      <div
        style={{
          padding: "5px 10px",
          borderBottom: "1px solid var(--line)",
          display: "flex",
          gap: 6,
          alignItems: "center",
        }}
      >
        <span className="px-7 dim">
          {events === null ? "loading…" : `${filtered.length} / ${events.length}`}
        </span>
        <span style={{ flex: 1 }} />
        <button
          onClick={() => setVerbose((v) => !v)}
          className="px-7"
          title="Pre/Stop 이벤트 포함 토글"
          style={{
            all: "unset",
            cursor: "pointer",
            padding: "3px 7px",
            background: verbose ? "var(--fg)" : "var(--bg)",
            color: verbose ? "var(--bg)" : "var(--fg)",
            border: "1px solid var(--line)",
          }}
        >
          verbose
        </button>
      </div>

      <div style={{ flex: 1, overflowY: "auto" }}>
        {events === null ? (
          <div style={{ padding: 24 }}>
            <span className="tm-14">loading…</span>
          </div>
        ) : filtered.length === 0 ? (
          <div
            style={{
              flex: 1,
              padding: 24,
              textAlign: "center",
              display: "flex",
              flexDirection: "column",
              gap: 6,
            }}
          >
            <div className="px-7 dim">NO EVENTS</div>
            <div className="tm-14">
              Claude Code에서 도구를 호출하면 여기 실시간으로 쌓입니다.
            </div>
          </div>
        ) : (
          filtered.map((e) => <EventRow key={e.id} e={e} />)
        )}
      </div>
    </Frame>
  );
}

function EventRow({ e }: { e: RecentHookEvent }) {
  const { glyph, tone } = eventChrome(e);
  const summary = eventSummary(e);
  const glyphColor =
    tone === "post"
      ? "var(--accent)"
      : tone === "lifecycle"
      ? "var(--fg)"
      : "var(--muted)";

  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "14px 56px 1fr auto",
        gap: 8,
        alignItems: "center",
        padding: "4px 10px",
        borderBottom: "1px dashed var(--line)",
        opacity: tone === "skip" ? 0.65 : 1,
      }}
    >
      <span
        className="px-8"
        style={{ textAlign: "center", color: glyphColor }}
      >
        {glyph}
      </span>
      <span className="tm-14 tnum" style={{ whiteSpace: "nowrap" }}>
        {formatTime(e.ts)}
      </span>
      <span
        className="tm-16"
        style={{
          minWidth: 0,
          overflow: "hidden",
          whiteSpace: "nowrap",
          textOverflow: "ellipsis",
        }}
        title={e.file_path ?? e.command ?? summary}
      >
        {summary}
      </span>
      <span className="px-7 dim" style={{ whiteSpace: "nowrap" }}>
        {shortSession(e.session_id)}
        {e.duration_ms != null && ` · ${e.duration_ms}ms`}
        {e.interrupted && " · INT"}
      </span>
    </div>
  );
}
