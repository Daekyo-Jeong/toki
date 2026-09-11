mod agent;
mod coach;
mod codex_hook_installer;
mod db;
mod desk;
mod deep_coach;
mod hook_installer;
mod hook_server;
mod jsonl;
mod migrate;
mod oauth_usage;
mod pets;
mod planner;
mod platform;
mod retro;
mod thrash_watch;
mod tray_sprite;
mod skill_catalog;
mod statusline_installer;
mod tick;
mod update;
mod usage_tracker;
mod watcher;

use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, State, WebviewWindow,
};

/// 창을 진짜 투명하게. **새 데스크 창마다 다시 걸어야 한다** — 창 하나짜리
/// 시절엔 setup에서 한 번이면 됐지만 M7부터 모니터마다 창이 생긴다.
/// 세 겹: (1) tauri 배경색 clear (2) NSWindow isOpaque=NO + clearColor
/// (3) WKWebView drawsBackground=NO (KVC) — 진짜 범인은 이거였다.
pub(crate) fn make_window_transparent(win: &WebviewWindow) {
    let _ = win.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
    #[cfg(target_os = "macos")]
    unsafe {
        use cocoa::appkit::{NSColor, NSWindow};
        use cocoa::base::{id, nil, NO};
        if let Ok(ptr) = win.ns_window() {
            let ns_window = ptr as id;
            ns_window.setOpaque_(NO);
            let clear: id = NSColor::clearColor(nil);
            ns_window.setBackgroundColor_(clear);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = win.with_webview(|webview| unsafe {
            use cocoa::base::{id, nil, NO};
            use cocoa::foundation::NSString;
            use objc::{class, msg_send, sel, sel_impl};
            let wk = webview.inner() as id;
            let key = NSString::alloc(nil).init_str("drawsBackground");
            let no_num: id = msg_send![class!(NSNumber), numberWithBool: NO];
            let _: () = msg_send![wk, setValue: no_num forKey: key];
        });
    }

    // 닫기는 종료가 아니라 숨김이다(트레이 앱). 데스크 창 전부에 건다.
    let win_clone = win.clone();
    win.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = win_clone.hide();
        }
    });
}

/// Show the main window full-screen on its current monitor. V4-7: the app
/// is an always-on full-screen transparent memo desk (shell + notes float on
/// it, gaps click through to the desktop), so the old tray-anchor / pinned
/// popover positioning is obsolete — every show fills the current monitor.
/// 트레이 글리프. macOS는 검정 템플릿(메뉴바가 밝기에 맞춰 뒤집는다), Windows는
/// 템플릿 개념이 없어 **흰 글리프**를 그대로 그린다(작업표시줄 기본이 어둡다 —
/// 밝은 작업표시줄에서의 가독성은 M9 실기 항목). tick.rs의 갱신도 이걸 쓴다.
pub(crate) fn tray_icon() -> tauri::image::Image<'static> {
    #[cfg(target_os = "macos")]
    {
        tauri::include_image!("icons/tray.png")
    }
    #[cfg(not(target_os = "macos"))]
    {
        tauri::include_image!("icons/tray-win.png")
    }
}

fn show_fullscreen_desk(win: &WebviewWindow) {
    if let Ok(Some(mon)) = win.current_monitor() {
        let _ = win.set_position(*mon.position());
        let _ = win.set_size(*mon.size());
    }
    // Re-assert the persisted stacking choice on every show — covers any
    // level reset across hide/show cycles, and makes the settings toggle
    // effective immediately after the next tray-show at worst.
    if let Some(db) = win.app_handle().try_state::<Arc<db::Db>>() {
        if let Ok(s) = db.load_settings() {
            eprintln!("[window] show desk, always_on_top={}", s.always_on_top);
            let _ = win.set_always_on_top(s.always_on_top);
        }
    }
    let _ = win.show();
    let _ = win.set_focus();
}

/// Tray-click handler — toggle the full-screen desk's visibility.
fn toggle_desk(app: &tauri::AppHandle) {
    // M7: 데스크는 모니터마다 하나다. 토글은 **전부 같이** 움직인다 —
    // 한 화면만 사라지면 "일부만 없어졌다"로 읽힌다.
    let any_visible = {
        let mut v = false;
        desk::for_each_desk(app, |w| v |= w.is_visible().unwrap_or(false));
        v
    };
    if any_visible {
        desk::for_each_desk(app, |w| {
            let _ = w.hide();
        });
    } else {
        // 보이기 전에 모니터 구성을 다시 맞춘다 — 숨어 있는 동안 바뀌었을 수 있다.
        desk::sync_windows(app);
        desk::for_each_desk(app, show_fullscreen_desk);
    }
}

