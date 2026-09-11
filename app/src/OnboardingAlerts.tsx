/* v5 M6 — 첫 실행 온보딩 (spec §3 시나리오 A · design-brief §12).
 *
 * **카세트 셸 안에 넣지 않는다.** 아직 앱을 "켜기 전" 단계라 셸 밖이 맞고,
 * 구형 Mac에서 시스템 창이 바탕화면 위에 뜨던 그 모양을 그대로 쓴다 —
 * `AlertDialog` 크롬(줄무늬 타이틀바 + close/zoom + ©1986 tokisoft) 재사용.
 *
 * ⚠️ 데스크는 투명 전체화면 창이고 조상에 transform이 걸려 있어 `position: fixed`가
 * 그 안에 갇힌다. 그래서 **body portal**로 띄운다 (v4에서 메모가 같은 문제를 겪었다).
 *
 * 4단: 인사(감지) → 설치 승인 → 읽는 중(백필) → 부화.
 */
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { AlertDialog, DEFAULT_PET_ROWS } from "./TamagotchiShell";

/** 설치 안내 링크 — 앱 안에 웹뷰를 띄우지 않고 기본 브라우저로 넘긴다. */
async function openExternal(url: string) {
  try {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url);
  } catch {
    /* 프리뷰(브라우저)에선 플러그인이 없다 — 조용히 넘어간다 */
  }
}

type DetectedAgent = {
  id: string; label: string; detected: boolean;
  path: string; log_files: number; install_url: string;
  /** M9: 실행 파일이 보이나 — 기록 폴더만 있고 CLI 가 없는 경우가 있다 */
  cli: string | null; install_cmd: string;
};

async function copyText(text: string): Promise<boolean> {
  try { await navigator.clipboard.writeText(text); return true; } catch { return false; }
}
export type OnboardingStatus = {
  needed: boolean; agents: DetectedAgent[]; any_agent: boolean; events: number;
};
type HookStatus = {
  installed: boolean; statusline_installed: boolean;
  codex_available: boolean; codex_installed: boolean;
};

/* 알 — 아직 부화 전. 16x16 격자를 지킨다(§2 픽셀 격자). */
const P_EGG = [
  "................", "......####......", ".....######.....", "....########....",
  "...##########...", "..############..", "..####....####..", ".####..##..####.",
  ".####..##..####.", ".##############.", ".##############.", "..############..",
  "..############..", "...##########...", ".....######.....", "................",
];

type Step = "hello" | "install" | "reading" | "hatch";

/** 진행 표기용 — 0개 분기는 설치·읽는 중을 건너뛰지만 번호는 고정으로 둔다. */
const STEP_NO: Record<Step, number> = { hello: 1, install: 2, reading: 3, hatch: 4 };

