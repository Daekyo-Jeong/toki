/**
 * V4-6 §3.2 — cross-session habit coaching surface.
 *
 * Distills the last N sessions and coaches on habits that only appear
 * across sessions (recurring frictions, time-of-day errors, delegation
 * inertia, trend). Backed by `coaching_cross_generate` (local Ollama by
 * default → zero 5h quota). Detection is deterministic in Rust; the LLM
 * only phrases.
 */

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CassettePanel, renderMarkdown } from "./components";
import type { AppSettings } from "./Settings";

const LIMIT = 20;

export function CrossCoachingView({ onClose }: { onClose: () => void }) {
  const [body, setBody] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  // Generation is slow (distill N transcripts + one local LLM call, ~20-40s).
  // A live elapsed counter reassures the user it's working, not hung.
  const [elapsed, setElapsed] = useState(0);
  const started = useRef(false);
  // Only for case-color/invert consistency with the rest of the app —
  // coaching itself doesn't otherwise touch settings.
  const [settings, setSettings] = useState<AppSettings | null>(null);

  function run() {
    if (loading) return;
    setLoading(true);
    setError(null);
    setBody(null);
    setElapsed(0);
    invoke<string>("coaching_cross_generate", { limit: LIMIT })
      .then((b) => setBody(b))
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }

  // Auto-run once on first mount.
  useEffect(() => {
    invoke<AppSettings>("get_settings").then(setSettings).catch(() => {});
    if (started.current) return;
    started.current = true;
    run();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!loading) return;
    const id = window.setInterval(() => setElapsed((e) => e + 1), 1000);
    return () => window.clearInterval(id);
  }, [loading]);

  return (
    <CassettePanel
      title="코칭 · 크로스세션"
      onBack={onClose}
      casing={settings?.case_color ?? "beige"}
      invert={settings?.invert_screen ?? false}
      footer={
        <button className="cf-soft auto primary" onClick={run} disabled={loading}>
          {loading ? "…" : "다시 생성"}
        </button>
      }
    >
      {loading && (
        <div className="pxmono" style={{ color: "var(--phos-dim)", fontSize: 11 }}>
          최근 {LIMIT}개 세션을 분석하는 중… ({elapsed}s)
          <div style={{ marginTop: 4, fontSize: 10 }}>로컬 LLM으로 돌려요 — 사용량 quota는 안 먹어요.</div>
        </div>
      )}
      {error && !loading && (
        <div className="pxmono" style={{ fontSize: 11 }}>
          <div style={{ fontWeight: 700, marginBottom: 4 }}>코칭을 만들지 못했어요</div>
          <div style={{ color: "var(--phos-dim)", overflowWrap: "anywhere" }}>{error}</div>
        </div>
      )}
      {body && !loading && <div style={{ fontSize: 12, lineHeight: 1.6 }}>{renderMarkdown(body)}</div>}
    </CassettePanel>
  );
}