#[tauri::command]
fn get_character_save(
    db: State<'_, Arc<db::Db>>,
) -> Result<Option<toki_core::game::CharacterSave>, String> {
    db.load_save().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_totals(db: State<'_, Arc<db::Db>>) -> Result<db::Totals, String> {
    db.totals().map_err(|e| e.to_string())
}

/// V4-14: 캐릭터별 코칭 페르소나 목록 — 외형 화면에서 "이 펫을 고르면 코칭이
/// 어떤 말투가 되는가"를 보여준다. retro::PERSONAS가 단일 소스(프롬프트 주입과
/// 공유)라 여기서 그대로 직렬화만 한다.
#[derive(serde::Serialize)]
struct PersonaRow {
    id: &'static str,
    name: &'static str,
    desc: &'static str,
    example: &'static str,
}

/// 실력 추세 — 레벨(누적 토큰=소비)과 별개로 "얼마나 늘었나"를 보는 지표.
/// 백그라운드에서 30분마다 갱신되며 여기서는 캐시만 읽는다(계산 11초).
/// 아직 안 돌았으면 None → UI가 "계산 중"을 띄운다.
#[tauri::command]
fn get_skill_trend(cache: State<'_, retro::SharedSkillTrend>) -> Option<retro::SkillTrend> {
    cache.lock().ok().and_then(|g| g.clone())
}

/// V4-19: Tokisoft Studio가 export한 유저 펫 목록 (~/.toki/pets/*.json).
#[tauri::command]
fn user_pets() -> Vec<pets::UserPet> {
    pets::load_user_pets()
}

#[tauri::command]
fn coach_personas() -> Vec<PersonaRow> {
    retro::PERSONAS
        .iter()
        .map(|p| PersonaRow { id: p.id, name: p.name, desc: p.desc, example: p.example })
        .collect()
}

/// One reconciled bundle for the "정보" screen. Level math (xp → progress,
/// tokens-to-next) is done here so the divisor lives in exactly one place
/// (Rust); the UI never re-derives it. `xp` is the level XP =
/// `cache_read + cache_creation + output` since baseline (baseline is 0
/// unless track_since_install) — spec §6.1. Cumulative token totals ride
/// along so the screen makes one round trip, not three.
#[derive(serde::Serialize)]
struct LevelInfo {
    lv: i32,
    at_cap: bool,
    xp: i64,       // level XP (since baseline)
    into_level: i64, // xp above the current level's floor
    span: i64,       // xp between this level's floor and the next
    to_next: i64,    // XP tokens remaining to next level
    progress: f64,   // 0.0 ~ 1.0
    // cumulative all-time token usage (raw), for "누적 사용"
    total_input: i64,
    total_output: i64,
    total_cache_creation: i64,
    total_cache_read: i64,
}

#[tauri::command]
fn get_level_info(db: State<'_, Arc<db::Db>>) -> Result<LevelInfo, String> {
    use toki_core::game;
    let save = db.load_save().map_err(|e| e.to_string())?;
    let t = db.totals().map_err(|e| e.to_string())?;
    let (lv, xp) = save.map(|s| (s.lv, s.xp)).unwrap_or((1, 0));
    let at_cap = lv >= game::LV_CAP;
    let floor = game::xp_for_level(lv);
    let next = game::xp_for_level(lv + 1);
    let span = (next - floor).max(1);
    let into_level = (xp - floor).max(0);
    let to_next = if at_cap { 0 } else { (next - xp).max(0) };
    Ok(LevelInfo {
        lv,
        at_cap,
        xp,
        into_level,
        span,
        to_next,
        progress: game::level_progress(xp, lv),
        total_input: t.input,
        total_output: t.output,
        total_cache_creation: t.cache_creation,
        total_cache_read: t.cache_read,
    })
}

#[tauri::command]
fn get_today_cache_read(db: State<'_, Arc<db::Db>>) -> Result<i64, String> {
    db.today_cache_read().map_err(|e| e.to_string())
}

/// M2: 정보 화면 "에이전트" 페이지 — 에이전트별 누적 소비(XP 화폐, 표시용
/// 분해 — XP는 한 펫 합산) + Codex 파싱 통계(spec §10.2 포맷 변경 조기 경보).
#[derive(serde::Serialize)]
struct AgentUsage {
    agents: Vec<db::TokenBucket>,
    codex_parse_ok: u64,
    codex_parse_failed: u64,
}

#[tauri::command]
fn get_agent_usage(db: State<'_, Arc<db::Db>>) -> Result<AgentUsage, String> {
    let (ok, failed) = agent::codex_parse_stats();
    Ok(AgentUsage {
        agents: db.xp_by_agent().map_err(|e| e.to_string())?,
        codex_parse_ok: ok,
        codex_parse_failed: failed,
    })
}

#[tauri::command]
fn get_usage_summary(db: State<'_, Arc<db::Db>>) -> Result<db::UsageSummary, String> {
    db.usage_summary().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_diary(db: State<'_, Arc<db::Db>>, limit: Option<i64>) -> Result<Vec<db::DiaryEntry>, String> {
    db.recent_diary(limit.unwrap_or(50)).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_settings(db: State<'_, Arc<db::Db>>) -> Result<db::AppSettings, String> {
    db.load_settings().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_settings(
    db: State<'_, Arc<db::Db>>,
    app: tauri::AppHandle,
    settings: db::AppSettings,
) -> Result<(), String> {
    // If user just turned ON track_since_install and baseline is 0, snapshot now.
    let mut s = settings;
    if s.track_since_install && s.install_baseline_cache_read == 0 {
        if let Ok(t) = db.totals() {
            s.install_baseline_cache_read = t.xp();
        }
    }
    db.save_settings(&s).map_err(|e| e.to_string())?;

    // Reflect autostart change on the OS side immediately
    use tauri_plugin_autostart::ManagerExt;
    let mgr = app.autolaunch();
    let want = s.autostart_enabled;
    let is_on = mgr.is_enabled().unwrap_or(false);
    if want && !is_on {
        let _ = mgr.enable();
    } else if !want && is_on {
        let _ = mgr.disable();
    }

    // Reflect always-on-top immediately (window starts on-top per
    // tauri.conf.json; this setting overrides it live + on next boot).
    desk::for_each_desk(&app, |w| {
        let _ = w.set_always_on_top(s.always_on_top);
    });
    Ok(())
}

#[tauri::command]
fn reset_character(db: State<'_, Arc<db::Db>>) -> Result<(), String> {
    db.reset_character().map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct DataSourceStatus {
    exists: bool,
    jsonl_count: usize,
    events_count: i64,
    path: String,
}

/* ─── Hook commands ────────────────────────────────────── */

#[derive(serde::Serialize)]
struct HookStatus {
    installed: bool,
    port: Option<u16>,
    received_count: i64,
    /// M4: Claude statusLine 피드(배터리 1순위 소스) 설치 여부.
    statusline_installed: bool,
    /// M4: 이 머신에 `~/.codex`가 있나. 없으면 UI는 "미감지"로 **보여준다** —
    /// 조건부 숨김 금지(있으면 항상 보인다).
    codex_available: bool,
    codex_installed: bool,
    /// M9: 훅·statusLine이 도는 POSIX 셸이 있나. Windows는 Git for Windows가
    /// 있어야 true — 없으면 Claude Code가 PowerShell로 돌려 우리 한 줄이 죽는다.
    shell_ok: bool,
}

#[tauri::command]
fn get_usage_snapshot(
    snap: State<'_, tick::SharedSnapshot>,
) -> Result<Option<usage_tracker::Snapshot>, String> {
    Ok(snap.lock().map_err(|e| e.to_string())?.clone())
}

#[tauri::command]
fn get_dungeons(db: State<'_, Arc<db::Db>>, limit: Option<i64>) -> Result<Vec<db::DungeonRow>, String> {
    db.list_dungeons(limit.unwrap_or(50)).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_skills(db: State<'_, Arc<db::Db>>) -> Result<Vec<db::SkillRow>, String> {
    db.list_skills().map_err(|e| e.to_string())
}

#[tauri::command]
fn backfill_skills(db: State<'_, Arc<db::Db>>) -> Result<(i64, i64), String> {
    db.backfill_skills(is_skill_tool).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_my_skills() -> Vec<skill_catalog::SkillManifest> {
    skill_catalog::scan_my_skills()
}

#[tauri::command]
fn get_library_skills() -> Vec<skill_catalog::SkillManifest> {
    skill_catalog::scan_library_skills()
}

/// Same predicate as tick::drain_hooks. Centralized here so the Tauri
/// command + drainer stay in sync.
pub fn is_skill_tool(tool_name: &str) -> bool {
    !matches!(
        tool_name,
        "Bash" | "Edit" | "Read" | "Write" | "Glob" | "Grep" | "LS" | "NotebookEdit"
    )
}

#[tauri::command]
fn get_active_dungeon(db: State<'_, Arc<db::Db>>) -> Result<Option<db::DungeonRow>, String> {
    db.active_dungeon().map_err(|e| e.to_string())
}

#[tauri::command]
fn hooks_status(db: State<'_, Arc<db::Db>>) -> Result<HookStatus, String> {
    let installed = hook_installer::is_installed().map_err(|e| e.to_string())?;
    let port = read_port_file();
    let received_count = db.count_hook_raw().map_err(|e| e.to_string())?;
    let statusline_installed = statusline_installer::is_installed().unwrap_or(false);
    let codex_available = codex_hook_installer::is_available();
    let codex_installed = codex_hook_installer::is_installed().unwrap_or(false);
    let shell_ok = platform::posix_shell_available();
    Ok(HookStatus { installed, port, received_count, statusline_installed, codex_available, codex_installed, shell_ok })
}

/// M4: Claude statusLine 인스톨러 — 배터리 1순위 소스를 사용자가 손 안 대고 켠다.
#[tauri::command]
fn statusline_install() -> Result<(), String> {
    statusline_installer::install().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn statusline_uninstall() -> Result<(), String> {
    statusline_installer::uninstall().map_err(|e| e.to_string())
}

/// M4: Codex hooks.json 인스톨러. 신뢰 승인은 codex 안에서 사용자가 한다.
#[tauri::command]
fn codex_hooks_install() -> Result<(), String> {
    let Some(port) = read_port_file() else {
        return Err("hook server not running yet".into());
    };
    codex_hook_installer::install(port).map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn codex_hooks_uninstall() -> Result<(), String> {
    codex_hook_installer::uninstall().map_err(|e| e.to_string())
}

#[tauri::command]
fn hooks_install() -> Result<(), String> {
    let Some(port) = read_port_file() else {
        return Err("hook server not running yet".into());
    };
    hook_installer::install(port).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn hooks_uninstall() -> Result<(), String> {
    hook_installer::uninstall().map_err(|e| e.to_string())?;
    Ok(())
}

fn read_port_file() -> Option<u16> {
    dirs::home_dir()
        .and_then(|h| std::fs::read_to_string(h.join(".toki").join("port")).ok())
        .and_then(|s| s.trim().parse().ok())
}

#[tauri::command]
fn migration_dialog_needed(db: State<'_, Arc<db::Db>>) -> Result<bool, String> {
    // Show dialog once if v1 legacy backup exists AND user hasn't answered yet.
    let shown = db.migration_dialog_shown().map_err(|e| e.to_string())?;
    if shown {
        return Ok(false);
    }
    let legacy_exists = dirs::home_dir()
        .map(|h| h.join(".toki").join("legacy-backup").join("tamagotchi.sqlite").exists())
        .unwrap_or(false);
    Ok(legacy_exists)
}

#[tauri::command]
fn resolve_migration_dialog(
    db: State<'_, Arc<db::Db>>,
    app: tauri::AppHandle,
    choice: String, // "fresh" | "keep"
) -> Result<(), String> {
    let mut settings = db.load_settings().map_err(|e| e.to_string())?;
    if choice == "fresh" {
        // Snapshot baseline = 현재 XP 총량이라 XP가 0부터 시작한다
        let totals = db.totals().map_err(|e| e.to_string())?;
        settings.track_since_install = true;
        settings.install_baseline_cache_read = totals.xp();
        // Reset save back to Lv 1
        db.reset_character().map_err(|e| e.to_string())?;
    } else {
        // keep — don't change baseline, just acknowledge
        settings.track_since_install = false;
        settings.install_baseline_cache_read = 0;
    }
    db.save_settings(&settings).map_err(|e| e.to_string())?;
    db.mark_migration_dialog_shown().map_err(|e| e.to_string())?;
    let _ = app.emit("migration-resolved", &choice);
    Ok(())
}

/* ─── Onboarding (v5 M6) ───────────────────────────────── */

/// 감지된 에이전트 한 종. `detected=false`여도 **목록에 남긴다** — 조건부 숨김은
/// 기능의 존재 자체를 감춘다. 못 찾았으면 설치 링크를 보여줄 자리다.
#[derive(serde::Serialize)]
struct DetectedAgent {
    id: String,
    label: String,
    detected: bool,
    /// 로그 루트 경로(`~/…`). 사용자가 "어디를 읽는지" 눈으로 확인할 자리.
    path: String,
    /// 스캔 대상 파일 수 — 백필로 얼마나 나올지의 예고.
    log_files: usize,
    /// CLI 설치 안내용.
    install_url: String,
}

/// v5 M6 — 첫 실행 온보딩에 필요한 것 전부. LLM 호출 없음.
#[derive(serde::Serialize)]
struct OnboardingStatus {
    /// 시퀀스를 띄워야 하나. 이미 끝냈거나 이벤트가 쌓인 설치면 false.
    needed: bool,
    agents: Vec<DetectedAgent>,
    /// 하나라도 감지됐나 — 0개면 안내 분기.
    any_agent: bool,
    events: i64,
}

fn count_log_files(root: &std::path::Path, src: &dyn agent::AgentSource) -> usize {
    let mut n = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if src.is_log_file(&p) {
                n += 1;
            }
        }
    }
    n
}

#[tauri::command]
fn onboarding_status(db: State<'_, Arc<db::Db>>) -> OnboardingStatus {
    let home = dirs::home_dir().unwrap_or_default();
    let agents: Vec<DetectedAgent> = agent::sources()
        .iter()
        .map(|s| {
            let root = s.watch_root();
            let detected = root.as_ref().map(|p| p.exists()).unwrap_or(false);
            let log_files = match (&root, detected) {
                (Some(p), true) => count_log_files(p, s.as_ref()),
                _ => 0,
            };
            let (label, install_url) = match s.id() {
                agent::AgentId::Claude => ("Claude Code", "https://claude.com/claude-code"),
                agent::AgentId::Codex => ("Codex CLI", "https://developers.openai.com/codex/cli"),
            };
            DetectedAgent {
                id: s.id().as_str().to_string(),
                label: label.to_string(),
                detected,
                path: root
                    .map(|p| match p.strip_prefix(&home) {
                        Ok(rest) => format!("~/{}", rest.display()),
                        Err(_) => p.display().to_string(),
                    })
                    .unwrap_or_default(),
                log_files,
                install_url: install_url.to_string(),
            }
        })
        .collect();

    let events = db.totals().map(|t| t.events).unwrap_or(0);
    let done = db.onboarded().unwrap_or(true);
    OnboardingStatus {
        needed: onboarding_needed(done, events),
        any_agent: agents.iter().any(|a| a.detected),
        agents,
        events,
    }
}

/// 온보딩을 띄울까 — **플래그 하나만 본다.**
///
/// 처음엔 `events == 0`도 같이 봤는데 그게 시나리오 A를 깨뜨렸다: Claude를 몇
/// 달 써온 사람이 Toki를 막 깔면, 앱이 뜨자마자 백필이 돌아 화면이 그려지기도
/// 전에 이벤트가 수만 건 쌓인다 → "신규가 아니다"로 판정돼 온보딩이 통째로
/// 안 뜬다. 정작 그 사람이 부화 화면에서 "Lv.68까지 자랐어요"를 봐야 할 사람이다.
///
/// 기존 사용자를 거르는 일은 **DB 마이그레이션**이 이미 한다(열 때 이벤트가
/// 있으면 `onboarded=1`). 마이그레이션은 watcher가 뜨기 전에 돌아서 백필과
/// 경합하지 않는다. 그래서 여기선 플래그만 믿으면 된다.
fn onboarding_needed(onboarded: bool, _events: i64) -> bool {
    !onboarded
}

/// 시퀀스를 끝냈다(또는 사용자가 건너뛰었다). 다시는 안 뜬다.
#[tauri::command]
fn onboarding_finish(db: State<'_, Arc<db::Db>>) -> Result<(), String> {
    db.mark_onboarded().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_data_source_status(db: State<'_, Arc<db::Db>>) -> DataSourceStatus {
    let path = dirs::home_dir()
        .map(|h| h.join(".claude").join("projects"))
        .unwrap_or_default();
    let exists = path.exists();
    let mut jsonl_count = 0;
    if exists {
        let mut stack = vec![path.clone()];
        while let Some(dir) = stack.pop() {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for entry in rd.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                        jsonl_count += 1;
                    }
                }
            }
        }
    }
    let events_count = db.totals().map(|t| t.events).unwrap_or(0);
    DataSourceStatus {
        exists,
        jsonl_count,
        events_count,
        path: path.to_string_lossy().to_string(),
    }
}

/// MVP-12 (pivot) — Retrospective generation for a finished dungeon.
/// Pulls hook events for the session, calls `claude -p` (haiku/sonnet),
/// saves the markdown body onto `dungeons.retrospective`, returns it.
/// User-action only — burns the user's 5h quota.
///
/// Runs on a worker thread (`spawn_blocking`) so the subprocess wait
/// (5-30s) doesn't freeze the webview's IPC thread. Also emits a
/// `retro-generated` event on success / `retro-failed` on error so any
/// view that's mounted can react without having to await this promise —
/// the user can navigate away mid-call and come back later.
#[tauri::command]
async fn retrospective_generate(
    app: tauri::AppHandle,
    db: State<'_, Arc<db::Db>>,
    dungeon_id: String,
    model: Option<String>,
) -> Result<String, String> {
    let db_clone = db.inner().clone();
    let id_clone = dungeon_id.clone();
    let model_owned = model.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        retro::generate(&db_clone, &id_clone, model_owned.as_deref())
    })
    .await
    .map_err(|e| format!("join error: {}", e))?;

    match result {
        Ok(body) => {
            let _ = app.emit(
                "retro-generated",
                serde_json::json!({
                    "dungeon_id": dungeon_id,
                    "body": body,
                }),
            );
            Ok(body)
        }
        Err(e) => {
            let msg = e.to_string();
            let _ = app.emit(
                "retro-failed",
                serde_json::json!({
                    "dungeon_id": dungeon_id,
                    "error": msg,
                }),
            );
            Err(msg)
        }
    }
}

/// V4-6 §3.2 — cross-session habit coaching. Distills the last `limit`
/// sessions, aggregates cross-session signals, and coaches on the habits
/// via the configured backend (local Ollama by default → zero quota).
/// Runs on a worker thread; distilling N transcripts + one LLM call is slow.
#[tauri::command]
async fn coaching_cross_generate(
    db: State<'_, Arc<db::Db>>,
    limit: Option<usize>,
) -> Result<retro::CrossCoaching, String> {
    let db_clone = db.inner().clone();
    let n = limit.unwrap_or(20).clamp(2, 100);
    tauri::async_runtime::spawn_blocking(move || retro::generate_cross(&db_clone, n))
        .await
        .map_err(|e| format!("join error: {}", e))?
        .map_err(|e| e.to_string())
}

/// A/V4-14 — single-session retro on the MOST RECENT session ("이번 세션").
/// Richer than cross-session (has the verbatim prompt thread), so it has more
/// to say for a disciplined user. Local Ollama by default (zero quota). Worker
/// thread — one LLM call is slow.
#[tauri::command]
async fn session_retro_latest(db: State<'_, Arc<db::Db>>) -> Result<String, String> {
    let db_clone = db.inner().clone();
    tauri::async_runtime::spawn_blocking(move || retro::generate_latest_session(&db_clone))
        .await
        .map_err(|e| format!("join error: {}", e))?
        .map_err(|e| e.to_string())
}

/// A/V4-14 회고 picker — recent projects (grouped, newest-first) to choose from.
#[tauri::command]
fn retro_project_list() -> Vec<retro::ProjectInfo> {
    retro::recent_projects(7, 12)
}

/// A/V4-14 — the last cached retro for a project, if any (instant, no LLM).
#[tauri::command]
fn retro_project_cached(dir: String) -> Option<retro::RetroCache> {
    retro::load_retro_cache(&dir)
}

/// A/V4-14 — retro on a chosen project, merging its recent sessions.
#[tauri::command]
async fn retro_project_generate(db: State<'_, Arc<db::Db>>, dir: String) -> Result<String, String> {
    let db_clone = db.inner().clone();
    tauri::async_runtime::spawn_blocking(move || retro::generate_project_retro(&db_clone, &dir))
        .await
        .map_err(|e| format!("join error: {}", e))?
        .map_err(|e| e.to_string())
}

/// P3-15: is the coaching model already warm? The frontend polls this while
/// generating so a cold first run reads "모델 깨우는 중" (model waking) instead
/// of a silent 40s "분석 중". Returns true when the Ollama model is resident, or
/// when the backend is claude -p (no warmup concept) so the caller shows the
/// normal analyzing label. Server down / refused → false (still cold).
#[tauri::command]
fn coaching_ollama_loaded(db: State<'_, Arc<db::Db>>) -> bool {
    match coach::Backend::resolve(db.inner()) {
        coach::Backend::Ollama { model } => coach::ollama_model_loaded(&model),
        _ => true,
    }
}

/// V5 — 딥 코칭. 프롬프트 히스토리 원문 + 자산 인벤토리 + 크로스세션 집계를
/// claude -p (sonnet)가 추론한다. **사용자 5h 쿼터를 소비** — UI의 명시적
/// "딥 분석" 버튼에서만 호출할 것 (자동 실행 금지, §3.1 쿼터 아이러니).
#[tauri::command]
async fn coaching_deep_generate(db: State<'_, Arc<db::Db>>) -> Result<deep_coach::DeepCoaching, String> {
    let backend = coach::Backend::resolve(db.inner());
    tauri::async_runtime::spawn_blocking(move || deep_coach::generate_deep(&backend))
        .await
        .map_err(|e| format!("join error: {}", e))?
        .map_err(|e| e.to_string())
}

/// 딥 코칭 실행 **전** 고지 (spec §9.2) — 어느 백엔드로 무엇이 나가는지.
/// LLM 호출도 쿼터 소비도 없다. 화면이 이 목록을 그대로 보여준다.
#[tauri::command]
fn coaching_deep_preview(db: State<'_, Arc<db::Db>>) -> deep_coach::DeepPreview {
    deep_coach::preview(&coach::Backend::resolve(db.inner()))
}

/// 붙여넣은 기억 (`~/.toki/memory.md`, spec §6.4.1) — 읽기.
#[tauri::command]
fn memory_paste_get() -> String {
    memory_paste_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default()
}

/// 붙여넣은 기억 — 쓰기. 파싱하지 않는다(형식 자유). 빈 내용이면 파일을 지운다
/// = 감지 목록에서 빠진다.
#[tauri::command]
fn memory_paste_set(text: String) -> Result<usize, String> {
    let p = memory_paste_path().ok_or("홈 디렉토리를 찾을 수 없어요")?;
    if text.trim().is_empty() {
        let _ = std::fs::remove_file(&p);
        return Ok(0);
    }
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, &text).map_err(|e| e.to_string())?;
    Ok(text.len())
}

fn memory_paste_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".toki").join("memory.md"))
}