export function OnboardingAlerts({ status, onDone }: {
  status: OnboardingStatus;
  onDone: () => void;
}) {
  const [copied, setCopied] = useState<string | null>(null);
  const [step, setStep] = useState<Step>("hello");
  const [installing, setInstalling] = useState(false);
  const [installNote, setInstallNote] = useState<string | null>(null);
  const [lv, setLv] = useState<number | null>(null);
  const [events, setEvents] = useState(status.events);

  /* 포커스를 다이얼로그 안에 가둔다. 안 그러면 Tab 첫 정거장이 **뒤의 셸**이고
     Enter가 실제로 먹혀, 온보딩도 안 끝낸 앱이 조작된다(2026-09-03 실측). */
  const trapRef = useRef<HTMLDivElement | null>(null);
  // 데스크(셸·메모)를 비활성으로 만드는 신호. 언마운트 때 반드시 되돌린다.
  useEffect(() => {
    document.documentElement.dataset.onboarding = "1";
    return () => { delete document.documentElement.dataset.onboarding; };
  }, []);
  useEffect(() => {
    const first = trapRef.current?.querySelector<HTMLElement>("button:not(:disabled)");
    first?.focus();
  }, [step]);
  function onTrapKey(e: React.KeyboardEvent) {
    if (e.key !== "Tab") return;
    const els = Array.from(
      trapRef.current?.querySelectorAll<HTMLElement>("button:not(:disabled)") ?? [],
    );
    if (els.length === 0) return;
    e.preventDefault();
    const i = els.indexOf(document.activeElement as HTMLElement);
    const next = e.shiftKey ? (i <= 0 ? els.length - 1 : i - 1) : (i + 1) % els.length;
    els[next].focus();
  }

  const found = status.agents.filter((a) => a.detected);
  const totalFiles = found.reduce((n, a) => n + a.log_files, 0);
  const hasClaude = found.some((a) => a.id === "claude");
  const hasCodex = found.some((a) => a.id === "codex");

  /* 3단 "읽는 중" — 백필은 앱이 이미 백그라운드로 돌리고 있다. 여기선 그
     진행을 **보여주기만** 한다: 이벤트 수가 늘다가 멈추면 다음으로.
     자동 전이라 버튼이 없다(design-brief §12). */
  const settled = useRef(0);
  const [slow, setSlow] = useState(false);
  const [dots, setDots] = useState(1);
  useEffect(() => {
    if (step !== "reading") return;
    let alive = true;
    let last = -1;
    /* 정착 판정은 **이 콜백 안에서** 한다. 예전엔 `[events]` effect에서 봤는데,
       수가 평탄해지면 같은 값이라 리렌더가 안 일어나 그 effect가 다시 안 돌았다
       → 완료 신호가 죽고 항상 12초 상한만 썼다(실측 12.03~12.10초). */
    const tick = async () => {
      try {
        const s = await invoke<{ events_count: number }>("get_data_source_status");
        if (!alive) return;
        if (s.events_count === last) settled.current += 1;
        else settled.current = 0;
        last = s.events_count;
        setEvents(s.events_count);
        if (settled.current >= 3) setStep("hatch");
      } catch { /* 스캔 중 일시 실패는 무시 — 다음 틱에 다시 본다 */ }
    };
    void tick();
    const iv = window.setInterval(tick, 700);
    const blink = window.setInterval(() => setDots((d) => (d % 3) + 1), 350);
    // 상한에 걸리면 자동으로 넘기지 않고 **탈출 버튼**을 준다 — 이유를 모른 채
    // 멈춘 화면을 보는 것보다 낫다.
    const cap = window.setTimeout(() => { if (alive) setSlow(true); }, 12_000);
    return () => {
      alive = false;
      window.clearInterval(iv); window.clearInterval(blink); window.clearTimeout(cap);
    };
  }, [step]);

  useEffect(() => {
    if (step !== "hatch") return;
    invoke<{ lv: number }>("get_level_info").then((i) => setLv(i.lv)).catch(() => setLv(1));
  }, [step]);

  async function approveInstall() {
    setInstalling(true);
    setInstallNote("설치 중…");
    const hook = await invoke<HookStatus>("hooks_status").catch(() => null);
    const jobs: Promise<unknown>[] = [];
    if (hasClaude) {
      if (!hook?.installed) jobs.push(invoke("hooks_install"));
      if (!hook?.statusline_installed) jobs.push(invoke("statusline_install"));
    }
    if (hook?.codex_available && !hook.codex_installed) jobs.push(invoke("codex_hooks_install"));
    const results = await Promise.allSettled(jobs);
    const failed = results.filter((r) => r.status === "rejected");
    setInstalling(false);
    if (failed.length > 0) {
      // 실패를 1초 보여주고 지나가면 사용자는 뭘 놓쳤는지 영영 모른다.
      // 자동 전이를 멈추고 이유를 남긴 채 선택을 준다(막다른 길은 아니다 —
      // JSONL만으로 기본 기능은 돌고, 설정에서 언제든 다시 켤 수 있다).
      const why = failed
        .map((r) => String((r as PromiseRejectedResult).reason))
        .join(" · ")
        .slice(0, 120);
      setInstallNote(`${failed.length}개 실패 — ${why}`);
      return;
    }
    setInstallNote(null);
    setStep("reading");
  }

  function finish() {
    invoke("onboarding_finish").catch(() => {});
    onDone();
  }

  let dialog: React.ReactNode = null;
  if (step === "hello") {
    dialog = (
      <AlertDialog
        title="Toki · 1/4" width={236} copyright="©1986 tokisoft"
        ok={status.any_agent ? "시작" : "그래도 시작"}
        onOk={() => setStep(status.any_agent ? "install" : "hatch")}
      >
        <div style={{ textAlign: "left", fontSize: 12, lineHeight: "18px" }}>
          <div style={{ marginBottom: 6 }}>안녕하세요, 토키예요.<br />코딩 에이전트를 쓰면 제가 자라요.</div>
          {status.agents.map((a) => (
            <div key={a.id} style={{ display: "flex", justifyContent: "space-between", gap: 8, color: a.detected ? "var(--phos)" : "var(--phos-dim)" }}>
              <span>{a.detected ? "✓" : "·"} {a.label}</span>
              <span style={{ fontFamily: "var(--pixel-mono-9)", fontSize: 10 }}>
                {a.detected ? `${a.log_files}개 기록` : "없음"}{a.cli ? "" : " · CLI 없음"}
              </span>
            </div>
          ))}
          {/* CLI 가 없는 에이전트는 **설치 명령을 복사**시킨다 — 링크만 주면 문서에서
              헤맨다(대교 실기 2026-09-11: "설치 안내 필요, 세팅까지"). 설치 뒤
              다음 화면(2/4)이 훅·사용량 연동까지 이어서 건다. */}
          {status.agents.some((a) => !a.cli) && (
            <div style={{ marginTop: 6, color: "var(--phos-dim)", fontSize: 10, lineHeight: "15px", fontFamily: "var(--pixel-9)" }}>
              {status.any_agent ? "CLI 가 안 보여요. 터미널에 붙여넣어 설치해요." : "둘 다 못 찾았어요. 하나를 설치하고 다시 켜면 그때부터 자라요."}
              {status.agents.filter((a) => !a.cli).map((a) => (
                <div key={a.id} style={{ display: "flex", gap: 6, marginTop: 4, alignItems: "center" }}>
                  <button className="cf-soft auto" style={{ fontSize: 10 }}
                    onClick={() => copyText(a.install_cmd).then((ok) => { if (ok) { setCopied(a.id); window.setTimeout(() => setCopied(null), 2000); } })}>
                    {copied === a.id ? "복사됨" : `${a.label} 설치 명령 복사`}
                  </button>
                  <button className="cf-soft auto" style={{ fontSize: 10 }} onClick={() => openExternal(a.install_url)}>안내</button>
                </div>
              ))}
            </div>
          )}
        </div>
      </AlertDialog>
    );
  } else if (step === "install") {
    dialog = (
      <AlertDialog
        title="설치 · 2/4" width={236} copyright="©1986 tokisoft"
        ok={installing ? "설치 중…" : installNote ? "다시 시도" : "허용"}
        onOk={approveInstall} okDisabled={installing}
        cancel={installNote ? "건너뛰기" : "나중에"}
        onCancel={() => setStep("reading")} cancelDisabled={installing}
      >
        <div style={{ textAlign: "left", fontSize: 12, lineHeight: "18px" }}>
          <div style={{ marginBottom: 6 }}>기록을 바로바로 받으려면 설정 파일에 한 줄씩 넣어야 해요.</div>
          <div style={{ fontFamily: "var(--pixel-9)", fontSize: 10, lineHeight: "15px", color: "var(--phos-dim)" }}>
            {/* 목록은 **실제 감지 결과**를 따른다. 없는 항목을 숨기지는 않되
                (조건부 숨김 금지) 왜 건너뛰는지 같은 줄에서 말한다 — 앞 화면에서
                "Codex 없음"이라 해놓고 Codex 훅을 안내하면 카피와 실행이 어긋난다. */}
            {[
              { on: hasClaude, label: "훅", path: "~/.claude/settings.json" },
              { on: hasClaude, label: "상태줄", path: "사용률 표시용" },
              { on: hasCodex, label: "Codex 훅", path: "~/.codex/hooks.json" },
            ].map((row, i) => (
              <div key={i} style={{ opacity: row.on ? 1 : 0.45 }}>
                · {row.label} — <span style={{ color: row.on ? "var(--phos)" : "inherit" }}>{row.path}</span>
                {!row.on && " (없어서 건너뛰어요)"}
              </div>
            ))}
            <div style={{ marginTop: 5 }}>
              쓰던 설정은 백업하고 이어 붙여요. 설정에서 언제든 끌 수 있어요.
              {hasCodex && <><br />Codex는 처음 한 번 codex 안에서 훅 신뢰를 승인해야 해요.</>}
            </div>
            {installNote && <div style={{ marginTop: 5, color: "var(--phos)" }}>{installNote}</div>}
          </div>
        </div>
      </AlertDialog>
    );
  } else if (step === "reading") {
    dialog = (
      <AlertDialog
        title="읽는 중 · 3/4" width={236} copyright="©1986 tokisoft"
        ok={slow ? "먼저 만나기" : ""} onOk={slow ? () => setStep("hatch") : undefined}
      >
        <div style={{ fontSize: 12, lineHeight: "18px" }}>
          지난 기록을 읽고 있어요{".".repeat(dots)}
          <div style={{ marginTop: 4, fontFamily: "var(--pixel-mono-9)", fontSize: 10, color: "var(--phos-dim)" }}>
            파일 {totalFiles}개 · 이벤트 {events.toLocaleString()}
          </div>
          {slow && (
            <div style={{ marginTop: 5, fontFamily: "var(--pixel-9)", fontSize: 10, lineHeight: "15px", color: "var(--phos-dim)" }}>
              생각보다 오래 걸려요.<br />먼저 만나도 읽기는 계속돼요.
            </div>
          )}
        </div>
      </AlertDialog>
    );
  } else {
    dialog = (
      <AlertDialog
        title="부화 · 4/4" width={236} copyright="©1986 tokisoft"
        sprite={lv === null ? P_EGG : DEFAULT_PET_ROWS} spriteScale={4}
        ok="만나기" onOk={finish} okDisabled={lv === null}
      >
        {lv === null ? "…" : (
          <>
            {events > 0 ? `지난 기록으로 Lv.${lv}까지 자랐어요.` : "이제 막 태어났어요."}
            {"\n"}앞으로 같이 일해요.
          </>
        )}
      </AlertDialog>
    );
  }

  /* 백드롭은 셸의 DlgStage가 아니라 **투명 데스크** 위다. 셸이 아직 없으니 딤도 옅게.
   *
   * ⚠️ 백드롭에 `.desk-ctl`을 주면 안 된다 — MemoDesk가 그 클래스를 훑어 OS
   * 클릭 렉트로 올리는데, `inset:0`이라 화면 전체가 통짜로 잡힌다. 데스크는
   * always-on-top 투명 전체화면 창이라, 첫 실행 사용자가 Dock도 Finder도 못
   * 누르게 된다(2026-09-03 검수에서 실측). 이 얼러트는 백드롭 클릭으로 닫히지도
   * 않으니 전체를 잡을 이유가 아예 없다 → **다이얼로그 박스만** desk-ctl로.
   */
  return createPortal(
    <div style={{
      position: "fixed", inset: 0, zIndex: 300, display: "flex",
      alignItems: "center", justifyContent: "center",
      background: "rgba(14,12,18,.28)", pointerEvents: "none",
      WebkitUserSelect: "none", userSelect: "none",
    }}>
      <div
        ref={trapRef}
        className="desk-ctl"
        role="dialog"
        aria-modal="true"
        aria-label={`토키 시작 ${STEP_NO[step]}/4`}
        style={{ pointerEvents: "auto" }}
        onKeyDown={onTrapKey}
      >
        {dialog}
      </div>
    </div>,
    document.body,
  );
}

/** 3단에서 보여줄 알 스프라이트 — 프리뷰 하네스가 쓴다. */
export { P_EGG };
