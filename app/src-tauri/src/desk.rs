//! v5 M7 — 다중 모니터 데스크 (spec §7.0).
//!
//! v4의 데스크는 **창 하나**였고 `currentMonitor()`에 자기를 고정했다. 그래서
//! 메모와 셸이 한 모니터에 갇혔다. v5는 **모니터마다 데스크 창을 하나씩** 띄운다.
//!
//! **왜 "전체를 덮는 큰 창 하나"가 아닌가**: 창은 배율을 하나만 가진다. 걸치면
//! 한쪽은 반드시 OS가 리샘플링하고, 그 순간 픽셀 격자가 깨진다(spec §6.6 —
//! 정수배가 아니면 1x에서 획이 통째로 빠진다). 가정이 아니라 실측이다:
//! 대교 회사 맥북프로 = 내장 Retina 2x + DELL 1x 두 대.
//!
//! 이 모듈이 지는 책임은 둘이다.
//! 1. **모니터 정체성** — 재부팅·재연결을 넘어 같은 모니터를 같다고 알아본다.
//!    메모가 어느 화면에 붙어 있었는지는 이 키에 매달린다.
//! 2. **데스크 상태의 단일 진실** — 메모·셸 위치를 파일 하나가 갖고, 창들이
//!    이벤트로 따라온다. v4는 localStorage가 주인이었는데 창이 여럿이 되면
//!    창마다 다른 React 상태를 들고 갈라진다.

use serde::Serialize;
use std::path::PathBuf;
use tauri::{Emitter, Manager, Monitor};

/// 데스크 상태 파일. v4의 `desk-backup.json`(안전망)에서 **정본**으로 승격됐다.
/// 옛 파일은 첫 실행 때 읽어 옮긴다(`migrate_legacy`).
fn state_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".toki").join("desk.json"))
}

fn legacy_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".toki").join("desk-backup.json"))
}

/// 모니터 정체성 — **해상도 + 배율**. 메모가 어느 화면에 붙어 있었는지가 여기
/// 매달리므로, 재부팅·재연결을 넘어 같은 값이 나와야 한다.
///
/// 후보를 하나씩 떨어뜨린 이유:
/// - `name`: macOS는 모델명이 아니라 **디스플레이 ID**를 준다(실측 `Monitor #16619`).
///   재부팅하면 바뀔 수 있어서, 이름을 키에 넣으면 메모가 엉뚱한 화면으로 간다.
/// - `position`: 배치를 바꾸면 변한다. 같은 모니터인데 다른 키가 된다.
/// - 열거 순서(index): 연결 순서에 따라 변한다. 노트북을 닫았다 열면 뒤바뀐다.
///
/// 해상도+배율은 **같은 모델 두 대를 구분하지 못한다.** 그 경우 `monitors()`가
/// 좌→우 위치 순으로 `#1`, `#2`를 붙인다(배치를 물리적으로 바꾸면 그 둘의
/// 메모가 서로 바뀐다 — 사라지는 것보다는 낫다).
pub fn monitor_key(m: &Monitor) -> String {
    format!("{}x{}@{}", m.size().width, m.size().height, (m.scale_factor() * 100.0).round() as u32)
}

/// 사람에게 보여줄 이름. macOS의 `Monitor #NNNN`은 사용자가 시스템 설정에서
/// 보는 이름과 달라 아무 도움이 안 된다 — 그럴 땐 해상도로 부른다.
fn display_name(m: &Monitor) -> String {
    match m.name().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(n) if !n.starts_with("Monitor #") => n.to_string(),
        _ => format!("{}×{}", m.size().width, m.size().height),
    }
}

/// UI가 쓰는 모니터 정보. 프런트는 이 키로 자기 메모를 고른다.
#[derive(Debug, Clone, Serialize)]
pub struct MonitorInfo {
    pub key: String,
    pub name: String,
    /// 이 모니터를 그리는 창 라벨. 프런트가 자기 창을 식별하는 데 쓴다.
    pub window: String,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    pub primary: bool,
}

/// 데스크 창 라벨 규약. `main`은 tauri.conf.json이 만든 첫 창이라 그대로 쓰고,
/// 두 번째 모니터부터 동적으로 만든다.
pub const PRIMARY_LABEL: &str = "main";
const EXTRA_PREFIX: &str = "desk-";

pub fn is_desk_label(label: &str) -> bool {
    label == PRIMARY_LABEL || label.starts_with(EXTRA_PREFIX)
}