/// 기억 붙여넣기의 "파일 불러오기" — 사용자가 다이얼로그에서 고른 `.md`/`.txt`를
/// 읽어 화면 필드로 돌려준다(저장 전에 보여주려고). plugin-fs를 안 쓰는 이유는
/// 이 한 가지 용도뿐이고, 여기서 확장자·크기를 직접 게이트하는 게 더 좁다.
#[tauri::command]
fn read_text_file_utf8(path: String) -> Result<String, String> {
    const MAX: u64 = 2 * 1024 * 1024;
    let p = std::path::PathBuf::from(&path);
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    if !matches!(ext.as_str(), "md" | "txt" | "markdown" | "text") {
        return Err("`.md` 또는 `.txt` 파일만 읽어요".into());
    }
    let meta = std::fs::metadata(&p).map_err(|e| e.to_string())?;
    if meta.len() > MAX {
        return Err(format!("파일이 너무 커요 ({}KB · 최대 2MB)", meta.len() / 1024));
    }
    std::fs::read_to_string(&p).map_err(|e| e.to_string())
}

/// 코칭 프롬프트 로컬 오버라이드를 지운다 → 코드 기본값 복귀 (spec §6.4).
#[tauri::command]
fn prompts_reset() -> usize {
    retro::reset_prompt_templates()
}

