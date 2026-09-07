import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./tokens.css";
import { DungeonLogView } from "./DungeonLog";
import { DungeonsView } from "./Dungeons";
import { OnboardingAlerts, type OnboardingStatus } from "./OnboardingAlerts";
import { StatsView } from "./Stats";
import { MigrationDialog } from "./MigrationDialog";
import { InventoryView } from "./Inventory";
import { RetrospectiveView } from "./Retrospective";
import { MemoDesk } from "./MemoDesk";
import { retroQueue } from "./retroQueue";
import { getCurrentWindow, currentMonitor, LogicalSize, PhysicalSize, PhysicalPosition } from "@tauri-apps/api/window";

type CharacterSave = {
  lv: number;
  xp: number;
  hp: number;
  hp_max: number;
  gold: number;
  sp: number;
  int_stat: number;
  str_stat: number;
  agi_stat: number;
  wis_stat: number;
  luk_stat: number;
  class: string;
  born_at: string;
  last_save: string;
};

/* ─── Helpers ──────────────────────────────────────────── */

function hoursSince(iso: string, now: Date): number {
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return 0;
  return (now.getTime() - t) / 3_600_000;
}


/* ─── App ──────────────────────────────────────────────── */

function App() {
  const [save, setSave] = useState<CharacterSave | null>(null);
  const [now, setNow] = useState(new Date());
  // "main" IS the always-on memo desk (full-screen transparent canvas with
  // the cassette shell + floating memos). Sub-views are opaque panels.
  const [view, setView] = useState<
    "main" | "dungeons" | "log" | "stats" | "inventory" | "retro"
  >("main");
  // Pin state is dead: "main" is a full-screen transparent desk (the shell
  // drags a div, never the window), so there's nothing to pin. Kept as a const
  // false only because the unreachable sub-views (P1-4) still take the prop.
  const pinned = false;
  // MVP-12 pivot — when Retrospective is entered from a DungeonCard, we
  // need to know which dungeon to summarize.
  const [retroDungeonId, setRetroDungeonId] = useState<string | null>(null);
  // v5 M6 — 첫 실행 온보딩. null이면 아직 모르거나 이미 끝난 것.
  const [onboarding, setOnboarding] = useState<OnboardingStatus | null>(null);
  const [showMigration, setShowMigration] = useState(false);

  useEffect(() => {
    invoke<OnboardingStatus>("onboarding_status")
      .then((o) => { if (o.needed) setOnboarding(o); })
      .catch(() => {});
  }, []);

  useEffect(() => {
    invoke<CharacterSave | null>("get_character_save")
      .then((s) => s && setSave(s))
      .catch(console.error);
    invoke<boolean>("migration_dialog_needed")
      .then((b) => setShowMigration(b))
      .catch(console.error);

    const unlistenSavePromise = listen<CharacterSave>("character-save", (e) => {
      setSave(e.payload);
    });

    // MVP-12 pivot — drain retro completion events into the global queue.
    // Lives at App level so retros keep running even if the user navigates
    // away from the Retrospective view mid-call.
    const unlistenRetroDone = listen<{ dungeon_id: string }>(
      "retro-generated",
      (e) => retroQueue.finish(e.payload.dungeon_id),
    );
    const unlistenRetroFail = listen<{ dungeon_id: string }>(
      "retro-failed",
      (e) => retroQueue.finish(e.payload.dungeon_id),
    );

    // Keep `now` fresh so idle-based hunger updates without user input.
    const poll = setInterval(() => setNow(new Date()), 1000);

    return () => {
      unlistenSavePromise.then((fn) => fn());
      unlistenRetroDone.then((fn) => fn());
      unlistenRetroFail.then((fn) => fn());
      clearInterval(poll);
    };
  }, []);

  // Window sizing per view. "main" IS the memo desk = the whole monitor
  // (transparent + click-through, so the desktop shows through the gaps and
  // notes can be placed anywhere on screen); other sub-views = opaque panels.
  useEffect(() => {
    const w = getCurrentWindow();

    if (view === "main") {
      // The desk must fill WHATEVER monitor it's on, and KEEP filling it when
      // the display geometry changes — resolution switch, DPI/scale change,
      // plugging/unplugging a monitor. Fitting only once (on mount) left the
      // window at the old monitor's size after such a change, so Toki "broke"
      // on a different-resolution screen. So: fit now, on scale change, on
      // webview resize, and re-check on a light interval (self-heals any
      // divergence without fighting the user — the shell drags a div, never
      // the window, so the window's target is always monitor origin+size).
      let disposed = false;
      const fit = async () => {
        const mon = await currentMonitor().catch(() => null);
        if (!mon) {
          const dpr = window.devicePixelRatio || 1;
          await w.setPosition(new PhysicalPosition(0, 0)).catch(() => {});
          await w.setSize(new PhysicalSize(
            Math.round(window.screen.width * dpr),
            Math.round(window.screen.height * dpr),
          )).catch(() => {});
          return;
        }
        const [sz, pos] = await Promise.all([
          w.innerSize().catch(() => null),
          w.outerPosition().catch(() => null),
        ]);
        const posOff = !pos || Math.abs(pos.x - mon.position.x) > 2 || Math.abs(pos.y - mon.position.y) > 2;
        const sizeOff = !sz || Math.abs(sz.width - mon.size.width) > 2 || Math.abs(sz.height - mon.size.height) > 2;
        if (posOff) await w.setPosition(new PhysicalPosition(mon.position.x, mon.position.y)).catch(() => {});
        if (sizeOff) await w.setSize(new PhysicalSize(mon.size.width, mon.size.height)).catch(() => {});
      };
      fit();
      const unlistenScale = w.onScaleChanged(() => { if (!disposed) fit(); });
      const onResize = () => { if (!disposed) fit(); };
      window.addEventListener("resize", onResize);
      const id = window.setInterval(() => { if (!disposed) fit(); }, 2500);
      return () => {
        disposed = true;
        window.clearInterval(id);
        window.removeEventListener("resize", onResize);
        unlistenScale.then((fn) => fn()).catch(() => {});
      };
    }

    // Opaque sub-view: reset click-through to fully interactive (the desk's
    // last multi-rect report would otherwise leave gaps click-through), and
    // size+center to a panel. The window's top-left is wherever "main"
    // (fullscreen desk) last left it — usually the monitor origin — so
    // without an explicit re-center these panels would open pinned to the
    // top-left corner instead of the middle of the screen.
    (async () => {
      // 불투명 패널은 **창 전체**가 조작 대상이다. 빈 배열은 "불투명 영역이
      // 하나도 없다" = 통째로 클릭스루라는 뜻이 됐으므로(M7) 여기서 쓰면 안 된다.
      invoke("set_click_rects", {
        rects: [[0, 0, window.innerWidth, window.innerHeight]],
      }).catch(() => {});
      const panelW = 380, panelH = 560;
      await w.setSize(new LogicalSize(panelW, panelH)).catch(() => {});
      const mon = await currentMonitor().catch(() => null);
      if (mon) {
        const dpr = mon.scaleFactor || 1;
        await w
          .setPosition(new PhysicalPosition(
            Math.round(mon.position.x + (mon.size.width - panelW * dpr) / 2),
            Math.round(mon.position.y + (mon.size.height - panelH * dpr) / 2),
          ))
          .catch(() => {});
      }
    })();
  }, [view]);

  // Idle → hunger. Only genuine activity states drive the shell; the old
  // RPG feedback loop (mob combat, eat pulse, evolve banner) was removed in
  // V4-8 since the cassette shell doesn't render any of it.
  const lastInteractionIso = save?.last_save ?? new Date().toISOString();
  const idleHours = save ? hoursSince(lastInteractionIso, now) : 0;
  const lv = save?.lv ?? 1;

  /* ─── View routing ─── */

  if (showMigration) {
    return <MigrationDialog onResolved={() => setShowMigration(false)} />;
  }
  // Single navigation handler — used by every sub-view's header so a
  // click on a different tab jumps straight there instead of bouncing
  // through main first. (Previously each sub-view only knew `onClose`,
  // which routed everything back to main.)
  const navigate = (k: "stat" | "log" | "map" | "inv" | "cfg") => {
    if (k === "stat") setView("stats");
    else if (k === "log") setView("log");
    else if (k === "inv") setView("inventory");
    // Settings now lives inside the shell (메뉴 → 설정), not a separate view.
    else if (k === "cfg") setView("main");
    else if (k === "map") setView("dungeons");
  };

  if (view === "stats") {
    return <StatsView onClose={() => setView("main")} onNav={navigate} pinned={pinned} />;
  }
  if (view === "dungeons") {
    return (
      <DungeonsView
        onClose={() => setView("main")}
        onNav={navigate}
        pinned={pinned}
        onRetro={(id) => {
          setRetroDungeonId(id);
          setView("retro");
        }}
      />
    );
  }
  if (view === "log") {
    return <DungeonLogView onClose={() => setView("main")} onNav={navigate} pinned={pinned} />;
  }
  if (view === "inventory") {
    return <InventoryView onClose={() => setView("main")} onNav={navigate} pinned={pinned} />;
  }
  if (view === "retro" && retroDungeonId) {
    return (
      <RetrospectiveView
        dungeonId={retroDungeonId}
        onClose={() => setView("dungeons")}
        onNav={navigate}
        pinned={pinned}
      />
    );
  }
  /* ─── Main view — the always-on memo desk ─── */
  // The cassette shell floats on a full-screen transparent canvas with the
  // memos always present around it (V4-7). Coaching and settings are modes
  // inside the shell itself now (V4-9 design-sync), reached via 메뉴.
  return (
    <>
      <MemoDesk
        lv={lv}
        hungry={idleHours >= 3}
        hasCoaching={true /* V4-6: cross-session coaching surface is wired */}
      />
      {/* v5 M6 — 첫 실행이면 데스크 위에 System 얼러트 시퀀스가 덮인다.
          셸은 뒤에서 이미 떠 있고, 마지막 "만나기"로 얼러트가 사라지는
          전이가 "앱이 켜졌다"는 신호다(design-brief §12). */}
      {onboarding?.needed && (
        <OnboardingAlerts status={onboarding} onDone={() => setOnboarding(null)} />
      )}
    </>
  );
}

export default App;
