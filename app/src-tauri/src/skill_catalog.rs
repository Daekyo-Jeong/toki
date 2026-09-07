//! Filesystem scanner for Claude Code skills.
//!
//! Two scopes:
//! - **My** — user-authored skills + slash commands
//!     - `~/.claude/skills/<name>/SKILL.md`
//!     - `~/.claude/commands/*.md`
//!     - `<project>/.claude/skills/<name>/SKILL.md` (cwd at app start)
//! - **Library** — installed plugin skills
//!     - `~/.claude/plugins/marketplaces/*/plugins/<plugin>/skills/<name>/SKILL.md`
//!     - `~/.claude/plugins/marketplaces/*/external_plugins/<plugin>/skills/<name>/SKILL.md`
//!
//! SKILL.md format: YAML frontmatter (---\n key: value ... \n---) then body.
//! We only need `name` and `description` from the frontmatter; multi-line
//! descriptions are folded into a single line. Files without frontmatter
//! (loose `commands/*.md`) fall back to filename for name.

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct SkillManifest {
    pub name: String,
    pub description: Option<String>,
    /// Plugin / source identifier (e.g. "hookify", "frontend-design",
    /// "user-commands"). None for top-level `~/.claude/skills/`.
    pub plugin: Option<String>,
    /// Where the file lives — useful for debugging.
    pub path: String,
}

/// User-authored skills (anything outside the installed plugins tree).
pub fn scan_my_skills() -> Vec<SkillManifest> {
    let Some(home) = dirs::home_dir() else { return Vec::new() };
    let mut out = Vec::new();

    // ~/.claude/skills/<name>/SKILL.md
    let user_skills = home.join(".claude").join("skills");
    scan_skills_dir(&user_skills, None, &mut out);

    // ~/.claude/commands/*.md  (slash commands)
    let user_commands = home.join(".claude").join("commands");
    if let Ok(rd) = std::fs::read_dir(&user_commands) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("md") {
                if let Some(m) = parse_command_md(&p, Some("user-commands")) {
                    out.push(m);
                }
            }
        }
    }

    out
}

/// Installed plugin skills — `~/.claude/plugins/marketplaces/*/plugins/.../skills/...`.
pub fn scan_library_skills() -> Vec<SkillManifest> {
    let Some(home) = dirs::home_dir() else { return Vec::new() };
    let marketplaces = home.join(".claude").join("plugins").join("marketplaces");
    let mut out = Vec::new();

    let Ok(mp_iter) = std::fs::read_dir(&marketplaces) else { return out };
    for mp_entry in mp_iter.flatten() {
        let mp_path = mp_entry.path();
        if !mp_path.is_dir() {
            continue;
        }
        // Two known plugin roots: `plugins/` and `external_plugins/`.
        for plugins_root_name in ["plugins", "external_plugins"] {
            let plugins_root = mp_path.join(plugins_root_name);
            let Ok(plugin_iter) = std::fs::read_dir(&plugins_root) else { continue };
            for plugin_entry in plugin_iter.flatten() {
                let plugin_path = plugin_entry.path();
                if !plugin_path.is_dir() {
                    continue;
                }
                let plugin_name = plugin_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                let skills_dir = plugin_path.join("skills");
                scan_skills_dir(&skills_dir, Some(plugin_name.as_str()), &mut out);
            }
        }
    }

    out
}

/// Walk `<dir>/<name>/SKILL.md` entries.
fn scan_skills_dir(dir: &Path, plugin: Option<&str>, out: &mut Vec<SkillManifest>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let skill_md = p.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }
        if let Some(m) = parse_skill_md(&skill_md, plugin) {
            out.push(m);
        }
    }
}

/// Parse a `SKILL.md` with YAML-ish frontmatter. Only `name` / `description`
/// pulled out. Multi-line descriptions get folded to a single line.
fn parse_skill_md(path: &Path, plugin: Option<&str>) -> Option<SkillManifest> {
    let text = std::fs::read_to_string(path).ok()?;
    let frontmatter = extract_frontmatter(&text);

    let (name, description) = match frontmatter {
        Some(fm) => {
            let n = pluck(&fm, "name");
            let d = pluck(&fm, "description");
            (n, d)
        }
        None => (None, None),
    };

    // Fallback: skill directory name if frontmatter has no `name`.
    let dir_name = path
        .parent()
        .and_then(|d| d.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let final_name = name.unwrap_or(dir_name);
    if final_name.is_empty() {
        return None;
    }

    Some(SkillManifest {
        name: final_name,
        description,
        plugin: plugin.map(|s| s.to_string()),
        path: path.to_string_lossy().to_string(),
    })
}

/// Slash command `*.md` — frontmatter optional; filename is the name.
fn parse_command_md(path: &Path, plugin: Option<&str>) -> Option<SkillManifest> {
    let text = std::fs::read_to_string(path).ok()?;
    let frontmatter = extract_frontmatter(&text);
    let name_fm = frontmatter.as_deref().and_then(|fm| pluck(fm, "name"));
    let desc = frontmatter.as_deref().and_then(|fm| pluck(fm, "description"));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let name = name_fm.unwrap_or_else(|| stem.to_string());
    if name.is_empty() {
        return None;
    }
    Some(SkillManifest {
        name,
        description: desc,
        plugin: plugin.map(|s| s.to_string()),
        path: path.to_string_lossy().to_string(),
    })
}

/// Returns the YAML body between leading `---` and the next `---` line.
fn extract_frontmatter(text: &str) -> Option<String> {
    let stripped = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n"))?;
    // Find `---` on its own line.
    let mut acc = String::new();
    for line in stripped.lines() {
        if line == "---" {
            return Some(acc);
        }
        acc.push_str(line);
        acc.push('\n');
    }
    None
}

/// Single-key pluck. Handles `key: value` or block scalar continuation
/// (next non-key lines are appended, folded with spaces).
fn pluck(frontmatter: &str, key: &str) -> Option<String> {
    let mut lines = frontmatter.lines().peekable();
    let needle = format!("{}:", key);
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with(&needle) {
            continue;
        }
        let rest = trimmed[needle.len()..].trim_start();
        let mut value = rest.trim().to_string();
        // Continuation: lines that don't start a new `key:` get appended.
        while let Some(peek) = lines.peek() {
            let pt = peek.trim_start();
            if pt.is_empty() {
                lines.next();
                continue;
            }
            // Heuristic for "this is a new key": `<ident>:` near the start.
            if pt
                .split_once(':')
                .map(|(k, _)| k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                .unwrap_or(false)
            {
                break;
            }
            value.push(' ');
            value.push_str(pt);
            lines.next();
        }
        // Strip surrounding quotes if any.
        let v = value.trim();
        if (v.starts_with('"') && v.ends_with('"'))
            || (v.starts_with('\'') && v.ends_with('\''))
        {
            return Some(v[1..v.len() - 1].to_string());
        }
        return Some(v.to_string());
    }
    None
}

#[allow(dead_code)]
fn _silence_unused(_: PathBuf) {}