/// 최신 딥 코칭 캐시 — 재진입 즉시 표시 (쿼터 재소비 없음).
#[tauri::command]
fn coaching_deep_latest() -> Option<deep_coach::DeepCoaching> {
    deep_coach::latest_deep()
}

/// The last cached cross-session analysis (newest file in ~/.toki/coaching/),
/// or None if never run. Lets the coaching screen show the previous result
/// instantly with a "N시간 전 분석" label — no LLM spend. File-backed, so it
/// needs no DB and survives restart.
#[tauri::command]
fn coaching_cross_latest() -> Option<retro::CrossCoaching> {
    retro::latest_cross()
}

/// Memo-desk backup (P2-6). Notes + shell position live in the webview's
/// localStorage, which a webview-data reset / re-signing wipes. We mirror the
/// blob to ~/.toki/desk-backup.json so a wiped desk can be recovered. Additive
/// only — localStorage stays the primary store; this is a safety net.
#[tauri::command]
fn desk_backup_save(json: String) -> Result<(), String> {
    let Some(home) = dirs::home_dir() else { return Err("no home dir".into()) };
    let dir = home.join(".toki");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("desk-backup.json"), json).map_err(|e| e.to_string())
}

#[tauri::command]
fn desk_backup_load() -> Option<String> {
    std::fs::read_to_string(dirs::home_dir()?.join(".toki").join("desk-backup.json")).ok()
}

