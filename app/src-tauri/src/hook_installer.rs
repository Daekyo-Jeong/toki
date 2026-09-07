//! Install/uninstall toki hooks in ~/.claude/settings.json.
//!
//! Strategy:
//! - On install: if no backup exists yet, copy current settings.json to
//!   settings.json.toki-backup. Then merge toki-managed hooks into a new
//!   settings.json. We tag our commands so we can identify+remove them later
//!   without losing user-added hooks.
//! - On uninstall: walk the hooks tree and remove any command containing the
//!   TOKI_TAG marker. If hooks become empty, remove key. Write back.
//!   Backup file is left in place for safety.

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

const TOKI_TAG: &str = "# toki-managed";
const EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "Stop",
    "SubagentStop",
    "PreToolUse",
    "PostToolUse",
    "UserPromptSubmit",
    "Notification",
    "PreCompact",
];

fn settings_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("no home dir")?;
    Ok(home.join(".claude").join("settings.json"))
}

fn backup_path() -> Result<PathBuf> {
    let p = settings_path()?;
    Ok(p.with_extension("json.toki-backup"))
}

fn read_settings(p: &Path) -> Result<Value> {
    if !p.exists() {
        return Ok(json!({}));
    }
    let text = fs::read_to_string(p).context("read settings.json")?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    let v: Value = serde_json::from_str(&text).context("settings.json is not valid JSON")?;
    Ok(v)
}

fn write_settings(p: &Path, v: &Value) -> Result<()> {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).ok();
    }
    let pretty = serde_json::to_string_pretty(v)?;
    fs::write(p, pretty).context("write settings.json")?;
    Ok(())
}

fn hook_command(port: u16, event: &str) -> String {
    // curl posts stdin JSON. `|| true` ensures Claude Code never fails on us.
    // Marker is at the END so the shell runs curl first, then ignores the comment.
    format!(
        "curl -sS -m 2 -X POST -H 'Content-Type: application/json' --data-binary @- 'http://127.0.0.1:{port}/hook?event={event}' >/dev/null 2>&1 || true {tag}",
        port = port,
        event = event,
        tag = TOKI_TAG,
    )
}

fn is_toki_managed(cmd: &str) -> bool {
    cmd.contains(TOKI_TAG)
}

/// Install hooks. Returns the path that was written.
pub fn install(port: u16) -> Result<PathBuf> {
    let p = settings_path()?;
    let bp = backup_path()?;

    // First-time install: take a backup snapshot
    if !bp.exists() && p.exists() {
        fs::copy(&p, &bp).context("backup settings.json")?;
    }

    let mut settings = read_settings(&p)?;
    let root = settings.as_object_mut().context("settings.json root not an object")?;
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("settings.json hooks not an object")?;

    for ev in EVENTS {
        let cmd = hook_command(port, ev);

        // We replace any existing toki-managed entry, preserve user entries.
        let arr = hooks
            .entry(ev.to_string())
            .or_insert_with(|| Value::Array(vec![]))
            .as_array_mut()
            .context("hook entry not an array")?;

        // Remove old toki-managed entries first
        arr.retain(|entry| !entry_is_toki_managed(entry));

        // Append our entry
        arr.push(json!({
            "matcher": "",
            "hooks": [{
                "type": "command",
                "command": cmd,
            }]
        }));
    }

    write_settings(&p, &settings)?;
    Ok(p)
}

/// Uninstall: remove all toki-managed hook entries. Leaves user hooks intact.
pub fn uninstall() -> Result<()> {
    let p = settings_path()?;
    if !p.exists() {
        return Ok(());
    }
    let mut settings = read_settings(&p)?;
    let Some(root) = settings.as_object_mut() else { return Ok(()) };
    let Some(hooks) = root.get_mut("hooks").and_then(|v| v.as_object_mut()) else {
        return Ok(());
    };

    let event_keys: Vec<String> = hooks.keys().cloned().collect();
    for ev in event_keys {
        if let Some(arr) = hooks.get_mut(&ev).and_then(|v| v.as_array_mut()) {
            arr.retain(|entry| !entry_is_toki_managed(entry));
            if arr.is_empty() {
                hooks.remove(&ev);
            }
        }
    }

    // If hooks is now empty, drop it entirely
    if hooks.is_empty() {
        root.remove("hooks");
    }

    write_settings(&p, &settings)?;
    Ok(())
}

/// Returns true if our hooks are currently installed in settings.json.
pub fn is_installed() -> Result<bool> {
    let p = settings_path()?;
    if !p.exists() {
        return Ok(false);
    }
    let v = read_settings(&p)?;
    let Some(hooks) = v.get("hooks").and_then(|h| h.as_object()) else {
        return Ok(false);
    };
    for entries in hooks.values() {
        let Some(arr) = entries.as_array() else { continue };
        if arr.iter().any(entry_is_toki_managed) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn entry_is_toki_managed(entry: &Value) -> bool {
    // entry = { matcher, hooks: [ { type, command } ] }
    entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter().any(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .map(is_toki_managed)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}
