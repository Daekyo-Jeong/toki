//! 플랫폼 분기는 **여기 한 곳** — spec §9.5 "macOS 가정을 더 늘리지 않는다".
//!
//! M9 Windows 이식(2026-09-11)에서 생긴 분기를 전부 모았다. 다른 모듈에
//! `cfg(target_os)`를 흩뿌리면 나중에 무엇이 플랫폼 가정인지 못 찾는다.
//!
//! ⚠️ 여기 Windows 쪽은 **실기 검증 전**이다. cargo-xwin 크로스빌드는 컴파일러를
//! 옮긴 것이지 런타임을 옮긴 게 아니다(reFlex 2026-08-14). 각 함수의 근거는
//! 문서(Claude Code setup·statusline·hooks)와 Win32 API 의미이고, 실측은
//! plan.md M9 검증표가 담당한다.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// GUI 앱이 자식을 띄우면 Windows는 콘솔 창을 하나 번쩍인다 — `claude.cmd`
/// 같은 배치 셸이면 더 확실히. `CREATE_NO_WINDOW`로 막는다. 에이전트 CLI를
/// 부르는 `Command`는 spawn 전에 전부 이걸 거친다. macOS에선 아무 일도 없다.
pub fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn is_ext(p: &Path, ext: &str) -> bool {
    p.extension().map(|e| e.eq_ignore_ascii_case(ext)).unwrap_or(false)
}

/// `which`(macOS) / `where.exe`(Windows) — 존재하는 첫 경로.
fn lookup_on_path(name: &str) -> Option<PathBuf> {
    let finder = if cfg!(windows) { "where.exe" } else { "which" };
    let out = quiet(&mut Command::new(finder)).arg(name).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut found: Vec<PathBuf> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect();
    if cfg!(windows) {
        // `where`는 PATHEXT 순으로 여러 줄을 준다(`claude`, `claude.cmd`, `claude.exe`).
        // 확장자 없는 것은 Git Bash용 셸 스크립트라 CreateProcess가 못 연다 —
        // .exe를 먼저, 다음 .cmd/.bat(std가 cmd.exe로 감싸 준다).
        found.retain(|p| is_ext(p, "exe") || is_ext(p, "cmd") || is_ext(p, "bat"));
        found.sort_by_key(|p| if is_ext(p, "exe") { 0 } else { 1 });
    }
    found.into_iter().next()
}

/// 탐색 결과 캐시. 설정 화면이 2초마다 상태를 묻는데 그때마다 `which`/`where`
/// 프로세스를 띄울 순 없다. 20초면 "방금 설치했다"도 곧 반영된다.
static CACHE: Mutex<Option<HashMap<String, (Instant, Option<PathBuf>)>>> = Mutex::new(None);
const CACHE_TTL: Duration = Duration::from_secs(20);

/// 에이전트 CLI(`claude`·`codex`) 경로. 순서: 환경변수 `TOKI_<NAME>_BIN`(설치
/// 경로가 특이한 사람용 수동 지정) → 캐시 → PATH → 흔한 설치 경로.
pub fn resolve_cli(name: &str) -> Option<PathBuf> {
    if let Some(v) = std::env::var_os(format!("TOKI_{}_BIN", name.to_ascii_uppercase())) {
        let p = PathBuf::from(v);
        if p.exists() {
            return Some(p);
        }
    }
    if let Some(m) = CACHE.lock().unwrap().as_ref() {
        if let Some((t, v)) = m.get(name) {
            if t.elapsed() < CACHE_TTL {
                return v.clone();
            }
        }
    }
    let found = resolve_cli_uncached(name);
    CACHE
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(name.to_string(), (Instant::now(), found.clone()));
    found
}