/// Whether a session has a transcript we can coach on. Drives the RETRO
/// button enabled state. Takes a list of session ids, returns the subset
/// that have transcripts (one read_dir scan).
#[tauri::command]
fn retrospective_available(session_ids: Vec<String>) -> Vec<String> {
    session_ids
        .into_iter()
        .filter(|sid| retro::has_transcript(sid))
        .collect()
}

/// Read the cached retrospective for a dungeon, if any.
/// Returns `(body, generated_at)` or `null`.
#[tauri::command]
fn retrospective_get(
    db: State<'_, Arc<db::Db>>,
    dungeon_id: String,
) -> Result<Option<(String, String)>, String> {
    db.load_retrospective(&dungeon_id).map_err(|e| e.to_string())
}

/// P2-7: fire an OS notification for an in-app alert, gated by the user's
/// `notifications_level` ("off" | "impt" | "all"). Important alerts (hunger,
/// coaching-ready, new pet) pass at "impt"; frequent ones (pomodoro phase)
/// only at "all". The frontend calls this alongside its in-app modal — and
/// only when the desk isn't focused, so a visible desk shows just the modal.
#[tauri::command]
fn send_alert_notification(
    app: tauri::AppHandle,
    db: State<'_, Arc<db::Db>>,
    title: String,
    body: String,
    important: bool,
) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    let level = db
        .load_settings()
        .map(|s| s.notifications_level)
        .unwrap_or_else(|_| "impt".into());
    let allow = match level.as_str() {
        "all" => true,
        "impt" => important,
        _ => false, // "off"
    };
    if !allow {
        return Ok(());
    }
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| e.to_string())
}

