import { invoke } from "@tauri-apps/api/core";
import { Frame, HeaderBar } from "./components";

export function MigrationDialog({
  onResolved,
}: {
  onResolved: (choice: "fresh" | "keep") => void;
}) {
  async function choose(choice: "fresh" | "keep") {
    await invoke("resolve_migration_dialog", { choice }).catch(console.error);
    onResolved(choice);
  }
  return (
    <Frame>
      <HeaderBar />
      <div style={{ padding: "16px 14px", fontSize: 11, lineHeight: 1.6, flex: 1 }}>
        <div className="up dim" style={{ fontSize: 9, marginBottom: 8 }}>NOTICE</div>
        <p style={{ margin: 0 }}>
          v1 데이터 (~/.toki/legacy-backup/)가 발견됐어요.
        </p>
        <p style={{ marginTop: 12 }}>
          캐릭터를 어떻게 시작할까요?
        </p>
        <div style={{ marginTop: 14, padding: "8px 10px", background: "var(--soft)" }}>
          <div style={{ fontWeight: 700 }}>처음부터 시작</div>
          <div className="dim" style={{ marginTop: 2 }}>
            기존 누적 토큰은 baseline으로 고정 → Lv 1 알부터 키움. <strong>추천.</strong>
          </div>
        </div>
        <div style={{ marginTop: 8, padding: "8px 10px", background: "var(--soft)" }}>
          <div style={{ fontWeight: 700 }}>현재 누적 유지</div>
          <div className="dim" style={{ marginTop: 2 }}>
            기존 cache_read 1G+ 그대로 XP로 계산 → 즉시 만렙(99) 도달.
          </div>
        </div>
      </div>
      <div style={{ display: "flex", borderTop: "2px solid var(--line)" }}>
        <button
          className="btn btn-acc"
          style={{ flex: 1, padding: "12px 0" }}
          onClick={() => choose("fresh")}
        >
          처음부터 시작
        </button>
        <button
          className="btn"
          style={{ flex: 1, padding: "12px 0", borderLeft: "none" }}
          onClick={() => choose("keep")}
        >
          누적 유지
        </button>
      </div>
    </Frame>
  );
}