/// 지금 붙어 있는 모니터들 — 창 라벨을 붙여서. 순서는 `available_monitors()`를
/// 따르되 **주 모니터를 맨 앞**으로 올린다(`main` 창이 거기 붙는다).
pub fn monitors(app: &tauri::AppHandle) -> Vec<MonitorInfo> {
    let Ok(list) = app.available_monitors() else { return Vec::new() };
    let primary_key = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| monitor_key(&m));

    let mut out: Vec<MonitorInfo> = Vec::new();
    let mut ordered: Vec<&Monitor> = list.iter().collect();
    ordered.sort_by_key(|m| (Some(monitor_key(m)) != primary_key) as u8);

    // 같은 해상도·배율이 둘 이상이면 좌→우 순으로 꼬리표를 붙여 가른다.
    let mut dup: std::collections::HashMap<String, Vec<i32>> = Default::default();
    for m in &list {
        dup.entry(monitor_key(m)).or_default().push(m.position().x);
    }
    for xs in dup.values_mut() {
        xs.sort_unstable();
    }

    for (i, m) in ordered.into_iter().enumerate() {
        let base = monitor_key(m);
        let key = match dup.get(&base) {
            Some(xs) if xs.len() > 1 => {
                let n = xs.iter().position(|x| *x == m.position().x).unwrap_or(0) + 1;
                format!("{base}#{n}")
            }
            _ => base,
        };
        let primary = Some(&key) == primary_key.as_ref();
        out.push(MonitorInfo {
            window: if i == 0 { PRIMARY_LABEL.to_string() } else { format!("{EXTRA_PREFIX}{i}") },
            name: display_name(m),
            key,
            width: m.size().width,
            height: m.size().height,
            x: m.position().x,
            y: m.position().y,
            scale: m.scale_factor(),
            primary,
        });
    }
    out
}

/// 창 라벨 → 그 창이 맡은 모니터. 프런트가 마운트 직후 자기 정체를 묻는다.
#[tauri::command]
pub fn desk_monitor(app: tauri::AppHandle, window: tauri::Window) -> Option<MonitorInfo> {
    let label = window.label().to_string();
    monitors(&app).into_iter().find(|m| m.window == label)
}

/// 붙어 있는 모니터 전부 — "이 메모를 저쪽 화면으로" 같은 메뉴가 쓴다.
#[tauri::command]
pub fn desk_monitors(app: tauri::AppHandle) -> Vec<MonitorInfo> {
    monitors(&app)
}

/// 붙어 있는 모니터마다 데스크 창을 하나씩 맞춘다 — 없으면 만들고, 사라진
/// 모니터의 창은 닫고, 남은 창은 자기 모니터에 딱 맞춘다.
///
/// Tauri에는 "모니터가 바뀌었다" 이벤트가 없다. 그래서 기동 시 1회 + 주기적으로
/// 부른다. 값이 이미 맞으면 아무것도 안 하므로(±2px 허용) 사용자가 창을 만지는
/// 걸 방해하지 않는다 — 셸은 창이 아니라 div를 끌기 때문에 창의 목표는 항상
/// "모니터 원점+크기" 하나뿐이다.
/// 진단 로그 — 트레이 앱을 죽이거나 터미널에서 띄우지 않고 상태를 보려고
/// 파일로 남긴다(2026-09-03: 그렇게 진단하다 트레이를 날렸다).
///
/// **주기적으로 부르지 않는다.** 창 생성·정리처럼 드물게 일어나는 사건만 남긴다 —
/// 3초 폴링을 찍었더니 파일이 끝없이 자랐다. 예외는 클릭스루 진단(lib.rs) —
/// 창이 보인 뒤 20초 동안만 3초에 한 줄이고, 1MB 회전이 있다.
pub fn debug_log(line: &str) {
    let Some(p) = dirs::home_dir().map(|h| h.join(".toki").join("desk-debug.log")) else { return };
    use std::io::Write;
    // 1MB를 넘으면 새로 시작한다 — 진단 파일이 디스크를 먹는 일은 없어야 한다.
    if std::fs::metadata(&p).map(|m| m.len() > 1_000_000).unwrap_or(false) {
        let _ = std::fs::remove_file(&p);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{} {}", chrono::Local::now().format("%H:%M:%S"), line);
    }
}

