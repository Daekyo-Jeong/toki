import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CassettePanel } from "./components";

export type AppSettings = {
  notifications_level: string;
  autostart_enabled: boolean;
  track_since_install: boolean;
  install_baseline_cache_read: number;
  active_character: string;
  case_color: string;
  invert_screen: boolean;
  coach_backend: string;
  coach_ollama_model: string;
  always_on_top: boolean;
};

type DataSourceStatus = {
  exists: boolean;
  jsonl_count: number;
  events_count: number;
  path: string;
};

type HookStatus = {
  installed: boolean;
  port: number | null;
  received_count: number;
  /** M9: 훅이 도는 POSIX 셸(Windows는 Git for Windows)이 있나 */
  shell_ok?: boolean;
};

export type CharacterState = {
  lv: number;
  born_at: string;
  total_xp: number;
};

function Toggle({ value, onChange }: { value: boolean; onChange: (v: boolean) => void }) {
  return (
    <button
      onClick={() => onChange(!value)}
      className="pxmono"
      style={{
        all: "unset", cursor: "pointer", display: "inline-flex", alignItems: "center",
        border: "1.5px solid var(--phos)", fontSize: 10, letterSpacing: "0.1em",
      }}
    >
      <span style={{ padding: "3px 7px", background: value ? "var(--phos)" : "transparent", color: value ? "var(--scr)" : "var(--phos)", fontWeight: 700 }}>ON</span>
      <span style={{ padding: "3px 7px", background: !value ? "var(--phos)" : "transparent", color: !value ? "var(--scr)" : "var(--phos)", fontWeight: 700, borderLeft: "1.5px solid var(--phos)" }}>OFF</span>
    </button>
  );
}

function Segmented({ value, options, onChange }: { value: string; options: string[]; onChange: (v: string) => void }) {
  return (
    <div className="pxmono" style={{ display: "inline-flex", border: "1.5px solid var(--phos)" }}>
      {options.map((opt, i) => (
        <button
          key={opt}
          onClick={() => onChange(opt)}
          style={{
            all: "unset", cursor: "pointer", padding: "3px 7px", fontSize: 10, letterSpacing: "0.1em", fontWeight: 700,
            background: opt === value ? "var(--phos)" : "transparent",
            color: opt === value ? "var(--scr)" : "var(--phos)",
            borderLeft: i > 0 ? "1.5px solid var(--phos)" : "none",
          }}
        >
          {opt}
        </button>
      ))}
    </div>
  );
}

function SettingRow({ code, label, sublabel, right }: { code: string; label: string; sublabel?: string; right: React.ReactNode }) {
  return (
    <div style={{ display: "grid", gridTemplateColumns: "20px 1fr auto", gap: 8, alignItems: "center",
      padding: "9px 2px", borderBottom: "1.5px solid var(--phos-14)" }}>
      <div className="pxmono" style={{ fontSize: 10, color: "var(--phos-dim)" }}>{code}</div>
      <div>
        <div style={{ fontSize: 11, letterSpacing: "0.05em" }}>{label}</div>
        {sublabel && <div style={{ fontSize: 10, color: "var(--phos-dim)", marginTop: 2, lineHeight: 1.4 }}>{sublabel}</div>}
      </div>
      {right}
    </div>
  );
}

function daysHoursSince(iso: string, now: Date): string {
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return "—";
  const ms = now.getTime() - t;
  const d = Math.floor(ms / 86_400_000);
  const h = Math.floor((ms % 86_400_000) / 3_600_000);
  return `${d}일 ${h}시간 전`;
}

