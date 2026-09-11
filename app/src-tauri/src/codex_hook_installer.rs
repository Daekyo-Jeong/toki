//! Codex CLI `~/.codex/hooks.json` 인스톨러 — Claude `hook_installer`와 동형.
//!
//! 스키마(순정 codex 0.147 실측): `{"hooks": {Event: [{"hooks": [{"type":"command",
//! "command": "...", "timeout": N}]}]}}`. Claude와 거의 같고 `matcher`가 없다.
//!
//! **신뢰 게이트는 건드리지 않는다.** codex는 `config.toml`의
//! `[hooks.state."<path>:<event>:i:j"] trusted_hash`가 없으면 훅을 안 돌린다.
//! 그 해시의 레시피는 공개돼 있지 않고(15가지 후보 전부 불일치), 설령 알아도
//! 남의 앱 보안 게이트를 우리가 대신 통과시키는 건 할 일이 아니다. 우리는
//! 항목만 넣고, 승인은 codex 안에서 사용자가 한다. UI는 "설치됨 · codex에서
//! 신뢰 승인 필요"라고 정직하게 말한다.
//!
//! 이벤트명은 `codex/<Event>`로 서버에 넘긴다(`agent=codex` 쿼리) — Claude
//! 훅과 같은 테이블에 섞여도 어디서 왔는지 안다.

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

const TOKI_TAG: &str = "# toki-managed";
/// codex가 발화하는 것 중 Claude 쪽 EVENTS와 의미가 겹치는 것만. codex엔
/// Notification·PreCompact·SessionEnd가 없다(순정 0.147 hooks.json 실측).
const EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Stop",
    "SubagentStart",
    "SubagentStop",
];

pub fn codex_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codex"))
}

pub fn hooks_path() -> Result<PathBuf> {
    Ok(codex_dir().context("no home dir")?.join("hooks.json"))
}

/// 이 머신에 codex가 있나 — `~/.codex`가 있으면 있다고 본다(agent.rs의 detect와 같은 기준).
pub fn is_available() -> bool {
    codex_dir().map(|d| d.exists()).unwrap_or(false)
}

fn read_json(p: &Path) -> Result<Value> {
    if !p.exists() {
        return Ok(json!({}));
    }
    let t = fs::read_to_string(p).context("read hooks.json")?;
    if t.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&t).context("hooks.json is not valid JSON")
}

fn write_json(p: &Path, v: &Value) -> Result<()> {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(p, serde_json::to_string_pretty(v)?).context("write hooks.json")
}

fn hook_command(port: u16, event: &str) -> String {
    format!(
        "curl -sS -m 2 -X POST -H 'Content-Type: application/json' --data-binary @- 'http://127.0.0.1:{port}/hook?event={event}&agent=codex' >/dev/null 2>&1 || true {tag}",
        port = port,
        event = event,
        tag = TOKI_TAG,
    )
}

/// Windows(M9): codex hooks.json은 `commandWindows`(Windows 전용 오버라이드)를
/// 지원한다(Codex hooks 문서). 어느 셸로 도는지는 문서가 안 밝혀서 cmd와
/// PowerShell **둘 다에서 유효한** 꼴로 쓴다 — 큰따옴표, `NUL` 장치, `||` 없음.
/// `curl.exe`로 박는 이유: PowerShell의 `curl`은 Invoke-WebRequest 별칭이다.
fn hook_command_windows(port: u16, event: &str) -> String {
    format!(
        "curl.exe -sS -m 2 -X POST -H \"Content-Type: application/json\" --data-binary \"@-\" \"http://127.0.0.1:{port}/hook?event={event}&agent=codex\" >NUL 2>&1",
    )
}

fn hook_entry(port: u16, event: &str, windows: bool) -> Value {
    let mut h = json!({ "type": "command", "command": hook_command(port, event), "timeout": 5 });
    if windows {
        h["commandWindows"] = json!(hook_command_windows(port, event));
    }
    h
}