pub fn sync_windows(app: &tauri::AppHandle) {
    // 드래그 중엔 창이 일부러 커져 있다 — 되돌리면 끌던 메모가 잘린다.
    if DRAGGING.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let mons = monitors(app);
    if mons.is_empty() {
        return;
    }
    // 모니터 구성이 바뀔 때만 표를 남긴다 — 듀얼 모니터 보고를 실기 없이 볼 근거.
    {
        static LAST: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
        let table = mons
            .iter()
            .map(|m| format!("{}={} {}x{}@({},{}) x{:.2}{}", m.window, m.key, m.width, m.height, m.x, m.y, m.scale, if m.primary { " primary" } else { "" }))
            .collect::<Vec<_>>()
            .join(" | ");
        if let Ok(mut last) = LAST.lock() {
            if *last != table {
                debug_log(&format!("monitors: {table}"));
                *last = table;
            }
        }
    }
    let want: std::collections::HashSet<String> = mons.iter().map(|m| m.window.clone()).collect();

    for m in &mons {
        let win = match app.get_webview_window(&m.window) {
            Some(w) => w,
            None => {
                // 두 번째 모니터부터. 첫 창(main)과 같은 설정으로 띄운다 —
                // 투명·장식 없음·작업표시줄 제외·항상 위. `visible:false`로
                // 만들고 배치를 맞춘 뒤 보여야 다른 화면에서 한 프레임 번쩍이지 않는다.
                let url = tauri::WebviewUrl::App("index.html".into());
                match tauri::WebviewWindowBuilder::new(app, &m.window, url)
                    .title("Toki")
                    .decorations(false)
                    .transparent(true)
                    .shadow(false)
                    .skip_taskbar(true)
                    .always_on_top(true)
                    .visible(false)
                    .build()
                {
                    Ok(w) => {
                        eprintln!("[desk] 창 생성 {} → {}", m.window, m.key);
                        let _ = w.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
                        crate::make_window_transparent(&w);
                        w
                    }
                    Err(e) => {
                        eprintln!("[desk] 창 생성 실패 {}: {e}", m.window);
                        continue;
                    }
                }
            }
        };
        fit_to_monitor(&win, m);
    }

    /* 새로 만든 창은 숨은 채로 나온다. 다른 데스크가 이미 보이는 중이면
       같이 보여야 한다 — 안 그러면 그 모니터만 영영 깜깜하고, 넘어오는
       메모의 미리보기도 안 뜬다(2026-09-03 실기: "특정 화면에선 안 보임"). */
    let any_visible = mons.iter().any(|m| {
        app.get_webview_window(&m.window)
            .and_then(|w| w.is_visible().ok())
            .unwrap_or(false)
    });
    if any_visible {
        for m in &mons {
            if let Some(w) = app.get_webview_window(&m.window) {
                if !w.is_visible().unwrap_or(true) {
                    debug_log(&format!("show late window {}", m.window));
                    let _ = w.show();
                }
            }
        }
    }

    // 사라진 모니터의 창은 닫는다. **메모는 안 지운다** — 상태 파일에 모니터
    // 키로 남아 있어서 다시 꽂으면 제자리로 돌아온다(spec §7.0).
    for (label, win) in app.webview_windows() {
        if is_desk_label(&label) && label != PRIMARY_LABEL && !want.contains(&label) {
            eprintln!("[desk] 모니터가 사라져 창 정리: {label}");
            let _ = win.close();
        }
    }
}

/// 창을 모니터 원점·크기에 맞춘다. 이미 맞으면 건드리지 않는다.
fn fit_to_monitor(win: &tauri::WebviewWindow, m: &MonitorInfo) {
    let pos = tauri::PhysicalPosition::new(m.x, m.y);
    let size = tauri::PhysicalSize::new(m.width, m.height);
    let off_pos = win
        .outer_position()
        .map(|p| (p.x - m.x).abs() > 2 || (p.y - m.y).abs() > 2)
        .unwrap_or(true);
    let off_size = win
        .inner_size()
        .map(|s| (s.width as i64 - m.width as i64).abs() > 2 || (s.height as i64 - m.height as i64).abs() > 2)
        .unwrap_or(true);
    if off_pos {
        let _ = win.set_position(pos);
    }
    if off_size {
        let _ = win.set_size(size);
    }
}

