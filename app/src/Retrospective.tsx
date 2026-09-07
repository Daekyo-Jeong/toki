/**
 * MVP-12 (pivot) — Retrospective view.
 *
 * Given a dungeon id, show a markdown retrospective summarising what the
 * user did in that session. Backed by `retrospective_generate` (calls
 * `claude -p` over the dungeon's hook events) and cached on
 * `dungeons.retrospective`.
 *
 * This is the LLM call Toki actually has unique data for — in-session
 * Claude can't see across sessions, but Toki has the full hook stream.
 */

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Frame, HeaderBar, SubHeader, TokenFooter, renderMarkdown } from "./components";
import { retroQueue } from "./retroQueue";

type DungeonRow = {
  id: string;
  source: string;
  project_path: string | null;
  status: string;
  started_at: string;
  ended_at: string | null;
  event_count: number;
};

type PlannerStatus = {
  available: boolean;
  bin_path: string | null;
  busy: boolean;
};

function durationLabel(started: string, ended: string | null): string {
  const a = new Date(started).getTime();
  const b = ended ? new Date(ended).getTime() : Date.now();
  if (Number.isNaN(a) || Number.isNaN(b) || b < a) return "—";
  const sec = Math.floor((b - a) / 1000);
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  if (h > 0) return `${h}h ${String(m).padStart(2, "0")}m`;
  return `${m}m`;
}