/// PATH → 흔한 설치 경로. Finder·탐색기에서 띄운 앱은 로그인 셸 PATH를 못
/// 받으므로 2단계가 실제 사용자 대부분을 잡는다.
fn resolve_cli_uncached(name: &str) -> Option<PathBuf> {
    if let Some(p) = lookup_on_path(name) {
        return Some(p);
    }
    let mut c: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        if cfg!(windows) {
            // 네이티브 인스톨러: %USERPROFILE%\.local\bin\claude.exe (Claude Code setup 문서)
            c.push(home.join(".local").join("bin").join(format!("{name}.exe")));
            // npm -g: %APPDATA%\npm\<name>.cmd
            if let Some(appdata) = std::env::var_os("APPDATA") {
                c.push(PathBuf::from(appdata).join("npm").join(format!("{name}.cmd")));
            }
        } else {
            c.push(home.join(".local").join("bin").join(name));
            c.push(home.join(".claude").join("local").join(name));
            c.push(home.join(".npm-global").join("bin").join(name));
        }
    }
    if !cfg!(windows) {
        c.push(PathBuf::from("/opt/homebrew/bin").join(name));
        c.push(PathBuf::from("/usr/local/bin").join(name));
        c.push(PathBuf::from("/usr/bin").join(name));
    }
    c.into_iter().find(|p| p.exists())
}

/// Windows에서 Claude Code는 훅·statusLine을 **Git Bash**로 돌린다(없으면
/// PowerShell — setup·statusline 문서). 우리 훅 명령과 statusLine 스크립트는
/// POSIX 한 줄이라 Git Bash가 있어야 산다. `where bash`는 WSL 런처
/// (System32\bash.exe)를 잡을 수 있어 못 믿는다 — git.exe 위치에서 역산한다.
/// macOS는 늘 true.
pub fn posix_shell_available() -> bool {
    if !cfg!(windows) {
        return true;
    }
    if let Some(git) = lookup_on_path("git") {
        // <Git>\cmd\git.exe 또는 <Git>\bin\git.exe → <Git>\bin\bash.exe
        if let Some(root) = git.parent().and_then(|p| p.parent()) {
            if root.join("bin").join("bash.exe").exists() {
                return true;
            }
        }
    }
    let mut roots: Vec<PathBuf> = vec![
        PathBuf::from("C:\\Program Files\\Git"),
        PathBuf::from("C:\\Program Files (x86)\\Git"),
    ];
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local).join("Programs").join("Git"));
    }
    roots.iter().any(|r| r.join("bin").join("bash.exe").exists())
}

/// settings.json에 적는 스크립트 경로. Git Bash는 따옴표 없는 백슬래시를
/// 이스케이프로 먹어 `C:\Users\...`가 조용히 깨진다(statusline 문서) —
/// Windows도 **슬래시**로 적는다. 우리는 작은따옴표로 감싸긴 하지만, 두 형태
/// 다 Windows API가 여니 슬래시가 무조건 안전하다.
pub fn shell_path(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s
    }
}

/// 그 CLI를 이 OS에 까는 한 줄 — 없을 때 화면이 보여주고 복사시킨다(Claude Code
/// setup 문서 · Codex README). 설치 URL은 lib.rs `DetectedAgent.install_url`.
pub fn install_cmd(name: &str) -> &'static str {
    match (name, cfg!(windows)) {
        ("claude", true) => "irm https://claude.ai/install.ps1 | iex",
        ("claude", false) => "curl -fsSL https://claude.ai/install.sh | bash",
        ("codex", _) => "npm install -g @openai/codex",
        _ => "",
    }
}

/// 기본 브라우저로 URL. 실패는 호출자가 결정한다.
pub fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        quiet(&mut Command::new("open")).arg(url).spawn().map(|_| ())
    }
    #[cfg(windows)]
    {
        quiet(&mut Command::new("cmd")).args(["/C", "start", "", url]).spawn().map(|_| ())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        quiet(&mut Command::new("xdg-open")).arg(url).spawn().map(|_| ())
    }
}