fn entry_is_ours(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter().any(|h| {
                h.get("command").and_then(|c| c.as_str()).map(|c| c.contains(TOKI_TAG)).unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

pub fn install(port: u16) -> Result<PathBuf> {
    let p = hooks_path()?;
    install_at(&p, port)?;
    Ok(p)
}

pub fn install_at(p: &Path, port: u16) -> Result<()> {
    let mut doc = read_json(p)?;
    let root = doc.as_object_mut().context("hooks.json root not an object")?;
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("hooks not an object")?;
    for ev in EVENTS {
        let arr = hooks
            .entry(ev.to_string())
            .or_insert_with(|| Value::Array(vec![]))
            .as_array_mut()
            .context("hook entry not an array")?;
        arr.retain(|e| !entry_is_ours(e));
        arr.push(json!({ "hooks": [hook_entry(port, ev, cfg!(windows))] }));
    }
    write_json(p, &doc)
}

pub fn uninstall() -> Result<()> {
    uninstall_at(&hooks_path()?)
}

pub fn uninstall_at(p: &Path) -> Result<()> {
    if !p.exists() {
        return Ok(());
    }
    let mut doc = read_json(p)?;
    let Some(root) = doc.as_object_mut() else { return Ok(()) };
    let Some(hooks) = root.get_mut("hooks").and_then(|v| v.as_object_mut()) else {
        return Ok(());
    };
    let keys: Vec<String> = hooks.keys().cloned().collect();
    for k in keys {
        if let Some(arr) = hooks.get_mut(&k).and_then(|v| v.as_array_mut()) {
            arr.retain(|e| !entry_is_ours(e));
            if arr.is_empty() {
                hooks.remove(&k);
            }
        }
    }
    if hooks.is_empty() {
        root.remove("hooks");
    }
    write_json(p, &doc)
}

pub fn is_installed() -> Result<bool> {
    is_installed_at(&hooks_path()?)
}

pub fn is_installed_at(p: &Path) -> Result<bool> {
    if !p.exists() {
        return Ok(false);
    }
    let v = read_json(p)?;
    let Some(hooks) = v.get("hooks").and_then(|h| h.as_object()) else { return Ok(false) };
    Ok(hooks.values().filter_map(|a| a.as_array()).any(|arr| arr.iter().any(entry_is_ours)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows 항목은 `commandWindows`를 함께 갖고, 그 명령은 cmd·PowerShell
    /// 어느 쪽에서도 죽지 않는 꼴이어야 한다(POSIX 전용 토큰 금지). macOS 항목엔
    /// 그 키가 없다 — 남의 hooks.json에 불필요한 키를 넣지 않는다.
    #[test]
    fn windows_entry_has_portable_override() {
        let mac = hook_entry(4321, "Stop", false);
        assert!(mac.get("commandWindows").is_none());
        let win = hook_entry(4321, "Stop", true);
        let cw = win["commandWindows"].as_str().unwrap();
        assert!(cw.starts_with("curl.exe "), "PowerShell의 curl은 별칭이라 .exe를 박아야 함");
        assert!(cw.contains("127.0.0.1:4321/hook?event=Stop&agent=codex"));
        for bad in ["/dev/null", "||", "'", "#"] {
            assert!(!cw.contains(bad), "cmd/PowerShell 공통이 아닌 토큰: {bad}");
        }
        // 식별 태그는 여전히 command 쪽에 있다 — is_ours 판정이 거기를 본다.
        assert!(win["command"].as_str().unwrap().contains(TOKI_TAG));
    }

    

    fn sandbox(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("toki-cx-test-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base.join("hooks.json")
    }

    /// 사용자의 기존 훅(Orca 모양)은 그대로 두고 우리 것만 얹었다가 정확히 걷는다.
    #[test]
    fn merges_beside_user_hooks_and_restores() {
        let p = sandbox("merge");
        let user = json!({"hooks": {"SessionStart": [{"hooks": [{"type":"command","command":"/bin/sh '/x/orca.sh'","timeout":10}]}],
                                    "PermissionRequest": [{"hooks": [{"type":"command","command":"/bin/sh '/x/orca.sh'","timeout":10}]}]}});
        fs::write(&p, user.to_string()).unwrap();
        install_at(&p, 6996).unwrap();
        let v = read_json(&p).unwrap();
        let ss = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(ss.len(), 2, "사용자 훅 1 + 우리 훅 1");
        assert!(ss[1]["hooks"][0]["command"].as_str().unwrap().contains("agent=codex"));
        assert_eq!(v["hooks"]["PermissionRequest"].as_array().unwrap().len(), 1, "우리가 안 쓰는 이벤트는 손대지 않음");
        assert!(is_installed_at(&p).unwrap());
        uninstall_at(&p).unwrap();
        assert_eq!(read_json(&p).unwrap(), user, "원본과 동등 — 우리 흔적 0");
    }

    /// hooks.json이 없던 사용자: 설치 → 파일 생성, 제거 → hooks 키 없는 빈 문서.
    #[test]
    fn fresh_file_roundtrip() {
        let p = sandbox("fresh");
        install_at(&p, 7000).unwrap();
        assert_eq!(read_json(&p).unwrap()["hooks"].as_object().unwrap().len(), EVENTS.len());
        uninstall_at(&p).unwrap();
        assert!(read_json(&p).unwrap().get("hooks").is_none());
    }

    /// 재설치는 중복을 만들지 않는다(포트가 바뀌어도 한 항목만).
    #[test]
    fn reinstall_replaces_not_duplicates() {
        let p = sandbox("dup");
        install_at(&p, 6996).unwrap();
        install_at(&p, 7001).unwrap();
        let ss = read_json(&p).unwrap()["hooks"]["SessionStart"].as_array().unwrap().clone();
        assert_eq!(ss.len(), 1);
        assert!(ss[0]["hooks"][0]["command"].as_str().unwrap().contains(":7001/"));
    }
}