/// v4 forward click-through. The window is transparent and covers a "desk"
/// area, but only the opaque islands (cassette shell + memos) should catch
/// clicks — the rest passes through to whatever is behind. JS reports the
/// opaque rects (window-content CSS px, top-left origin); a background poll
/// toggles `ignore_cursor_events` by whether the cursor is over one.
/// v5 M7: 창 **라벨별** rects. 데스크 창이 모니터마다 하나씩 뜨므로 하나의
/// 전역 목록으로는 서로를 덮어쓴다(마지막에 보고한 창만 살아남는다).
type ClickRects = Arc<Mutex<std::collections::HashMap<String, Vec<[f64; 4]>>>>;

#[tauri::command]
fn set_click_rects(
    rects_state: State<'_, ClickRects>,
    window: tauri::Window,
    rects: Vec<[f64; 4]>,
) {
    // P3-11: this fires ~3×/s from the desk's rect poll — debug-only so release
    // stderr isn't spammed.
    #[cfg(debug_assertions)]
    eprintln!("[clickthru] set_click_rects {} {:?}", window.label(), rects);
    if let Ok(mut g) = rects_state.lock() {
        g.insert(window.label().to_string(), rects);
    }
}

fn spawn_click_through(app: tauri::AppHandle, rects: ClickRects) {
    std::thread::spawn(move || {
        // 창마다 마지막 상태를 따로 기억한다 — 한 창의 토글이 다른 창의
        // 판정을 덮으면 커서가 없는 화면이 계속 인터랙티브로 남는다.
        let mut last_ignore: std::collections::HashMap<String, bool> = Default::default();
        let mut last_emit: Option<(f64, f64)> = None;
        let mut tick: u32 = 0;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(30));
            tick = tick.wrapping_add(1);
            let labels: Vec<String> = app
                .webview_windows()
                .keys()
                .filter(|l| desk::is_desk_label(l))
                .cloned()
                .collect();
            for label in labels {
                click_through_tick(&app, &rects, &label, &mut last_ignore, &mut last_emit, tick);
            }
        }
    });
}