/// 데스크 창 전부에 대해 실행. 트레이 토글·설정 반영처럼 "모든 데스크"에
/// 걸리는 동작이 하나둘이 아니라 헬퍼로 둔다.
pub fn for_each_desk<F: FnMut(&tauri::WebviewWindow)>(app: &tauri::AppHandle, mut f: F) {
    for (label, win) in app.webview_windows() {
        if is_desk_label(&label) {
            f(&win);
        }
    }
}

/// 드래그하는 동안 데스크 창을 **전체 모니터를 덮는 크기**로 늘린다.
///
/// 메모는 창 안의 요소라 창이 모니터에서 끝나면 경계를 넘을 수 없다. 마우스
/// 버튼을 쥔 동안엔 이벤트를 처음 창이 독점하므로 옆 창이 이어받지도 못한다.
/// 그래서 **끄는 동안만** 창 하나가 전 화면을 덮게 해서 메모가 실제로 건너가게
/// 하고, 놓으면 목적지 모니터의 창으로 넘긴 뒤 원래 크기로 되돌린다.
/// 쉴 때는 여전히 모니터당 창 하나라 픽셀 격자(spec §6.6)가 지켜진다.
///
/// **좌표는 points(논리)로 계산한다.** 모니터의 `position`·`size`는 각자 배율로
/// 스케일된 값이라 배율이 섞이면 그대로 더할 수 없다. points로 환산해야 한 좌표계가
/// 되고, 그 공간이 곧 확장된 창의 CSS px다.
#[derive(Debug, Clone, Serialize)]
pub struct MonRect {
    pub key: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DragSpace {
    /// 창 로컬 좌표에 더하면 확장된 창의 좌표가 되는 값.
    pub dx: f64,
    pub dy: f64,
    pub width: f64,
    pub height: f64,
    /// 확장된 창 좌표계에서의 각 모니터 영역.
    pub monitors: Vec<MonRect>,
}

/// 드래그 중에는 주기 동기화가 창 크기를 되돌리려 든다 — 잠근다.
static DRAGGING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[tauri::command]
pub fn desk_drag_begin(app: tauri::AppHandle, window: tauri::Window) -> Option<DragSpace> {
    let mons = monitors(&app);
    if mons.len() < 2 {
        return None; // 모니터가 하나면 늘릴 이유가 없다.
    }
    let label = window.label().to_string();
    let me = mons.iter().find(|m| m.window == label)?;

    let pt = |v: f64, s: f64| v / s;
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for m in &mons {
        let (ox, oy) = (pt(m.x as f64, m.scale), pt(m.y as f64, m.scale));
        let (w, h) = (pt(m.width as f64, m.scale), pt(m.height as f64, m.scale));
        x0 = x0.min(ox);
        y0 = y0.min(oy);
        x1 = x1.max(ox + w);
        y1 = y1.max(oy + h);
    }
    let (mx, my) = (pt(me.x as f64, me.scale), pt(me.y as f64, me.scale));

    let win = app.get_webview_window(&label)?;
    DRAGGING.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = win.set_position(tauri::LogicalPosition::new(x0, y0));
    let _ = win.set_size(tauri::LogicalSize::new(x1 - x0, y1 - y0));
    debug_log(&format!("drag begin {label} union=({x0},{y0})-({x1},{y1})"));

    Some(DragSpace {
        dx: mx - x0,
        dy: my - y0,
        width: x1 - x0,
        height: y1 - y0,
        monitors: mons
            .iter()
            .map(|m| MonRect {
                key: m.key.clone(),
                x: pt(m.x as f64, m.scale) - x0,
                y: pt(m.y as f64, m.scale) - y0,
                w: pt(m.width as f64, m.scale),
                h: pt(m.height as f64, m.scale),
            })
            .collect(),
    })
}

/// 드래그 끝 — 잠금을 풀고 창을 자기 모니터로 되돌린다.
#[tauri::command]
pub fn desk_drag_end(app: tauri::AppHandle) {
    DRAGGING.store(false, std::sync::atomic::Ordering::Relaxed);
    sync_windows(&app);
    debug_log("drag end");
}

/// 커서가 지금 어느 화면 위에 있고, 그 화면 안에서 어디인가.
///
/// 화면을 넘나드는 드래그의 핵심이다. 창이 모니터마다 갈려 있어 프런트는
/// 자기 창 밖의 좌표를 해석할 수 없다 — 전역 좌표 변환을 여기서 한 번에 한다.
///
/// **좌표 규약은 클릭스루와 같다**(그쪽이 이미 혼합 DPI에서 검증된 코드다):
/// macOS의 `cursor_position()`은 **주 모니터 배율**로 스케일된 값이고, 모니터의
/// `position`·`size`는 **그 모니터 자신의 배율**로 스케일된 값이다. 둘을 그냥
/// 빼면 배율이 다른 화면에서 어긋난다 — 양쪽을 전역 논리 좌표(points)로
/// 환산한 뒤에 비교한다.
#[derive(Debug, Clone, Serialize)]
pub struct CursorTarget {
    pub key: String,
    /// 그 모니터 안에서의 위치 (CSS px = 논리 좌표).
    pub x: f64,
    pub y: f64,
}

#[tauri::command]
pub fn desk_cursor_target(app: tauri::AppHandle) -> Option<CursorTarget> {
    let any = app.webview_windows().into_values().next()?;
    let cursor = any.cursor_position().ok()?;
    let primary = app.primary_monitor().ok().flatten().map(|m| m.scale_factor()).unwrap_or(1.0);
    let (gx, gy) = (cursor.x / primary, cursor.y / primary);

    for m in monitors(&app) {
        let (ox, oy) = (m.x as f64 / m.scale, m.y as f64 / m.scale);
        let (w, h) = (m.width as f64 / m.scale, m.height as f64 / m.scale);
        if gx >= ox && gx < ox + w && gy >= oy && gy < oy + h {
            return Some(CursorTarget { key: m.key, x: gx - ox, y: gy - oy });
        }
    }
    None
}

/// 데스크 상태(메모·셸)를 그대로 돌려준다. **Rust는 내용을 해석하지 않는다** —
/// 노트 스키마는 프런트 것이고, 여기서 다시 정의하면 두 곳이 갈라진다.
#[tauri::command]
pub fn desk_state_load() -> String {
    if let Some(p) = state_path() {
        if let Ok(s) = std::fs::read_to_string(&p) {
            if !s.trim().is_empty() {
                return s;
            }
        }
    }
    // v4 안전망 파일에서 승계 (한 번만 일어난다 — 저장하는 순간 새 파일이 생긴다).
    if let Some(p) = legacy_path() {
        if let Ok(s) = std::fs::read_to_string(&p) {
            if !s.trim().is_empty() {
                eprintln!("[desk] 옛 desk-backup.json에서 승계");
                return s;
            }
        }
    }
    String::new()
}

/// 상태를 저장하고 **다른 창들에** 알린다. 보낸 창은 자기 이벤트를 무시한다
/// (라벨을 같이 실어 보낸다) — 안 그러면 자기 입력이 되돌아와 커서가 튄다.
#[tauri::command]
pub fn desk_state_save(app: tauri::AppHandle, window: tauri::Window, json: String) -> Result<(), String> {
    let p = state_path().ok_or("홈 디렉토리를 찾을 수 없어요")?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, &json).map_err(|e| e.to_string())?;
    debug_log(&format!("save from={} bytes={}", window.label(), json.len()));
    let _ = app.emit("desk-changed", serde_json::json!({ "from": window.label(), "json": json }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 키는 배치가 아니라 **정체**를 담아야 한다 — 모니터를 옮겨 꽂아도
    /// 같은 키여야 메모가 제 화면으로 돌아온다.
    /// 키는 배치나 이름이 아니라 **물리 특성**을 담는다. Monitor는 생성자가
    /// 없어 직접 못 만드므로 포맷 계약을 고정한다.
    #[test]
    fn key_is_resolution_and_scale() {
        let k = |w: u32, h: u32, s: f64| format!("{}x{}@{}", w, h, (s * 100.0).round() as u32);
        assert_eq!(k(2560, 1440, 1.0), "2560x1440@100");
        // 같은 패널이라도 회전하면 다른 화면이다(대교 세팅의 U2717D/U2715H).
        assert_ne!(k(2560, 1440, 1.0), k(1440, 2560, 1.0));
        // 해상도가 같아도 배율이 다르면 다른 화면 — Retina와 외장 1x를 가른다.
        assert_ne!(k(1920, 1080, 2.0), k(1920, 1080, 1.0));
    }

    #[test]
    fn desk_labels_are_recognized() {
        assert!(is_desk_label("main"));
        assert!(is_desk_label("desk-1"));
        assert!(is_desk_label("desk-2"));
        assert!(!is_desk_label("settings"));
        assert!(!is_desk_label(""));
    }
}
