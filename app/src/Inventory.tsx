import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Frame, HeaderBar, SubHeader, TokenFooter } from "./components";

/** A skill recorded via hook (something Toki has actually seen used). */
type UsedSkill = {
  skill_name: string;
  unlocked_at: string;
  use_count: number;
};

/** A skill discovered by scanning the filesystem (catalog). */
type SkillManifest = {
  name: string;
  description: string | null;
  plugin: string | null;
  path: string;
};

type Totals = { cache_read: number };
type Tab = "used" | "library" | "my";

/* `mcp__<server>__<tool>` — pull out the server + tool halves. */
function parseUsedName(raw: string): { server: string; tool: string } {
  if (!raw.startsWith("mcp__")) return { server: "", tool: raw };
  const rest = raw.slice(5);
  const i = rest.indexOf("__");
  if (i < 0) return { server: rest, tool: "" };
  return { server: rest.slice(0, i), tool: rest.slice(i + 2) };
}

function flavorForUsed(server: string, tool: string): string {
  // Curated RPG names. Anything else gets a friendly fallback.
  const named: Record<string, string> = {
    ccd_session: "기록의 룬",
    ccd_session_mgmt: "회의의 룬",
    ccd_directory: "지도의 마법",
    Figma: "환영 마법",
    Blender: "조각의 룬",
    Claude_Preview: "관찰의 거울",
    Claude_in_Chrome: "탐험의 마법",
    Control_Chrome: "지배의 룬",
    Read_and_Write_Apple_Notes: "두루마리 작성",
    "mcp-registry": "도감 열람",
    "pdf-viewer": "고문서 해독",
    "scheduled-tasks": "시간의 룬",
    "computer-use": "원격 조종술",
  };
  if (server && named[server]) return named[server];
  // Built-in flavors for common non-mcp tools.
  const builtin: Record<string, string> = {
    WebFetch: "원격 시야",
    TaskOutput: "정찰병의 보고",
    TaskStop: "정찰병 회수",
    Agent: "사역마 소환",
    AskUserQuestion: "조언 구하기",
    ToolSearch: "도구 탐색",
    Monitor: "감시의 눈",
    ScheduleWakeup: "시간 마법",
    CronCreate: "예언서 작성",
    CronList: "예언서 열람",
    CronDelete: "예언서 파기",
    PushNotification: "전령",
    RemoteTrigger: "원격 발동",
    EnterPlanMode: "사색의 룬",
    ExitPlanMode: "사색의 종결",
    EnterWorktree: "차원 이동",
    ExitWorktree: "차원 복귀",
    WebSearch: "도서관 탐색",
    TodoWrite: "임무 기록",
  };
  if (builtin[tool || server]) return builtin[tool || server];
  return "고유 비전";
}

function fmtCount(n: number): string {
  if (n >= 1_000) return (n / 1_000).toFixed(1) + "K";
  return String(n);
}

/* ─── Card components ────────────────────────────────────── */

function UsedCard({ s }: { s: UsedSkill }) {
  const { server, tool } = parseUsedName(s.skill_name);
  const display = tool || s.skill_name;
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "40px 1fr auto",
        gap: 8,
        alignItems: "stretch",
        borderBottom: "1px dashed var(--line)",
        padding: "6px 10px",
      }}
    >
      <div
        style={{
          border: "1px solid var(--line)",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: "var(--fg)",
          color: "var(--accent)",
          fontFamily: "var(--pixel)",
          fontSize: 14,
        }}
      >
        ◆
      </div>
      <div style={{ minWidth: 0 }}>
        <div
          className="px-8"
          style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
        >
          {flavorForUsed(server, display)}
        </div>
        <div
          className="tm-14 dim"
          style={{ marginTop: 3, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
        >
          /{server ? `${server}::${display}` : display}
        </div>
      </div>
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "flex-end",
          justifyContent: "space-between",
        }}
      >
        <div className="px-7 tnum">×{fmtCount(s.use_count)}</div>
        <div className="px-7 dim">READY</div>
      </div>
    </div>
  );
}

function ManifestCard({ s, locked = false }: { s: SkillManifest; locked?: boolean }) {
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "40px 1fr",
        gap: 8,
        alignItems: "stretch",
        borderBottom: "1px dashed var(--line)",
        padding: "6px 10px",
        opacity: locked ? 0.55 : 1,
      }}
    >
      <div
        style={{
          border: "1px solid var(--line)",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: locked ? "var(--bg)" : "var(--fg)",
          color: locked ? "var(--muted)" : "var(--accent)",
          fontFamily: "var(--pixel)",
          fontSize: 14,
        }}
      >
        {locked ? "?" : "◆"}
      </div>
      <div style={{ minWidth: 0 }}>
        <div
          className="px-8"
          style={{
            display: "flex",
            gap: 6,
            alignItems: "baseline",
            overflow: "hidden",
            whiteSpace: "nowrap",
            textOverflow: "ellipsis",
          }}
        >
          <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{s.name}</span>
          {s.plugin && (
            <span
              className="px-7 dim"
              style={{ flexShrink: 0 }}
            >
              · {s.plugin}
            </span>
          )}
        </div>
        {s.description && (
          <div
            className="tm-14 dim"
            style={{
              marginTop: 3,
              lineHeight: 1.3,
              display: "-webkit-box",
              WebkitLineClamp: 2,
              WebkitBoxOrient: "vertical",
              overflow: "hidden",
            }}
          >
            {s.description}
          </div>
        )}
      </div>
    </div>
  );
}

/* ─── View ───────────────────────────────────────────────── */