/// 데스크 창 하나에 대한 한 틱. 창이 여럿이 되며 루프 본문을 함수로 뺐다.
fn click_through_tick(
    app: &tauri::AppHandle,
    rects: &ClickRects,
    label: &str,
    last_ignore: &mut std::collections::HashMap<String, bool>,
    last_emit: &mut Option<(f64, f64)>,
    tick: u32,
) {
    {
        let Some(win) = app.get_webview_window(label) else { return };
        if !win.is_visible().unwrap_or(false) {
            // Reset so the shell is interactive the instant it reappears.
            if last_ignore.get(label) != Some(&false) {
                let _ = win.set_ignore_cursor_events(false);
                last_ignore.insert(label.to_string(), false);
            }
            return;
        }
        let cursor = match win.cursor_position() { Ok(c) => c, Err(_) => return };
        let pos = match win.outer_position() { Ok(p) => p, Err(_) => return };
            let sf = win.scale_factor().unwrap_or(1.0);
            // Mixed-DPR fix: on macOS `cursor_position()` is scaled by the
            // PRIMARY monitor's factor, but `outer_position()` is scaled by
            // the window's OWN monitor factor. Subtracting them raw is only
            // correct when both monitors share a scale (why retina-only worked
            // and a non-retina second monitor didn't). Convert both to global
            // LOGICAL points first — cursor ÷ primary_scale, window ÷ its own
            // scale — so the difference is window-relative CSS px, matching the
            // rects JS now reports in CSS px (logical, no ×dpr).
            #[cfg(target_os = "macos")]
            let primary = win
                .primary_monitor()
                .ok()
                .flatten()
                .map(|m| m.scale_factor())
                .unwrap_or(sf);
            // Windows: `cursor_position()`(GetCursorPos)도 `outer_position()`
            // (GetWindowRect)도 **물리 px**라 같은 창 배율로 나눈다. 위 macOS
            // 보정은 AppKit이 커서만 주 모니터 배율로 주는 데서 온 것이다.
            // (M9 실기 검증 항목 — 혼합 DPI 두 모니터에서 확인)
            #[cfg(not(target_os = "macos"))]
            let primary = sf;
            let cx = cursor.x / primary - pos.x as f64 / sf;
            let cy = cursor.y / primary - pos.y as f64 / sf;
            // Emit the cursor (window-relative CSS/logical px) ~every 90ms so the
            // pet can track it even over transparent/click-through areas, where
            // the webview receives no pointermove. Throttled + move-gated to
            // keep IPC light (~10/s, only when it actually moved).
        // 커서 좌표 emit은 **셸이 있는 창에서만** 의미가 있다(펫 시선 추적).
        // 창마다 쏘면 같은 이벤트가 N배로 날아가 서로를 덮는다.
        if tick % 3 == 0 && label == desk::PRIMARY_LABEL {
            let moved = last_emit
                .map(|(lx, ly)| (cx - lx).abs() > 1.0 || (cy - ly).abs() > 1.0)
                .unwrap_or(true);
            if moved {
                let _ = app.emit("cursor-pos", (cx, cy));
                *last_emit = Some((cx, cy));
            }
        }
            let (inside, unreported) = {
                // Pad each rect by a margin: the two scale-factor divisions above
                // accumulate rounding error, so an exact containment test can
                // judge a cursor visually ON an edge button (e.g. 자세히, the
                // shell's bottom-right soft key) as *outside* → ignore stays on
                // → the click falls through. The pad also flips interactivity
                // slightly BEFORE the cursor reaches a button, absorbing the
                // ~30ms poll race for fast move-and-click.
                const PAD: f64 = 12.0;
            let g = rects.lock().unwrap();
            let mine = g.get(label);
            (
                mine.map(|v| v.iter().any(|r| {
                        cx >= r[0] - PAD
                            && cx <= r[0] + r[2] + PAD
                            && cy >= r[1] - PAD
                        && cy <= r[1] + r[3] + PAD
                })).unwrap_or(false),
                // **"아직 보고 안 함"과 "보고했는데 비었음"은 다르다.**
                // 창이 하나였을 땐 셸이 늘 있어서 둘을 같이 취급해도 안전했다.
                // M7부터는 메모도 셸도 없는 데스크 창이 정상적으로 존재한다 —
                // 그 창을 "조작 가능"으로 두면 투명한 전체화면이 그 모니터의
                // 클릭을 전부 삼킨다(2026-09-03: 델 화면에서 다른 앱이 안 눌렸다).
                mine.is_none(),
            )
        };
        // 프런트가 **한 번도** 보고하지 않은 창만 조작 가능으로 둔다(기동 직후
        // 셸이 먹통이 되는 걸 막는 원래 목적). 빈 배열을 보고한 창은 통째로 클릭스루.
        let ignore = if unreported { false } else { !inside };
        if last_ignore.get(label) != Some(&ignore) {
            let _ = win.set_ignore_cursor_events(ignore);
            #[cfg(debug_assertions)]
            eprintln!(
                "[clickthru] {label} cursor_rel=({:.0},{:.0}) unreported={} inside={} -> ignore={}",
                cx, cy, unreported, inside, ignore
            );
            last_ignore.insert(label.to_string(), ignore);
        }
    }
}

/// 프런트가 진단 한 줄을 남긴다 — 창을 죽이지 않고 상태를 보기 위한 통로.
#[tauri::command]
fn desk_debug_note(window: tauri::Window, line: String) {
    desk::debug_log(&format!("[{}] {}", window.label(), line));
}

/// v3: "Claude 열기" — the everyday entry point. Opens the Claude desktop
/// app if installed, else claude.ai in the browser. Non-blocking.
#[tauri::command]
fn open_claude() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // Try the desktop app first; fall back to the website.
        let app = std::process::Command::new("open")
            .args(["-a", "Claude"])
            .status();
        let ok = matches!(app, Ok(s) if s.success());
        if !ok {
            let _ = std::process::Command::new("open")
                .arg("https://claude.ai")
                .status();
        }
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        // Claude 데스크톱(Squirrel 설치: %LOCALAPPDATA%\AnthropicClaude\claude.exe)
        // → 없으면 기본 브라우저. 경로는 문서가 아니라 관례라 M9 실기 항목.
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let exe = std::path::PathBuf::from(local).join("AnthropicClaude").join("claude.exe");
            if exe.exists() && platform::quiet(&mut std::process::Command::new(&exe)).spawn().is_ok() {
                return Ok(());
            }
        }
        platform::quiet(&mut std::process::Command::new("cmd"))
            .args(["/C", "start", "", "https://claude.ai"])
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err("open_claude: unsupported platform".into())
    }
}

/// Recent raw hook events for the global Log view (timeline). Limit
/// kept small (default 100) so the UI can paint quickly.
#[tauri::command]
fn get_recent_events(
    db: State<'_, Arc<db::Db>>,
    limit: Option<i64>,
) -> Result<Vec<db::RecentHookEvent>, String> {
    db.recent_hook_events(limit.unwrap_or(100)).map_err(|e| e.to_string())
}

