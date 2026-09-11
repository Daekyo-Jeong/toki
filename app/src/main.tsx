import React, { useEffect, useState } from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import App from "./App";
import { TamagotchiShell } from "./TamagotchiShell";
import { MemoDesk } from "./MemoDesk";
import { OnboardingAlerts, type OnboardingStatus } from "./OnboardingAlerts";

/** v5 M6 분기 하네스 — 감지된 에이전트 0/1/2개를 데스크 위에서 눈으로 본다. */
function OnboardingPreview({ n }: { n: number }) {
  const [status, setStatus] = useState<OnboardingStatus | null>(null);
  const [done, setDone] = useState(false);
  useEffect(() => {
    invoke<OnboardingStatus>("onboarding_status").then(setStatus).catch(() => {});
  }, [n]);
  return (
    <>
      <MemoDesk lv={4} hungry={false} hasCoaching={true} />
      {status && !done && <OnboardingAlerts status={status} onDone={() => setDone(true)} />}
    </>
  );
}

// Alert triggers fire on a false→true transition (anti-spam), so a static
// `?hungry=1` prop at mount never fires them. Delay the flip so the preview
// harness actually exercises the rising-edge path.
function ShellPreview({ hungry, hasCoaching, lv, lvbump, initialMode }: {
  hungry: boolean; hasCoaching: boolean; lv: number; lvbump: boolean;
  initialMode?: "home" | "menu" | "pomo" | "appearance" | "coaching" | "settings";
}) {
  const [h, setH] = useState(false);
  const [c, setC] = useState(false);
  // lvbump: start one level below and bump up to `lv` after mount so the
  // unlock rising-edge (new-pet alert) fires, exercising V4-5.
  const [curLv, setCurLv] = useState(lvbump ? lv - 1 : lv);
  useEffect(() => {
    const t1 = hungry ? window.setTimeout(() => setH(true), 300) : undefined;
    const t2 = hasCoaching ? window.setTimeout(() => setC(true), 300) : undefined;
    const t3 = lvbump ? window.setTimeout(() => setCurLv(lv), 400) : undefined;
    return () => { [t1, t2, t3].forEach((t) => t && window.clearTimeout(t)); };
  }, [hungry, hasCoaching, lvbump, lv]);
  return <TamagotchiShell lv={curLv} hungry={h} hasCoaching={c} initialMode={initialMode} />;
}