/// 에이전트의 **데스크톱 앱**을 띄운다 — 없으면 웹. "밥" 버튼이 부른다(대교
/// 실기 2026-09-11: 주 에이전트에 따라 다른 게 떠야 한다).
///
/// Windows 는 설치 경로를 추측하지 않는다 — Store 앱(ChatGPT)은 `%LOCALAPPDATA%`
/// 아래에 없고, 첫 판의 `Programs\Codex\Codex.exe` 추측은 실기에서 늘 웹으로
/// 떨어졌다. `Get-StartApps` 가 시작 메뉴에 등록된 앱(Store·Win32 둘 다)을 이름과
/// AppID 로 주니 그걸로 `shell:AppsFolder\<AppID>` 를 연다.
pub fn open_agent_app(agent: &str) -> std::io::Result<()> {
    let (apps, url): (&[&str], &str) = match agent {
        "codex" => (&["Codex", "ChatGPT"], "https://chatgpt.com/codex"),
        _ => (&["Claude"], "https://claude.ai"),
    };
    #[cfg(target_os = "macos")]
    {
        for app in apps {
            let ok = quiet(&mut Command::new("open")).args(["-a", app]).status().map(|s| s.success()).unwrap_or(false);
            if ok {
                return Ok(());
            }
        }
    }
    #[cfg(windows)]
    {
        if let Some(app_id) = windows_start_app_id(apps) {
            let target = format!("shell:AppsFolder\\{app_id}");
            if quiet(&mut Command::new("explorer.exe")).arg(&target).spawn().is_ok() {
                return Ok(());
            }
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = apps;
    }
    open_url(url)
}

/// 시작 메뉴 앱 목록에서 이름이 정확히 일치하는 첫 AppID. 이름 순서가 우선순위.
#[cfg(windows)]
fn windows_start_app_id(names: &[&str]) -> Option<String> {
    let out = quiet(&mut Command::new("powershell.exe"))
        .args(["-NoProfile", "-NonInteractive", "-Command",
               "Get-StartApps | ForEach-Object { $_.Name + '`t' + $_.AppID }"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let rows: Vec<(String, String)> = text
        .lines()
        .filter_map(|l| l.split_once('\t').map(|(n, id)| (n.trim().to_string(), id.trim().to_string())))
        .collect();
    names.iter().find_map(|want| rows.iter().find(|(n, _)| n.eq_ignore_ascii_case(want)).map(|(_, id)| id.clone()))
}

/// 세션 로그의 `cwd`가 임시 폴더인가(프로젝트 귀속에서 제외). macOS는
/// `/var`↔`/private/var` 심링크 양쪽, Windows는 `%TEMP%`(AppData\Local\Temp).
pub fn is_transient_cwd(cwd: &str) -> bool {
    if cwd.starts_with("/tmp")
        || cwd.starts_with("/private/var/folders")
        || cwd.starts_with("/var/folders")
        || cwd.starts_with("/private/tmp")
    {
        return true;
    }
    let lower = cwd.to_ascii_lowercase().replace('\\', "/");
    lower.contains("/appdata/local/temp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_cwd_both_platforms() {
        assert!(is_transient_cwd("/tmp/x"));
        assert!(is_transient_cwd("/private/var/folders/ab/T/x"));
        assert!(is_transient_cwd("C:\\Users\\dk\\AppData\\Local\\Temp\\codex-1"));
        assert!(is_transient_cwd("c:/users/dk/appdata/local/temp/x"));
        assert!(!is_transient_cwd("C:\\Users\\dk\\proj"));
        assert!(!is_transient_cwd("/Users/dk/proj"));
    }

    #[test]
    fn shell_path_keeps_posix_untouched() {
        assert_eq!(shell_path(Path::new("/Users/dk/.toki/a.sh")), "/Users/dk/.toki/a.sh");
    }

    /// 이 머신(macOS)에서도 탐색 사슬이 실제로 돈다 — `which`가 죽으면 None이지
    /// 패닉이 아니어야 한다.
    #[test]
    fn resolve_cli_never_panics() {
        let _ = resolve_cli("definitely-not-a-real-cli-xyz");
        assert!(posix_shell_available());
    }
}