/// Planner status — kept around so the UI can show "claude CLI 미발견"
/// even though we no longer expose `planner_call` directly. Retro relies
/// on the same binary discovery.
#[tauri::command]
fn planner_status() -> planner::PlannerStatus {
    planner::status()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            get_character_save,
            get_totals,
            get_level_info,
            coach_personas,
            user_pets,
            get_skill_trend,
            get_today_cache_read,
            get_agent_usage,
            get_diary,
            get_settings,
            set_settings,
            reset_character,
            get_data_source_status,
            onboarding_status,
            onboarding_finish,
            get_usage_summary,
            migration_dialog_needed,
            resolve_migration_dialog,
            hooks_status,
            hooks_install,
            hooks_uninstall,
            statusline_install,
            statusline_uninstall,
            codex_hooks_install,
            codex_hooks_uninstall,
            get_dungeons,
            get_active_dungeon,
            get_usage_snapshot,
            get_skills,
            backfill_skills,
            get_my_skills,
            get_library_skills,
            planner_status,
            retrospective_generate,
            coaching_cross_generate,
            coaching_deep_generate,
            coaching_deep_preview,
            memory_paste_get,
            memory_paste_set,
            read_text_file_utf8,
            prompts_reset,
            coaching_deep_latest,
            session_retro_latest,
            retro_project_list,
            retro_project_cached,
            retro_project_generate,
            coaching_ollama_loaded,
            coaching_cross_latest,
            desk_backup_save,
            desk_backup_load,
            retrospective_get,
            retrospective_available,
            get_recent_events,
            open_claude,
            set_click_rects,
            desk::desk_monitor,
            desk::desk_monitors,
            desk::desk_cursor_target,
            update::update_check,
            update::update_run,
            desk::desk_drag_begin,
            desk::desk_drag_end,
            desk_debug_note,
            desk::desk_state_load,
            desk::desk_state_save,
            send_alert_notification
        ])
        .setup(|app| {
            // macOS: run as a menubar-only utility — no Dock icon, no
            // Cmd-Tab presence. Accessory ≈ LSUIElement=true in Info.plist
            // but applied at runtime so we don't need a custom plist.
            #[cfg(target_os = "macos")]
            {
                let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }

            // P2-7: ensure OS-notification authorization so alerts can surface
            // when the desk is hidden. No-op after the user's first decision.
            {
                use tauri_plugin_notification::NotificationExt;
                if !matches!(app.notification().permission_state(), Ok(tauri_plugin_notification::PermissionState::Granted)) {
                    let _ = app.notification().request_permission();
                }
            }

            let paths = migrate::resolve(app.handle()).expect("resolve toki paths");
            println!("[db] {}", paths.db_path.display());
            let database = Arc::new(db::Db::open(&paths.db_path).expect("db open"));

            let usage_snap: tick::SharedSnapshot = Arc::new(std::sync::Mutex::new(None));

            watcher::spawn(database.clone());
            tick::spawn(app.handle().clone(), database.clone(), usage_snap.clone());
            let skill_cache: retro::SharedSkillTrend = Arc::new(Mutex::new(None));
            app.manage(skill_cache.clone());
            retro::spawn_skill_trend(skill_cache);
            hook_server::spawn(app.handle().clone(), database.clone(), paths.toki_dir.clone());

            // v4 forward click-through: shared opaque-rects + poll thread.
            let click_rects: ClickRects = Arc::new(Mutex::new(Default::default()));
            app.manage(click_rects.clone());
            spawn_click_through(app.handle().clone(), click_rects);

            // One-shot: backfill skills from historical PostToolUse rows so
            // the user sees their tool history immediately rather than only
            // tools called after install. Idempotent — re-runs are no-ops.
            match database.backfill_skills(is_skill_tool) {
                Ok((new_skills, total_uses)) => {
                    println!("[skill] backfill: +{} skills, {} total uses", new_skills, total_uses);
                }
                Err(e) => eprintln!("[skill] backfill error: {}", e),
            }

            app.manage(database);
            app.manage(usage_snap);

            // --- Tray
            let show_i = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            TrayIconBuilder::with_id("main-tray")
                // Dedicated menubar glyph (black-on-transparent) rendered as a
                // macOS template so it inverts with light/dark menubar — the
                // full-color app icon reads poorly at menubar size.
                .icon(tray_icon())
                // macOS 전용(다른 OS에선 no-op) — Windows는 tray_icon()이 흰 글리프를 준다.
                .icon_as_template(true)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(win) = app.get_webview_window("main") {
                            show_fullscreen_desk(&win);
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_desk(tray.app_handle());
                    }
                })
                .build(app)?;

            if let Some(win) = app.get_webview_window("main") {
                // V4-10: re-apply the persisted always-on-top choice (the
                // conf.json default is `true`; a user who turned it off gets
                // a normal-stacking desk back on every boot).
                if let Some(db) = app.try_state::<Arc<db::Db>>() {
                    if let Ok(s) = db.load_settings() {
                        eprintln!("[window] boot, always_on_top={}", s.always_on_top);
                        let _ = win.set_always_on_top(s.always_on_top);
                    }
                }

                make_window_transparent(&win);
            }

            // M7: 모니터마다 데스크 창 하나. Tauri엔 "모니터 구성이 바뀌었다"
            // 이벤트가 없어서 기동 시 1회 + 3초 폴링으로 따라간다(값이 이미
            // 맞으면 아무것도 안 한다).
            {
                let h = app.handle().clone();
                desk::sync_windows(&h);
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    desk::sync_windows(&h);
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod onboarding_tests {
    use super::*;

    #[test]
    fn needed_only_for_a_fresh_install() {
        assert!(onboarding_needed(false, 0), "신규 설치엔 떠야 한다");
        // 끝냈으면 이벤트가 0이어도(에이전트 미설치) 다시 안 뜬다 — 매번 뜨면 고문이다.
        assert!(!onboarding_needed(true, 0));
        assert!(!onboarding_needed(true, 12_345));
        // **시나리오 A 회귀 가드**: Claude를 오래 써온 사람이 Toki를 막 깔면
        // 화면이 뜨기 전에 백필이 이벤트를 채운다. 그걸 "기존 사용자"로 읽으면
        // 정작 "Lv.68까지 자랐어요"를 봐야 할 사람이 온보딩을 못 본다.
        assert!(onboarding_needed(false, 120_000), "백필이 먼저 끝나도 떠야 한다");
    }

    /// 재귀로 세되 **소스가 인정하는 로그 파일만** 센다 — "N개 기록"이 과장되면
    /// 백필 결과와 어긋나 첫 화면부터 신뢰를 잃는다.
    #[test]
    fn counts_only_log_files_recursively() {
        let base = std::env::temp_dir().join(format!("toki-onb-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("a/b")).unwrap();
        std::fs::write(base.join("one.jsonl"), "{}").unwrap();
        std::fs::write(base.join("a/two.jsonl"), "{}").unwrap();
        std::fs::write(base.join("a/b/three.jsonl"), "{}").unwrap();
        std::fs::write(base.join("a/notes.md"), "x").unwrap();
        std::fs::write(base.join("a/b/data.json"), "{}").unwrap();

        let n = count_log_files(&base, &agent::ClaudeSource);
        assert_eq!(n, 3);
        let _ = std::fs::remove_dir_all(&base);
    }
}