export function SettingsView({ onClose, state }: { onClose: () => void; state: CharacterState | null }) {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [src, setSrc] = useState<DataSourceStatus | null>(null);
  const [hook, setHook] = useState<HookStatus | null>(null);
  const [confirmingReset, setConfirmingReset] = useState(false);
  const now = new Date();

  useEffect(() => {
    invoke<AppSettings>("get_settings").then(setSettings).catch(console.error);
    invoke<DataSourceStatus>("get_data_source_status").then(setSrc).catch(console.error);
    invoke<HookStatus>("hooks_status").then(setHook).catch(console.error);
    const id = setInterval(() => {
      invoke<HookStatus>("hooks_status").then(setHook).catch(() => {});
    }, 2000);
    return () => clearInterval(id);
  }, []);

  async function toggleHooks(install: boolean) {
    try {
      if (install) {
        await invoke("hooks_install");
      } else {
        await invoke("hooks_uninstall");
      }
      const s = await invoke<HookStatus>("hooks_status");
      setHook(s);
    } catch (e) {
      console.error(e);
      alert(`Hooks ${install ? "install" : "uninstall"} 실패: ${e}`);
    }
  }

  function update(patch: Partial<AppSettings>) {
    if (!settings) return;
    const next = { ...settings, ...patch };
    setSettings(next);
    invoke("set_settings", { settings: next }).catch(console.error);
  }

  async function doReset() {
    await invoke("reset_character").catch(console.error);
    setConfirmingReset(false);
  }

  return (
    <CassettePanel
      title="설정"
      onBack={onClose}
      casing={settings?.case_color ?? "beige"}
      invert={settings?.invert_screen ?? false}
      footer={
        <button
          className="cf-soft auto"
          style={confirmingReset ? { background: "var(--rust)", color: "var(--scr)", borderColor: "var(--rust)" } : undefined}
          onClick={() => (confirmingReset ? doReset() : setConfirmingReset(true))}
        >
          {confirmingReset ? "정말 초기화?" : "데이터 초기화"}
        </button>
      }
    >
      {state && (
        <div className="pxmono" style={{ fontSize: 11, color: "var(--phos-dim)", marginBottom: 12, paddingBottom: 10, borderBottom: "1.5px solid var(--phos-30)" }}>
          Lv.{state.lv} · {state.total_xp.toLocaleString()} 토큰 · {daysHoursSince(state.born_at, now)} 시작
        </div>
      )}

      {settings && (
        <>
          <SettingRow
            code="01"
            label="알림"
            sublabel="마일스톤 OS 토스트"
            right={
              <Segmented
                value={settings.notifications_level.toUpperCase()}
                options={["OFF", "IMPT", "ALL"]}
                onChange={(v) => update({ notifications_level: v.toLowerCase() })}
              />
            }
          />
          <SettingRow
            code="02"
            label="로그인 시 자동 실행"
            sublabel="부팅하면 트레이에 상주"
            right={<Toggle value={settings.autostart_enabled} onChange={(v) => update({ autostart_enabled: v })} />}
          />
          <SettingRow
            code="03"
            label="설치 이후만 집계"
            sublabel="이 옵션 켜기 전 먹은 토큰은 무시"
            right={
              <Toggle
                value={settings.track_since_install}
                onChange={(v) =>
                  update({
                    track_since_install: v,
                    install_baseline_cache_read: v ? settings.install_baseline_cache_read : 0,
                  })
                }
              />
            }
          />
          <SettingRow
            code="04"
            label="Claude Hooks"
            sublabel={
              hook
                ? `${hook.installed ? "✓ 설치됨" : "✗ 미설치"} · 포트 ${hook.port ?? "?"} · ${hook.received_count}건 수신${hook.shell_ok === false ? " · 훅은 Git Bash로 돌아요 — Git for Windows 설치 필요" : ""}`
                : "확인 중…"
            }
            right={hook ? <Toggle value={hook.installed} onChange={(v) => toggleHooks(v)} /> : null}
          />
          <SettingRow
            code="05"
            label="데이터 소스"
            sublabel={
              src
                ? `${src.exists ? "✓" : "✗"} ~/.claude/projects · 파일 ${src.jsonl_count}개 · 이벤트 ${src.events_count.toLocaleString()}건`
                : "확인 중…"
            }
            right={
              <span className="pxmono" style={{ fontSize: 9, padding: "2px 6px", border: "1.5px solid var(--phos)",
                background: src?.exists && src.events_count > 0 ? "var(--phos)" : "transparent",
                color: src?.exists && src.events_count > 0 ? "var(--scr)" : "var(--phos-dim)" }}>
                {src?.exists && src.events_count > 0 ? "OK" : "—"}
              </span>
            }
          />
          <SettingRow
            code="06"
            label="케이스 색"
            sublabel="플라스틱 케이스 색"
            right={
              <Segmented
                value={(settings.case_color || "beige").toUpperCase()}
                options={["BEIGE", "PLATINUM", "ECRU", "SLATE"]}
                onChange={(v) => update({ case_color: v.toLowerCase() })}
              />
            }
          />
          <SettingRow
            code="07"
            label="화면 반전"
            sublabel="페이퍼 포스포 (밝은 화면)"
            right={<Toggle value={settings.invert_screen} onChange={(v) => update({ invert_screen: v })} />}
          />
          <SettingRow
            code="08"
            label="코칭 LLM"
            sublabel={
              settings.coach_backend === "ollama"
                ? `로컬 ${settings.coach_ollama_model} · quota 0 · 밖으로 안 나감`
                : settings.coach_backend === "codex"
                  ? "codex exec · 구독 쿼터 소비"
                  : settings.coach_backend === "claude"
                    ? "claude -p · 5h quota 소비"
                    : "최근 쓴 에이전트 자동 선택 · 구독 쿼터 소비"
            }
            right={
              <Segmented
                value={(settings.coach_backend || "auto").toUpperCase() === "OLLAMA" ? "LOCAL" : (settings.coach_backend || "auto").toUpperCase()}
                options={["AUTO", "CLAUDE", "CODEX", "LOCAL"]}
                onChange={(v) => update({ coach_backend: v === "LOCAL" ? "ollama" : v.toLowerCase() })}
              />
            }
          />
        </>
      )}
    </CassettePanel>
  );
}
