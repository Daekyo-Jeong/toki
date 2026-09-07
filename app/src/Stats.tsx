import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Frame, HeaderBar, SubHeader, RuleHead } from "./components";

type Bucket = { label: string; tokens: number };
type UsageSummary = {
  last_5h: number;
  today: number;
  last_7d: number;
  by_model: Bucket[];
  by_project: Bucket[];
};

function compact(n: number): string {
  if (n >= 1e9) return (n / 1e9).toFixed(1) + "B";
  if (n >= 1e6) return (n / 1e6).toFixed(1) + "M";
  if (n >= 1e3) return (n / 1e3).toFixed(1) + "K";
  return Math.round(n).toString();
}

function shortProject(path: string): string {
  if (!path) return "—";
  const last = path.split("/").filter(Boolean).pop() ?? path;
  if (last.startsWith("-")) return last.split("-").pop() ?? last;
  return last;
}

function shortModel(model: string): string {
  const m = model.toLowerCase();
  if (m.includes("opus")) return "Opus";
  if (m.includes("sonnet")) return "Sonnet";
  if (m.includes("haiku")) return "Haiku";
  return model;
}

function BucketRow({ label, tokens, total }: { label: string; tokens: number; total: number }) {
  const pct = total > 0 ? Math.round((tokens / total) * 100) : 0;
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "1fr auto",
        gap: 6,
        padding: "6px 10px",
        borderBottom: "1px dashed var(--line)",
        fontSize: 11,
      }}
    >
      <div>
        <div style={{ display: "flex", justifyContent: "space-between", marginBottom: 4 }}>
          <span style={{ fontWeight: 700 }}>{label}</span>
          <span className="tnum dim">{compact(tokens)}</span>
        </div>
        <div style={{ position: "relative", height: 6, border: "1px solid var(--line)", background: "var(--bg)" }}>
          <div style={{ position: "absolute", inset: 0, width: `${pct}%`, background: "var(--fg)" }} />
        </div>
      </div>
      <div className="tnum" style={{ alignSelf: "end", fontSize: 11, fontWeight: 700, minWidth: 30, textAlign: "right" }}>
        {pct}%
      </div>
    </div>
  );
}

function WindowCell({ label, value }: { label: string; value: number }) {
  return (
    <div style={{ padding: "8px 10px", borderRight: "1px solid var(--line)", flex: 1, minWidth: 0 }}>
      <div className="section-label dim">{label}</div>
      <div className="tnum" style={{ fontWeight: 800, fontSize: 16, marginTop: 2 }}>{compact(value)}</div>
    </div>
  );
}

type NavKey = "stat" | "log" | "map" | "inv" | "cfg";
export function StatsView({
  onClose,
  onNav,
  pinned,
}: {
  onClose: () => void;
  onNav?: (k: NavKey) => void;
  pinned?: boolean;
}) {
  const [s, setS] = useState<UsageSummary | null>(null);

  useEffect(() => {
    const load = () =>
      invoke<UsageSummary>("get_usage_summary").then(setS).catch(console.error);
    load();
    const id = setInterval(load, 5000);
    return () => clearInterval(id);
  }, []);

  const modelTotal = s ? s.by_model.reduce((a, b) => a + b.tokens, 0) : 0;
  const projectTotal = s ? s.by_project.reduce((a, b) => a + b.tokens, 0) : 0;

  return (
    <Frame>
      <HeaderBar active="stat" onNav={onNav} pinned={pinned} />
      <SubHeader title="STATS · USAGE" code="ST" back="MAIN" onBack={onClose} />

      <div style={{ display: "flex", borderBottom: "2px solid var(--line)" }}>
        <WindowCell label="LAST 5H" value={s?.last_5h ?? 0} />
        <WindowCell label="TODAY" value={s?.today ?? 0} />
        <div style={{ padding: "8px 10px", flex: 1, minWidth: 0 }}>
          <div className="section-label dim">LAST 7D</div>
          <div className="tnum" style={{ fontWeight: 800, fontSize: 16, marginTop: 2 }}>{compact(s?.last_7d ?? 0)}</div>
        </div>
      </div>

      <RuleHead code="01" label="BY MODEL · 7D" right={s ? compact(modelTotal) : ""} />
      <div style={{ overflowY: "auto", maxHeight: 140 }}>
        {s && s.by_model.length > 0 ? (
          s.by_model.map((b) => (
            <BucketRow key={b.label} label={shortModel(b.label)} tokens={b.tokens} total={modelTotal} />
          ))
        ) : (
          <div style={{ padding: 12, fontSize: 11, color: "var(--muted)" }}>&gt; no model usage in window</div>
        )}
      </div>

      <RuleHead code="02" label="BY PROJECT · 7D · TOP 5" right={s ? compact(projectTotal) : ""} />
      <div style={{ flex: 1, overflowY: "auto" }}>
        {s && s.by_project.length > 0 ? (
          s.by_project.map((b) => (
            <BucketRow key={b.label} label={shortProject(b.label)} tokens={b.tokens} total={projectTotal} />
          ))
        ) : (
          <div style={{ padding: 12, fontSize: 11, color: "var(--muted)" }}>&gt; no project activity in window</div>
        )}
      </div>

      <div
        style={{
          padding: "6px 10px",
          borderTop: "1px dashed var(--line)",
          fontSize: 10,
          color: "var(--muted)",
          letterSpacing: "0.12em",
        }}
      >
        cache_read tokens · rolling windows · share within window
      </div>
    </Frame>
  );
}