export function RetrospectiveView({
  dungeonId,
  onClose,
  onNav,
  pinned,
}: {
  dungeonId: string;
  onClose: () => void;
  onNav?: (k: "stat" | "log" | "map" | "inv" | "cfg") => void;
  pinned?: boolean;
}) {
  const [dungeon, setDungeon] = useState<DungeonRow | null>(null);
  const [status, setStatus] = useState<PlannerStatus | null>(null);
  const [body, setBody] = useState<string | null>(null);
  const [generatedAt, setGeneratedAt] = useState<string | null>(null);
  // `busy` is derived from the global retroQueue so it survives view
  // unmount: clicking ▶ GENERATE then navigating away and back returns
  // to a "still generating" state rather than appearing idle.
  const [busy, setBusy] = useState(retroQueue.has(dungeonId));
  const [error, setError] = useState<string | null>(null);
  // Model picker — sonnet is default (coaching needs real reasoning, not
  // haiku-grade summarization). opus offered for the deepest critique.
  const [model, setModel] = useState<"sonnet" | "opus">("sonnet");

  useEffect(() => {
    let stale = false;
    // Pull dungeon meta + cached retro + planner status in parallel.
    invoke<DungeonRow[]>("get_dungeons", { limit: 200 })
      .then((rows) => {
        if (stale) return;
        setDungeon(rows.find((r) => r.id === dungeonId) ?? null);
      })
      .catch(console.error);
    invoke<[string, string] | null>("retrospective_get", { dungeonId })
      .then((res) => {
        if (stale) return;
        if (res) {
          setBody(res[0]);
          setGeneratedAt(res[1]);
        }
      })
      .catch(() => {});
    invoke<PlannerStatus>("planner_status")
      .then((s) => { if (!stale) setStatus(s); })
      .catch(() => {});

    // Subscribe to the global retro queue so busy state stays correct
    // even if the call was kicked off from a previous mount of this view.
    const unsubQueue = retroQueue.subscribe((s) => {
      if (!stale) setBusy(s.has(dungeonId));
    });

    // Listen for completion events: even if `busy` is managed by the
    // queue, we need to actually pull the new body into local state.
    const unlistenDone = listen<{ dungeon_id: string; body: string }>(
      "retro-generated",
      (e) => {
        if (stale || e.payload.dungeon_id !== dungeonId) return;
        setBody(e.payload.body);
        setGeneratedAt(new Date().toISOString());
        setError(null);
      },
    );
    const unlistenFail = listen<{ dungeon_id: string; error: string }>(
      "retro-failed",
      (e) => {
        if (stale || e.payload.dungeon_id !== dungeonId) return;
        setError(e.payload.error);
      },
    );

    return () => {
      stale = true;
      unsubQueue();
      unlistenDone.then((fn) => fn());
      unlistenFail.then((fn) => fn());
    };
  }, [dungeonId]);

  // Gate on transcript availability, not event_count — retro reads the
  // session transcript now, and dungeon_events is sparse.
  const [hasTranscript, setHasTranscript] = useState<boolean | null>(null);
  useEffect(() => {
    invoke<string[]>("retrospective_available", { sessionIds: [dungeonId] })
      .then((ids) => setHasTranscript(ids.includes(dungeonId)))
      .catch(() => setHasTranscript(false));
  }, [dungeonId]);
  // Gate on the per-dungeon queue state (`busy`), NOT the global
  // `status.busy` — that's polled every 4s and reflects a process-wide
  // planner mutex, so it lags and can stay stale-true right after a
  // generation, silently blocking REGENERATE. True single-flight is
  // enforced by retroQueue (per-dungeon) + the backend mutex (which
  // surfaces a "planner busy" error via retro-failed if genuinely
  // concurrent). `status.available` is the only status field we still gate on.
  const canGenerate =
    !!status?.available && !busy && hasTranscript === true;

  function generate() {
    if (!canGenerate) return;
    setError(null);
    // Fire-and-forget via the global queue. The Tauri command runs on a
    // worker thread (#[tauri::command] async + spawn_blocking) and emits
    // `retro-generated` / `retro-failed` when done. Body/error arrive via
    // the event listeners above — works even if the user navigates away.
    retroQueue.start(dungeonId, model);
  }

  return (
    <Frame>
      {/* Retro lives below the Dungeons (map) tab, not the Log tab. */}
      <HeaderBar active="map" onNav={onNav} pinned={pinned} />
      <SubHeader
        title="RETROSPECTIVE · 자가학습 회고"
        code="RT"
        back="LOG"
        onBack={onClose}
      />

      {/* Dungeon meta strip */}
      <div
        style={{
          padding: "6px 10px",
          borderBottom: "1px solid var(--line)",
          display: "flex",
          flexDirection: "column",
          gap: 2,
        }}
      >
        {dungeon ? (
          <>
            <div
              className="px-8"
              style={{
                display: "flex",
                gap: 6,
                alignItems: "baseline",
                flexWrap: "wrap",
                wordBreak: "break-all",
              }}
            >
              <span style={{ flex: 1, minWidth: 0 }}>
                {dungeon.project_path ?? "(no path)"}
              </span>
              <span className="px-7 dim">· {dungeon.id.slice(0, 8)}</span>
            </div>
            <div className="tm-14 dim">
              {dungeon.status} · {durationLabel(dungeon.started_at, dungeon.ended_at)} · ⚔ {dungeon.event_count}
            </div>
          </>
        ) : (
          <div className="px-7 dim">loading dungeon…</div>
        )}
      </div>

      <div
        style={{
          flex: 1,
          overflowY: "auto",
          padding: 10,
          display: "flex",
          flexDirection: "column",
          gap: 8,
        }}
      >
        {/* Generate / regenerate + model picker */}
        <div style={{ display: "flex", gap: 6, alignItems: "center", flexWrap: "wrap" }}>
          <button
            onClick={generate}
            disabled={!canGenerate}
            className="px-7"
            style={{
              all: "unset",
              cursor: canGenerate ? "pointer" : "not-allowed",
              opacity: canGenerate ? 1 : 0.4,
              padding: "5px 10px",
              background: "var(--fg)",
              color: "var(--bg)",
              letterSpacing: "0.15em",
            }}
          >
            {busy
              ? "GENERATING…"
              : body
              ? "▶ REGENERATE"
              : "▶ GENERATE RETRO"}
          </button>
          <div style={{ display: "flex", gap: 0 }}>
            {(["sonnet", "opus"] as const).map((m) => (
              <button
                key={m}
                onClick={() => setModel(m)}
                disabled={busy}
                className="px-7"
                title={m === "sonnet" ? "균형 · 기본" : "가장 깊은 비평 · ~5x 비용"}
                style={{
                  all: "unset",
                  cursor: busy ? "not-allowed" : "pointer",
                  padding: "3px 7px",
                  background: model === m ? "var(--fg)" : "var(--bg)",
                  color: model === m ? "var(--bg)" : "var(--fg)",
                  border: "1px solid var(--line)",
                  letterSpacing: "0.1em",
                }}
              >
                {m}
              </button>
            ))}
          </div>
          {!status?.available && (
            <span className="px-7" style={{ color: "var(--accent)" }}>
              ! claude CLI 미발견
            </span>
          )}
          {status?.busy && <span className="px-7 dim">busy</span>}
          {hasTranscript === false && (
            <span className="px-7 dim">transcript 없음 · 회고 불가</span>
          )}
        </div>

        {generatedAt && (
          <div className="tm-14 dim">
            generated · {new Date(generatedAt).toLocaleString()}
          </div>
        )}

        {error && (
          <div
            className="tm-14"
            style={{
              padding: 8,
              border: "1px solid var(--accent)",
              color: "var(--accent)",
              wordBreak: "break-word",
            }}
          >
            ERROR · {error}
          </div>
        )}

        {body && (
          <div
            className="tm-14"
            style={{
              borderTop: "1px dashed var(--line)",
              paddingTop: 6,
              fontFamily: "var(--term)",
            }}
          >
            {renderMarkdown(body)}
          </div>
        )}

        {!body && !error && !busy && (
          <div className="tm-14 dim" style={{ lineHeight: 1.5 }}>
            ⓘ 이 세션의 transcript(네 프롬프트 + 도구 흐름 + 에러·중단 신호)를 추려
            {" "}{model}에게 보내, "어떻게 명령했고 / 어디서 꼬였고 / 더 나았을 길 /
            관성처럼 하는 것"을 짚는 코칭 회고를 만듭니다. 사용자 5h 쿼터 소비.
          </div>
        )}

        {busy && (
          <div
            className="tm-14"
            style={{
              padding: 8,
              border: "1px dashed var(--line)",
              lineHeight: 1.5,
              opacity: 0.8,
            }}
          >
            ⏳ 백그라운드에서 생성 중 ({model})
            <br />
            <span className="dim">
              다른 화면으로 이동해도 계속 진행됩니다. 완료 시 자동 반영.
            </span>
          </div>
        )}
      </div>

      <TokenFooter today="—" total="—" />
    </Frame>
  );
}