type NavKey = "stat" | "log" | "map" | "inv" | "cfg";
export function InventoryView({
  onClose,
  onNav,
  pinned,
}: {
  onClose: () => void;
  onNav?: (k: NavKey) => void;
  pinned?: boolean;
}) {
  const [tab, setTab] = useState<Tab>("used");
  const [used, setUsed] = useState<UsedSkill[] | null>(null);
  const [library, setLibrary] = useState<SkillManifest[] | null>(null);
  const [mySkills, setMySkills] = useState<SkillManifest[] | null>(null);
  const [totals, setTotals] = useState<Totals | null>(null);

  useEffect(() => {
    const refreshUsed = () =>
      invoke<UsedSkill[]>("get_skills").then(setUsed).catch(console.error);
    const refreshLibrary = () =>
      invoke<SkillManifest[]>("get_library_skills").then(setLibrary).catch(console.error);
    const refreshMy = () =>
      invoke<SkillManifest[]>("get_my_skills").then(setMySkills).catch(console.error);
    const refreshTotals = () =>
      invoke<Totals>("get_totals").then(setTotals).catch(() => {});

    refreshUsed();
    refreshLibrary();
    refreshMy();
    refreshTotals();

    const id = setInterval(() => {
      refreshUsed();
      refreshTotals();
    }, 3000);
    const unlistens: Promise<() => void>[] = [
      listen("skill-unlocked", refreshUsed),
      listen("skill-used", refreshUsed),
    ];
    return () => {
      clearInterval(id);
      unlistens.forEach((p) => p.then((fn) => fn()));
    };
  }, []);

  const counts: Record<Tab, number | null> = {
    used: used?.length ?? null,
    library: library?.length ?? null,
    my: mySkills?.length ?? null,
  };

  return (
    <Frame>
      <HeaderBar
        active="inv"
        onNav={onNav}
        pinned={pinned}
      />
      <SubHeader title="INVENTORY · SKILLBOOK" code="IN" back="MAIN" onBack={onClose} />

      <div
        style={{
          padding: "6px 10px",
          borderBottom: "1px solid var(--line)",
          display: "flex",
          gap: 6,
          alignItems: "center",
        }}
      >
        {(["used", "library", "my"] as const).map((t) => {
          const labels: Record<Tab, string> = {
            used: "USED",
            library: "LIBRARY",
            my: "MY",
          };
          const active = tab === t;
          return (
            <button
              key={t}
              onClick={() => setTab(t)}
              className="px-7"
              style={{
                all: "unset",
                cursor: "pointer",
                padding: "3px 5px",
                background: active ? "var(--fg)" : "var(--bg)",
                color: active ? "var(--bg)" : "var(--fg)",
                border: "1px solid var(--line)",
              }}
            >
              {labels[t]}
              {counts[t] !== null && <span style={{ marginLeft: 6 }}>{counts[t]}</span>}
            </button>
          );
        })}
        <span style={{ flex: 1 }} />
        <span className="px-7 dim">{tab === "used" ? "hook-tracked" : tab === "library" ? "installed" : "user-authored"}</span>
      </div>

      {tab === "used" && (
        <UsedList list={used} />
      )}
      {tab === "library" && (
        <ManifestList list={library} emptyHint="설치된 플러그인의 SKILL.md 가 없어요." />
      )}
      {tab === "my" && (
        <ManifestList
          list={mySkills}
          emptyHint={
            <>
              직접 작성한 스킬이 아직 없어요.
              <br />
              <span className="dim">
                ~/.claude/skills/&lt;name&gt;/SKILL.md 또는
                <br />
                ~/.claude/commands/*.md 에 만들면 보임
              </span>
            </>
          }
        />
      )}

      <TokenFooter
        today="—"
        total={totals ? fmtCount(totals.cache_read) : "—"}
      />
    </Frame>
  );
}

function UsedList({ list }: { list: UsedSkill[] | null }) {
  if (list === null) {
    return <div style={{ padding: 24, fontSize: 11, color: "var(--muted)" }}>loading…</div>;
  }
  if (list.length === 0) {
    return (
      <EmptyState
        title="No skills used"
        body={
          <>
            &gt; MCP 도구나 비기본 내장 도구를 호출하면 토키가 학습합니다.
            <br />
            <span className="dim">(Bash · Edit · Read 같은 기본 도구는 평타)</span>
          </>
        }
      />
    );
  }
  return (
    <div style={{ flex: 1, overflowY: "auto" }}>
      {list.map((s) => (
        <UsedCard key={s.skill_name} s={s} />
      ))}
    </div>
  );
}

function ManifestList({
  list,
  emptyHint,
}: {
  list: SkillManifest[] | null;
  emptyHint: React.ReactNode;
}) {
  if (list === null) {
    return <div style={{ padding: 24, fontSize: 11, color: "var(--muted)" }}>loading…</div>;
  }
  if (list.length === 0) {
    return <EmptyState title="No skills" body={emptyHint} />;
  }
  return (
    <div style={{ flex: 1, overflowY: "auto" }}>
      {list.map((s, i) => (
        <ManifestCard key={`${s.plugin ?? ""}::${s.name}::${i}`} s={s} locked />
      ))}
    </div>
  );
}

function EmptyState({ title, body }: { title: string; body: React.ReactNode }) {
  return (
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
      <div className="px-7 dim" style={{ letterSpacing: "0.2em", textTransform: "uppercase" }}>
        {title}
      </div>
      <div style={{ fontSize: 11, color: "var(--muted)", lineHeight: 1.5 }}>{body}</div>
    </div>
  );
}
