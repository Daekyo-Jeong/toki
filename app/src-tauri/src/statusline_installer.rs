//! Claude Code statusLine 인스톨러 — `hook_installer`와 동형(태그·백업·복구).
//!
//! 왜 필요한가: 배터리의 1순위 소스(`~/.toki/usage-pct`)는 statusLine 스크립트가
//! 적는데, v4까진 이 스크립트를 **손으로** 깔아야 했고 개인 경로가 하드코딩돼
//! 있었다. 남의 맥에선 영영 안 뜨는 구조였다. spec §5.4 — "터미널에 이걸
//! 붙여넣으세요"는 실패다.
//!
//! 계약:
//! 1. 번들에 박힌 스크립트(`include_str!`)를 `~/.toki/statusline-toki.sh`로 내보낸다.
//!    본문은 고정 — 머신마다 치환하지 않는다.
//! 2. 사용자의 기존 `statusLine`은 두 곳에 보존한다:
//!    - `~/.toki/statusline-prev.json` — 원본 JSON 그대로 (uninstall 때 복원용)
//!    - `~/.toki/statusline-next.sh`   — 그 명령을 실행 가능한 파일로 (스크립트가 체이닝)
//!    이미 우리 것이 설치돼 있으면 prev/next를 **덮지 않는다** — 원본을 잃는다.
//! 3. `settings.json`의 `statusLine`을 우리 명령(`# toki-managed` 태그)으로 교체.
//! 4. uninstall은 prev를 그대로 되돌리고, 없었으면 키를 지운다.
//!
//! 절대 조건: 남의 statusLine을 죽이지 않는다.

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

const TOKI_TAG: &str = "# toki-managed";
const SCRIPT: &str = include_str!("../statusline-toki.sh");

/// 인스톨러가 다루는 경로 묶음. 테스트에서 임시 디렉토리로 바꿔 끼운다.
#[derive(Debug, Clone)]
pub struct Paths {
    pub settings: PathBuf,
    pub toki_dir: PathBuf,
}

impl Paths {
    pub fn default() -> Result<Self> {
        let home = dirs::home_dir().context("no home dir")?;
        Ok(Self {
            settings: home.join(".claude").join("settings.json"),
            toki_dir: home.join(".toki"),
        })
    }
    fn script(&self) -> PathBuf {
        self.toki_dir.join("statusline-toki.sh")
    }
    fn next(&self) -> PathBuf {
        self.toki_dir.join("statusline-next.sh")
    }
    fn prev(&self) -> PathBuf {
        self.toki_dir.join("statusline-prev.json")
    }
}

fn read_settings(p: &Path) -> Result<Value> {
    if !p.exists() {
        return Ok(json!({}));
    }
    let text = fs::read_to_string(p).context("read settings.json")?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).context("settings.json is not valid JSON")
}

fn write_settings(p: &Path, v: &Value) -> Result<()> {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(p, serde_json::to_string_pretty(v)?).context("write settings.json")
}

