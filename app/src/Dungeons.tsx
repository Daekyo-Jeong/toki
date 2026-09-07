import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Frame, HeaderBar, SubHeader } from "./components";

type DungeonRow = {
  id: string;
  source: string;
  project_path: string | null;
  status: string;
  started_at: string;
  ended_at: string | null;
  event_count: number;
};

function durationLabel(started: string, ended: string | null): string {
  const a = new Date(started).getTime();
  const b = ended ? new Date(ended).getTime() : Date.now();
  if (Number.isNaN(a) || Number.isNaN(b) || b < a) return "—";
  const sec = Math.floor((b - a) / 1000);
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  const s = sec % 60;
  if (h > 0) return `${h}h ${String(m).padStart(2, "0")}m`;
  if (m > 0) return `${m}m ${String(s).padStart(2, "0")}s`;
  return `${s}s`;
}

function projectShort(path: string | null): string {
  if (!path) return "(unknown)";
  const parts = path.split("/").filter(Boolean);
  return parts.slice(-2).join("/");
}

function shortId(id: string): string {
  return id.length > 8 ? id.slice(0, 8) : id;
}

function formatStarted(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getMonth() + 1)}/${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function DungeonsView({
  onClose,
  onNav,
  pinned,
  onRetro,
}: {
  onClose: () => void;
  onNav?: (k: "stat" | "log" | "map" | "inv" | "cfg") => void;
  pinned?: boolean;
  /** Open the retrospective view for a given dungeon. */
  onRetro: (dungeonId: string) => void;
}) {
  const [dungeons, setDungeons] = useState<DungeonRow[] | null>(null);
  // Set of session ids that have a transcript → RETRO enabled. The old
  // event_count gate was wrong (retro reads the transcript now, and
  // dungeon_events is sparse — most real sessions have zero events).
  const [retroable, setRetroable] = useState<Set<string>>(() => new Set());

  useEffect(() => {
    const refresh = () =>
      invoke<DungeonRow[]>("get_dungeons", { limit: 300 })
        .then((rows) => {
          setDungeons(rows);
          // Ask backend which of these sessions have transcripts.
          invoke<string[]>("retrospective_available", {
            sessionIds: rows.map((r) => r.id),
          })
            .then((ids) => setRetroable(new Set(ids)))
            .catch(() => {});
        })
        .catch(console.error);
    refresh();
    const id = setInterval(refresh, 2000);
    const unlistens: Promise<() => void>[] = [
      listen("dungeon-opened", refresh),
      listen("dungeon-closed", refresh),
      listen("dungeon-abandoned", refresh),
      listen("battle-resolved", refresh),
    ];
    return () => {
      clearInterval(id);
      unlistens.forEach((p) => p.then((fn) => fn()));
    };
  }, []);

  const isEmpty = dungeons !== null && dungeons.length === 0;

  return (
    <Frame>
      <HeaderBar active="map" onNav={onNav} pinned={pinned} />
      <SubHeader title="DUNGEONS" code="DG" back="MAIN" onBack={onClose} />

      {dungeons === null ? (
        <div style={{ padding: 24, fontSize: 11, color: "var(--muted)" }}>loading…</div>
      ) : isEmpty ? (
        <div
          style={{
            flex: 1,
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            justifyContent: "center",
            gap: 8,
            padding: 24,
            textAlign: "center",
          }}
        >
          <div
            style={{
              fontSize: 11,
              letterSpacing: "0.2em",
              color: "var(--muted)",
              textTransform: "uppercase",
            }}
          >
            No dungeons yet
          </div>
          <div style={{ fontSize: 11, color: "var(--muted)", lineHeight: 1.5 }}>
            &gt; Claude Code 세션이 시작되면 던전이 열립니다.
          </div>
        </div>
      ) : (
        <GroupedDungeonList dungeons={dungeons} onRetro={onRetro} retroable={retroable} />
      )}

      <div style={{ display: "flex", borderTop: "2px solid var(--line)" }}>
        <button className="btn" style={{ flex: 1, padding: "10px 0" }} onClick={onClose}>
          ← BACK
        </button>
      </div>
    </Frame>
  );
}

/* ─── Project grouping ───────────────────────────────────
   Multiple Claude Code sessions on the same folder happen all the time
   (each window restart = new session). Grouping by `project_path` keeps
   the log readable. Groups collapse independently; the most-recently
   active group sits at the top.                                       */

type DungeonGroup = {
  key: string;
  displayName: string;
  rows: DungeonRow[];
  totalEvents: number;
  latestStart: string; // for ordering groups
};

function groupByProject(rows: DungeonRow[]): DungeonGroup[] {
  const map = new Map<string, DungeonGroup>();
  for (const r of rows) {
    const key = r.project_path ?? "__unknown__";
    let g = map.get(key);
    if (!g) {
      g = {
        key,
        displayName: r.project_path ? projectShort(r.project_path) : "(no path)",
        rows: [],
        totalEvents: 0,
        latestStart: r.started_at,
      };
      map.set(key, g);
    }
    g.rows.push(r);
    g.totalEvents += r.event_count;
    if (r.started_at > g.latestStart) g.latestStart = r.started_at;
  }
  return Array.from(map.values())
    .map((g) => ({
      ...g,
      rows: [...g.rows].sort((a, b) => b.started_at.localeCompare(a.started_at)),
    }))
    .sort((a, b) => b.latestStart.localeCompare(a.latestStart));
}

function GroupedDungeonList({
  dungeons,
  onRetro,
  retroable,
}: {
  dungeons: DungeonRow[];
  onRetro: (dungeonId: string) => void;
  retroable: Set<string>;
}) {
  // Single-selection: clicking a session toggles its action strip.
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  const groups = groupByProject(dungeons);

  const toggleCollapse = (key: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      next.has(key) ? next.delete(key) : next.add(key);
      return next;
    });

  return (
    <div style={{ flex: 1, overflowY: "auto" }}>
      {groups.map((g) => {
        const isCollapsed = collapsed.has(g.key);
        return (
          <div key={g.key}>
            {/* Group header */}
            <button
              onClick={() => toggleCollapse(g.key)}
              style={{
                all: "unset",
                cursor: "pointer",
                display: "grid",
                gridTemplateColumns: "14px 1fr auto",
                gap: 8,
                alignItems: "center",
                width: "100%",
                boxSizing: "border-box",
                padding: "5px 10px",
                background: "var(--fg)",
                color: "var(--bg)",
                borderTop: "1px solid var(--line)",
                borderBottom: "1px solid var(--line)",
              }}
            >
              <span className="px-7" style={{ textAlign: "center" }}>
                {isCollapsed ? "▸" : "▾"}
              </span>
              <span
                className="px-8"
                style={{
                  letterSpacing: "0.1em",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
              >
                {g.displayName}
              </span>
              <span className="px-7">
                {g.rows.length} sess · ⚔ {g.totalEvents}
              </span>
            </button>

            {/* Session rows */}
            {!isCollapsed &&
              g.rows.map((d) => (
                <DungeonRowItem
                  key={d.id}
                  d={d}
                  selected={selectedId === d.id}
                  canRetro={retroable.has(d.id)}
                  onSelect={() => setSelectedId(selectedId === d.id ? null : d.id)}
                  onRetro={onRetro}
                />
              ))}
          </div>
        );
      })}
    </div>
  );
}

function DungeonRowItem({
  d,
  selected,
  canRetro,
  onSelect,
  onRetro,
}: {
  d: DungeonRow;
  selected: boolean;
  canRetro: boolean;
  onSelect: () => void;
  onRetro: (id: string) => void;
}) {
  const dur = durationLabel(d.started_at, d.ended_at);
  const { glyph, label, bg, fg } = statusStyle(d.status);

  return (
    <>
      <button
        onClick={onSelect}
        style={{
          all: "unset",
          cursor: "pointer",
          display: "grid",
          gridTemplateColumns: "20px 1fr auto",
          gap: 8,
          alignItems: "center",
          width: "100%",
          boxSizing: "border-box",
          padding: "6px 10px",
          borderBottom: "1px dashed var(--line)",
          background: selected ? "var(--bg)" : "transparent",
          outline: selected ? "1px solid var(--accent)" : "none",
          outlineOffset: -1,
        }}
      >
        <div className="section-label dim tnum">{glyph}</div>
        <div style={{ minWidth: 0 }}>
          <div className="px-7" style={{ display: "flex", gap: 6, alignItems: "center" }}>
            <span>{formatStarted(d.started_at)}</span>
            <span className="dim">· {dur}</span>
            <span className="dim">· ⚔ {d.event_count}</span>
            <span className="dim">· {shortId(d.id)}</span>
          </div>
        </div>
        <span
          style={{
            fontSize: 10,
            fontWeight: 800,
            background: bg,
            color: fg,
            padding: "2px 6px",
            border: "1px solid var(--line)",
            letterSpacing: "0.1em",
          }}
        >
          {label}
        </span>
      </button>

      {/* Expanded action strip — only when this row is selected. */}
      {selected && (
        <div
          style={{
            display: "flex",
            gap: 6,
            padding: "6px 10px 8px",
            borderBottom: "1px dashed var(--line)",
            background: "var(--bg)",
          }}
        >
          <button
            onClick={() => canRetro && onRetro(d.id)}
            disabled={!canRetro}
            title={canRetro ? "자가학습 회고 생성" : "transcript 없음 — 회고 불가"}
            className="px-7"
            style={{
              all: "unset",
              cursor: canRetro ? "pointer" : "not-allowed",
              opacity: canRetro ? 1 : 0.3,
              padding: "4px 10px",
              background: "var(--fg)",
              color: "var(--bg)",
              letterSpacing: "0.15em",
            }}
          >
            ◆ RETRO
          </button>
          <span className="px-7 dim" style={{ alignSelf: "center" }}>
            ⓘ 이 세션의 자가학습 회고를 생성합니다 (sonnet · 사용자 5h 쿼터 소비)
          </span>
        </div>
      )}
    </>
  );
}

function statusStyle(status: string): {
  glyph: string;
  label: string;
  bg: string;
  fg: string;
} {
  switch (status) {
    case "active":
      return { glyph: "▶", label: "ACTIVE", bg: "var(--accent)", fg: "var(--bg)" };
    case "abandoned":
      return { glyph: "✗", label: "ABANDONED", bg: "var(--fg)", fg: "var(--bg)" };
    case "cleared":
    default:
      return { glyph: "✓", label: "CLEAR", bg: "var(--bg)", fg: "var(--fg)" };
  }
}