// Dev-only isolated preview of the cassette shell: App's real render is
// gated behind `invoke("get_save")` etc, which never resolve outside a
// Tauri window (plain browser preview would otherwise show a blank page).
// `?shell` bypasses that gate with mock props for fast visual iteration.
// Query params: hungry=1, hasCoaching=1, case=beige|platinum|ecru|slate, invert=1,
// mode=home|menu|pomo|appearance|coaching|settings (jump straight in).
const params = new URLSearchParams(location.search);
let root = <App />;
if (import.meta.env.DEV && params.has("shell")) {
  const mockSettings = {
    notifications_level: "impt", autostart_enabled: false, track_since_install: false,
    install_baseline_cache_read: 0, active_character: "bunny",
    case_color: params.get("case") || "beige", invert_screen: params.get("invert") === "1",
    coach_backend: "auto", coach_ollama_model: "exaone3.5:7.8b", always_on_top: true,
  };
  const mockHook = { installed: true, port: 6996, received_count: 789,
    statusline_installed: true, codex_available: true, codex_installed: false };
  let mockMemoryPaste = "";
  if (!(window as any).__TAURI_INTERNALS__) {
    (window as any).__TAURI_INTERNALS__ = {
      invoke: (cmd: string, args?: any) => {
        if (cmd === "get_settings") return Promise.resolve({ ...mockSettings });
        if (cmd === "user_pets") return Promise.resolve([
          { id: "walrus", name: "엄니", states: { idle: { fps: 3, frames: [
            ["....####....", ".#..@..@..#.", "..##.##.##..", "....#..#...."],
            ["............", "....####....", ".#..@..@..#.", "..##.##.##.."],
          ] } } },
        ]);
        if (cmd === "coach_personas") return Promise.resolve([
          { id: "bunny", name: "깡총이", desc: "밝고 경쾌하게 말해요. 리듬감 있는 짧은 문장을 즐겨 써요.", example: "좋아요!" },
          { id: "dong", name: "동글이", desc: "차분하고 다정한 기본 톤이에요. 꾸밈없이 또박또박 짚어요.", example: "" },
          { id: "cat", name: "뾰족이", desc: "짧고 담백하게 끊어 말해요. 수식어 없이 핵심만 콕 집어요.", example: "짧게 짚을게요." },
          { id: "bear", name: "곰이", desc: "포근하고 느긋하게 말해요. 짚을 건 짚되 부드럽게 감싸요.", example: "천천히 봐도 괜찮아요." },
        ]);
        if (cmd === "get_skill_trend") return Promise.resolve({
          now: { sessions: 30, edits: 324, reworks: 8, one_shot: 97.5, prompts: 142, short_prompts: 6, delegation: 4.2, max_chain: 1 },
          prev: { sessions: 104, edits: 430, reworks: 49, one_shot: 88.6, prompts: 356, short_prompts: 0, delegation: 0, max_chain: 3 },
          computed_at: new Date(Date.now() - 12 * 60_000).toISOString(),
        });
        if (cmd === "get_level_info") return Promise.resolve({
          lv: 50, at_cap: false, xp: 12_167_193_166,
          into_level: 167_193_166, span: 495_000_000, to_next: 327_806_834, progress: 0.338,
          total_input: 8_300_000, total_output: 45_100_000, total_cache_creation: 1_420_000_000, total_cache_read: 12_167_193_166,
        });
        if (cmd === "set_settings") { Object.assign(mockSettings, args?.settings); return Promise.resolve(null); }
        if (cmd === "get_data_source_status") return Promise.resolve({ exists: true, jsonl_count: 42, events_count: 12345, path: "~/.claude/projects" });
        if (cmd === "get_usage_snapshot") return Promise.resolve({ active_block_tokens: Number(params.get("usage") ?? 56), token_limit: 100, source: "oauth_usage" });
        if (cmd === "hooks_status") return Promise.resolve({ ...mockHook });
        if (cmd === "hooks_install") { mockHook.installed = true; return Promise.resolve(null); }
        if (cmd === "hooks_uninstall") { mockHook.installed = false; return Promise.resolve(null); }
        // v5 M6 온보딩 분기 하네스: ?onboard=0|1|2 (감지된 에이전트 수).
        if (cmd === "onboarding_status") {
          const n = Number(params.get("onboard") ?? -1);
          const mk = (id: string, label: string, on: boolean, files: number, url: string) =>
            ({ id, label, detected: on, path: `~/.${id}`, log_files: on ? files : 0, install_url: url, cli: on ? `/usr/local/bin/${id}` : null, install_cmd: `install ${id}` });
          const agents = [
            mk("claude", "Claude Code", n >= 1, 128, "https://claude.com/claude-code"),
            mk("codex", "Codex CLI", n >= 2, 80, "https://developers.openai.com/codex/cli"),
          ];
          return Promise.resolve({
            needed: n >= 0, agents, any_agent: n >= 1, events: 0,
          });
        }
        if (cmd === "onboarding_finish") return Promise.resolve(null);
        if (cmd === "coaching_deep_preview") return Promise.resolve({
          backend: "claude:sonnet", backend_kind: "claude", egresses: true,
          n_prompts: 351, n_days: 30, has_inventory: true,
          sources: [{ label: "Claude 메모리", kind: "memory", path: "~/.claude/projects/*/memory/MEMORY.md", bytes: 11_200 }],
        });
        if (cmd === "memory_paste_get") return Promise.resolve(mockMemoryPaste);
        if (cmd === "memory_paste_set") { mockMemoryPaste = args?.text ?? ""; return Promise.resolve(mockMemoryPaste.length); }
        if (cmd === "prompts_reset") return Promise.resolve(0);
        // M8: ?update=1 로 업데이트 얼러트를 띄워 본다.
        if (cmd === "update_check") return Promise.resolve(
          params.get("update") === "1"
            ? { current: "1.0.0", latest: "1.1.0", available: true }
            : { current: "1.0.0", latest: "1.0.0", available: false },
        );
        if (cmd === "update_run") return Promise.resolve(null);
        if (cmd === "coaching_deep_latest") return Promise.resolve(
          params.get("cached") === "1"
            ? { n_prompts: 351, n_days: 30, model: "sonnet",
                generated_at: new Date(Date.now() - 3 * 3600_000).toISOString(),
                body: "## 지금 상태\n30일 동안 세션 15개가 프로젝트 셋에 몰려 있고, 훅·statusline 층위는 비어 있어요.\n\n## 추천\n### 1. 릴리스 체크리스트 — 스킬\n- **근거**: 3주에 걸쳐 같은 릴리스 절차를 채팅으로 재확인했어요.\n- **무엇을**: 릴리스 순서를 체크리스트로 강제하는 스킬.\n- **어떻게**: `~/.claude/skills/release/SKILL.md` 신설.\n- **확인**: 다음 릴리스에서 순서 질문 왕복이 줄면 성공.\n\n## 한 줄 결론\n릴리스 스킬부터 만들어요." }
            : null,
        );
        if (cmd === "retro_project_list") return Promise.resolve([
          { dir: "d1", label: "toki", last_active: new Date(Date.now() - 5 * 60_000).toISOString(), sessions: 6 },
          { dir: "d2", label: "my-app", last_active: new Date(Date.now() - 2 * 3600_000).toISOString(), sessions: 4 },
          { dir: "d3", label: "side-project", last_active: new Date(Date.now() - 26 * 3600_000).toISOString(), sessions: 17 },
        ]);
        if (cmd === "retro_project_generate" || cmd === "session_retro_latest") return new Promise((r) => setTimeout(() => r(
          "## 이번 세션 한 줄\n메모·코칭 기능을 손봐달라 했고, 여러 파일을 고쳐 마무리했어요.\n\n## 의도 — 어떻게 명령했나\n첫 지시(**프롬프트 1**)는 두루뭉술했고, **프롬프트 3**에서야 구체화됐어요.\n\n## 과정 — 어디서 꼬였나\n에러 몇 건이 한 지점에서 났고, 중간에 방향을 한 번 되돌렸어요.\n\n## 더 나았을 길\n- 첫 프롬프트에 완료 기준 한 줄을 붙였다면 뒤의 재설명을 아낄 수 있었을 거예요.\n\n## 관성 점검\n증상보다 '고쳐줘'를 먼저 던지는 흐름, 매번 반복되진 않나요?",
        ), 400));
        return Promise.resolve(null);
      },
    };
  }
  const mode = params.get("mode") as
    | "home" | "menu" | "pomo" | "appearance" | "coaching" | "settings" | null;
  // v5 M6: ?shell&onboard=0|1|2 — 데스크 위에 온보딩 시퀀스를 얹어 분기를 본다.
  root = params.has("onboard")
    ? <OnboardingPreview n={Number(params.get("onboard"))} />
    : params.get("desk") === "1"
    ? <MemoDesk lv={4} hungry={false} hasCoaching={true} />
    : (
      <ShellPreview
        lv={Number(params.get("lv") ?? 4)}
        hungry={params.get("hungry") === "1"}
        hasCoaching={params.get("hasCoaching") === "1"}
        lvbump={params.get("lvbump") === "1"}
        initialMode={mode ?? undefined}
      />
    );
}

// non-Retina 가독성 (2026-08-10): CSS 미디어 쿼리만으로는 멀티 디스플레이에서
// 창이 다른 화면으로 옮겨갈 때 재평가가 안 붙는 경우가 있어, JS로 직접 감지해
// <html data-dpi="low|hi">를 찍는다. matchMedia 리스너로 이동 시에도 갱신.
// (실제 인식값은 정보 화면에 노출 — 안 먹을 때 원인을 눈으로 가르기 위함.)
function applyDpiClass() {
  const dpr = window.devicePixelRatio || 1;
  document.documentElement.dataset.dpi = dpr < 1.5 ? "low" : "hi";
  document.documentElement.dataset.dpr = String(dpr);
}
applyDpiClass();
try {
  matchMedia("(resolution: 1dppx)").addEventListener("change", applyDpiClass);
} catch { /* 구형 WebKit: 초기 1회 적용으로 만족 */ }
window.addEventListener("resize", applyDpiClass);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{root}</React.StrictMode>,
);