fn write_executable(p: &Path, body: &str) -> Result<()> {
    fs::write(p, body).with_context(|| format!("write {}", p.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn our_command(script: &Path) -> String {
    format!("/bin/sh '{}' {}", script.display(), TOKI_TAG)
}

fn is_ours(status_line: &Value) -> bool {
    status_line
        .get("command")
        .and_then(|c| c.as_str())
        .map(|c| c.contains(TOKI_TAG))
        .unwrap_or(false)
}

/// 사용자의 statusLine 값을 체이닝용 실행 파일 본문으로. `type: command`만
/// 지원한다 — Claude Code의 statusLine은 현재 그것뿐이다. 다른 모양이면
/// 체이닝을 포기하되(파일을 안 만든다) 원본 JSON은 prev로 보존되니 복원은 된다.
fn next_script_body(status_line: &Value) -> Option<String> {
    if status_line.get("type").and_then(|t| t.as_str()) != Some("command") {
        return None;
    }
    let cmd = status_line.get("command")?.as_str()?.trim();
    if cmd.is_empty() {
        return None;
    }
    Some(format!("#!/bin/sh\n# 사용자의 원래 statusLine — Toki가 payload를 이어 넘긴다.\n{cmd}\n"))
}

pub fn install() -> Result<PathBuf> {
    install_at(&Paths::default()?)
}

pub fn install_at(paths: &Paths) -> Result<PathBuf> {
    fs::create_dir_all(&paths.toki_dir).context("create ~/.toki")?;
    write_executable(&paths.script(), SCRIPT)?;

    let mut settings = read_settings(&paths.settings)?;
    let root = settings.as_object_mut().context("settings.json root not an object")?;

    match root.get("statusLine") {
        Some(existing) if !is_ours(existing) => {
            // 처음 설치: 원본 보존. prev가 이미 있으면(예: 이전 설치의 잔재) 덮지 않는다.
            if !paths.prev().exists() {
                fs::write(&paths.prev(), serde_json::to_string_pretty(existing)?)?;
            }
            match next_script_body(existing) {
                Some(body) => write_executable(&paths.next(), &body)?,
                None => {
                    let _ = fs::remove_file(paths.next());
                }
            }
        }
        Some(_) => {
            // 이미 우리 것 — prev/next 그대로. 스크립트 본문만 최신으로 갱신됐다.
        }
        None => {
            // 원래 statusLine이 없었다 — 체이닝 대상 없음, 복원할 것도 없음.
            let _ = fs::remove_file(paths.next());
            let _ = fs::remove_file(paths.prev());
        }
    }

    root.insert(
        "statusLine".to_string(),
        json!({ "type": "command", "command": our_command(&paths.script()) }),
    );
    write_settings(&paths.settings, &settings)?;
    Ok(paths.settings.clone())
}

pub fn uninstall() -> Result<()> {
    uninstall_at(&Paths::default()?)
}

pub fn uninstall_at(paths: &Paths) -> Result<()> {
    if paths.settings.exists() {
        let mut settings = read_settings(&paths.settings)?;
        if let Some(root) = settings.as_object_mut() {
            if root.get("statusLine").map(is_ours).unwrap_or(false) {
                match fs::read_to_string(paths.prev()) {
                    Ok(prev) => {
                        let v: Value = serde_json::from_str(&prev).context("prev json")?;
                        root.insert("statusLine".to_string(), v);
                    }
                    Err(_) => {
                        root.remove("statusLine");
                    }
                }
                write_settings(&paths.settings, &settings)?;
            }
        }
    }
    let _ = fs::remove_file(paths.prev());
    let _ = fs::remove_file(paths.next());
    let _ = fs::remove_file(paths.script());
    Ok(())
}

pub fn is_installed() -> Result<bool> {
    is_installed_at(&Paths::default()?)
}

pub fn is_installed_at(paths: &Paths) -> Result<bool> {
    if !paths.settings.exists() {
        return Ok(false);
    }
    let v = read_settings(&paths.settings)?;
    Ok(v.get("statusLine").map(is_ours).unwrap_or(false))
}

#[allow(dead_code)]
fn _unused(_: Map<String, Value>) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(name: &str) -> Paths {
        let base = std::env::temp_dir().join(format!("toki-sl-test-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join(".claude")).unwrap();
        Paths { settings: base.join(".claude").join("settings.json"), toki_dir: base.join(".toki") }
    }

    /// 원래 statusLine이 없던 사용자: 설치 → 우리 것, 제거 → 키 자체가 사라진다.
    #[test]
    fn fresh_user_roundtrip_leaves_no_trace() {
        let p = sandbox("fresh");
        fs::write(&p.settings, r#"{"theme":"dark"}"#).unwrap();
        install_at(&p).unwrap();
        assert!(is_installed_at(&p).unwrap());
        assert!(p.script().exists(), "스크립트가 내보내져야 함");
        assert!(!p.next().exists(), "체이닝 대상이 없으면 next도 없어야 함");
        uninstall_at(&p).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&p.settings).unwrap()).unwrap();
        assert_eq!(v, json!({"theme":"dark"}), "원본과 동등해야 함 — 우리 흔적 0");
    }

    /// 기존 statusLine이 셸 한 줄짜리 명령인 사용자(실제 Orca 사례):
    /// 설치 후 next.sh에 그 명령이 그대로 들어가고, 제거하면 정확히 복원된다.
    #[test]
    fn existing_command_is_chained_and_restored_verbatim() {
        let p = sandbox("chain");
        let orig = json!({"type":"command","command":"if [ -x '/x/y z.sh' ]; then /bin/sh '/x/y z.sh'; else cat >/dev/null; fi"});
        fs::write(&p.settings, json!({"statusLine": orig, "hooks": {}}).to_string()).unwrap();
        install_at(&p).unwrap();
        let next = fs::read_to_string(p.next()).unwrap();
        assert!(next.contains("if [ -x '/x/y z.sh' ]"), "원래 명령이 따옴표 그대로 들어가야 함");
        let v: Value = serde_json::from_str(&fs::read_to_string(&p.settings).unwrap()).unwrap();
        assert!(is_ours(&v["statusLine"]));
        assert_eq!(v["hooks"], json!({}), "다른 키는 건드리지 않는다");
        uninstall_at(&p).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&p.settings).unwrap()).unwrap();
        assert_eq!(v["statusLine"], orig, "원본 statusLine이 글자 그대로 복원돼야 함");
        assert!(!p.next().exists() && !p.prev().exists());
    }

    /// 두 번 설치해도 원본(prev)을 잃지 않는다 — 두 번째 설치는 우리 것 위에 하는 것이라.
    #[test]
    fn double_install_keeps_original() {
        let p = sandbox("twice");
        let orig = json!({"type":"command","command":"echo hi"});
        fs::write(&p.settings, json!({"statusLine": orig}).to_string()).unwrap();
        install_at(&p).unwrap();
        install_at(&p).unwrap();
        let prev: Value = serde_json::from_str(&fs::read_to_string(p.prev()).unwrap()).unwrap();
        assert_eq!(prev, orig);
        uninstall_at(&p).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&p.settings).unwrap()).unwrap();
        assert_eq!(v["statusLine"], orig);
    }

    /// 내보낸 스크립트에 개인 경로가 없어야 한다 — M0에서 M4로 이월한 grep 예외를 닫는 조건.
    #[test]
    fn exported_script_has_no_personal_path() {
        assert!(!SCRIPT.contains("/Users/"), "스크립트 본문에 절대 홈 경로가 있으면 안 됨");
        assert!(SCRIPT.contains("statusline-next.sh"), "사이드카 체이닝이 있어야 함");
    }
}
