//! Coaching LLM backend abstraction (V4-6 → v5 M5).
//!
//! Three backends behind one `complete()`:
//! - **claude -p** / **codex exec** — 에이전트 CLI. v5 기본(spec §4.1):
//!   추가 설치 0, 품질이 로컬 7.8B와 비교 불가. 사용자 구독 쿼터를 쓴다
//!   ("쿼터 아이러니") — 그래서 명시적 버튼에서만, 소비 고지와 함께 돈다.
//!   설정 `auto`는 최근 활동 에이전트를 따른다(배터리 창 선택과 같은 규칙).
//! - **Ollama (option)** — local HTTP at :11434. Zero quota, offline,
//!   private ("밖으로 나가지 않음"). v4의 기본이었고 v5에선 선택지로 남는다.
//!   Default model: `exaone3.5:7.8b` (LG, Korean-native).
//!
//! Pattern DETECTION stays deterministic in `retro.rs::distill`; the model
//! only phrases the extracted signals (Socratic). That keeps the reasoning
//! bar low enough for a mid-size local model to clear.

use anyhow::{anyhow, Result};
use std::time::Duration;

const OLLAMA_URL: &str = "http://localhost:11434/api/generate";
// `/api/ps` lists models currently resident in memory — how we tell a cold
// first run (model not loaded yet) from a warm one (P3-15).
const OLLAMA_PS_URL: &str = "http://localhost:11434/api/ps";
// First call cold-loads the model into RAM (10-30s on a 7.8B) on top of
// generation, so allow generous headroom.
const OLLAMA_TIMEOUT: Duration = Duration::from_secs(180);

pub const DEFAULT_OLLAMA_MODEL: &str = "exaone3.5:7.8b";
pub const DEFAULT_CLAUDE_MODEL: &str = "sonnet";
/// `app_settings.coach_backend` 값들. `auto`가 v5 기본.
pub const BACKEND_AUTO: &str = "auto";
pub const BACKEND_CLAUDE: &str = "claude";
pub const BACKEND_CODEX: &str = "codex";
pub const BACKEND_OLLAMA: &str = "ollama";
/// 설정 순환 순서(UI의 cycle 행이 이 순서를 따른다).
pub const BACKEND_CYCLE: [&str; 4] = [BACKEND_AUTO, BACKEND_CLAUDE, BACKEND_CODEX, BACKEND_OLLAMA];

#[derive(Debug, Clone)]
pub enum Backend {
    /// Local Ollama. `model` is a tag like "exaone3.5:7.8b".
    Ollama { model: String },
    /// `claude -p` subprocess. `model` is an alias like "sonnet"/"opus".
    ClaudeP { model: String },
    /// `codex exec` subprocess. 모델은 사용자의 `~/.codex/config.toml`을
    /// 그대로 따른다(`-m`을 넘기지 않는다) — 그 사람이 평소 쓰는 모델·플랜이
    /// 곧 코칭 모델이다.
    CodexExec,
}

impl Backend {
    /// Build from the `app_settings` strings.
    /// - `claude` / `codex` / `ollama` → 그대로.
    /// - `auto`(및 모름) → 최근 활동 에이전트(`recent_agent`, `db.last_active_agent()`)를
    ///   따른다. 그게 codex면 `codex exec`, 아니면 `claude -p`. 활동 기록이 없으면
    ///   **설치된 CLI**로 정한다(claude 우선, 없으면 codex).
    pub fn from_settings(backend: &str, ollama_model: &str, recent_agent: Option<&str>) -> Self {
        match backend {
            BACKEND_CLAUDE => Backend::ClaudeP { model: DEFAULT_CLAUDE_MODEL.to_string() },
            BACKEND_CODEX => Backend::CodexExec,
            BACKEND_OLLAMA => Backend::Ollama { model: nonempty_or(ollama_model, DEFAULT_OLLAMA_MODEL) },
            _ => match recent_agent {
                Some("codex") => Backend::CodexExec,
                Some(_) => Backend::ClaudeP { model: DEFAULT_CLAUDE_MODEL.to_string() },
                None => {
                    if crate::planner::resolve_claude_bin().is_none() && resolve_codex_bin().is_some() {
                        Backend::CodexExec
                    } else {
                        Backend::ClaudeP { model: DEFAULT_CLAUDE_MODEL.to_string() }
                    }
                }
            },
        }
    }

    /// 설정 + DB(최근 활동 에이전트)에서 백엔드를 정한다 — 모든 코칭 경로의 진입점.
    pub fn resolve(db: &crate::db::Db) -> Self {
        let s = db.load_settings().unwrap_or_default();
        let recent = db.last_active_agent().ok().flatten();
        Backend::from_settings(&s.coach_backend, &s.coach_ollama_model, recent.as_deref())
    }

    pub fn label(&self) -> String {
        match self {
            Backend::Ollama { model } => format!("ollama:{model}"),
            Backend::ClaudeP { model } => format!("claude:{model}"),
            Backend::CodexExec => "codex".to_string(),
        }
    }

    /// UI용 종류 문자열: "claude" | "codex" | "ollama".
    pub fn kind(&self) -> &'static str {
        match self {
            Backend::Ollama { .. } => BACKEND_OLLAMA,
            Backend::ClaudeP { .. } => BACKEND_CLAUDE,
            Backend::CodexExec => BACKEND_CODEX,
        }
    }

    /// 딥 코칭 한 번의 상한. 백엔드마다 성격이 달라 하나로 못 묶는다:
    /// `claude -p`는 단발 완성(실측 122~167초)이라 480초면 넉넉하지만,
    /// `codex exec`는 사용자 config(MCP·알림·추론 강도)를 그대로 물고 와서
    /// 같은 프롬프트에 15분을 넘겼다. `codex_exec`의 오버라이드 셋을 넣고
    /// 98초로 떨어졌지만, 모델·플랜이 사람마다 달라 여유를 더 둔다.
    /// Ollama는 로컬이라 오래 매달릴 이유가 없다.
    pub fn deep_timeout(&self) -> Duration {
        match self {
            Backend::ClaudeP { .. } => crate::planner::DEEP_TIMEOUT,
            Backend::CodexExec => Duration::from_secs(900),
            Backend::Ollama { .. } => OLLAMA_TIMEOUT,
        }
    }

    /// 프롬프트가 사용자 CLI를 타고 밖으로 나가는가(spec §9.2 고지용).
    pub fn egresses(&self) -> bool {
        !matches!(self, Backend::Ollama { .. })
    }
}

/// `codex` 바이너리 — planner::resolve_claude_bin과 같은 사슬(PATH → 흔한 설치 경로).
/// Finder에서 띄운 앱은 로그인 셸 PATH를 못 받으므로 2단계가 실제 사용자 대부분을 잡는다.
pub fn resolve_codex_bin() -> Option<std::path::PathBuf> {
    crate::platform::resolve_cli("codex")
}

fn nonempty_or(s: &str, default: &str) -> String {
    let t = s.trim();
    if t.is_empty() { default.to_string() } else { t.to_string() }
}

/// One-shot completion. `cwd` only matters for the agent CLIs (CLAUDE.md /
/// AGENTS.md scope); Ollama ignores it.
pub fn complete(prompt: &str, backend: &Backend, cwd: Option<&std::path::Path>) -> Result<String> {
    complete_with_timeout(prompt, backend, cwd, None)
}

/// `complete` + 명시 타임아웃. 딥 코칭(수백 KB 프롬프트)은 planner의 60초로는
/// 모자라 `planner::DEEP_TIMEOUT`을 넘긴다. `None`이면 백엔드별 기본.
pub fn complete_with_timeout(
    prompt: &str,
    backend: &Backend,
    cwd: Option<&std::path::Path>,
    timeout: Option<Duration>,
) -> Result<String> {
    match backend {
        Backend::Ollama { model } => ollama_generate(prompt, model),
        Backend::ClaudeP { model } => {
            let raw = match timeout {
                Some(t) => crate::planner::call_with_timeout(prompt, None, Some(model), cwd, t)?,
                None => crate::planner::call(prompt, None, Some(model), cwd)?,
            };
            parse_claude_envelope(&raw)
        }
        Backend::CodexExec => codex_exec(prompt, cwd, timeout.unwrap_or(CODEX_TIMEOUT)),
    }
}

const CODEX_TIMEOUT: Duration = Duration::from_secs(180);

/// `codex exec` 한 번. 프롬프트는 **stdin**으로 넘긴다(인자로 주면 수백 KB가
/// argv 한계에 걸린다). 마지막 메시지는 `-o <file>`로 받는다 — stdout엔 진행
/// 로그가 섞여서 파싱이 불안정하다.
/// - `--sandbox read-only`: 코칭은 읽고 쓰기만 한다. 셸 실행이 필요 없다.
/// - `--ephemeral`: 세션 파일을 안 남긴다 — 코칭 호출이 `~/.codex/sessions`
///   롤아웃으로 남으면 Toki 자신이 그걸 XP로 세는 자기오염이 생긴다.
/// - `--skip-git-repo-check`: cwd가 레포가 아니어도 돈다(`~/.toki`에서 실행).
///
/// **사용자 config를 세 가지만 덮는다** (2026-09-02 실측: 15분+ → 98초).
/// codex exec는 대화형 codex와 같은 설정을 그대로 읽어서, 평소 세팅이
/// 그대로 코칭 한 번에 실린다:
/// - `mcp_servers={}` — 코칭은 도구를 하나도 안 쓴다. 그런데 등록된 MCP
///   서버를 전부 기동한다(대교 맥: figma·node_repl·computer-use, 그중 하나는
///   `startup_timeout_sec=120`). 순수 오버헤드다.
/// - `notify=[]` — 턴 종료 훅이 외부 앱을 띄운다. 코칭엔 알릴 사람이 없다.
/// - `model_reasoning_effort="low"` — 헌법상 탐지는 결정론 Rust가 이미 끝냈고
///   LLM은 **주어진 데이터를 문장화**한다. 이 일에 high는 과하다(대교 전역
///   설정이 high라 184KB 프롬프트가 15분을 넘겼다). 품질 비교에서 low 출력이
///   claude sonnet 판과 같은 수준의 근거 인용을 냈다.
///
/// 모델 자체는 안 건드린다 — 그 사람이 평소 쓰는 모델이 곧 코칭 모델이다.
fn codex_exec(prompt: &str, cwd: Option<&std::path::Path>, timeout: Duration) -> Result<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let bin = resolve_codex_bin()
        .ok_or_else(|| anyhow!("codex CLI를 못 찾았어요 — `npm i -g @openai/codex` 또는 `brew install codex`"))?;
    let out_path = std::env::temp_dir().join(format!("toki-codex-{}.md", std::process::id()));
    let _ = std::fs::remove_file(&out_path);

    let mut cmd = Command::new(&bin);
    crate::platform::quiet(&mut cmd);
    cmd.arg("exec")
        .arg("--sandbox")
        .arg("read-only")
        .arg("--ephemeral")
        .arg("--skip-git-repo-check")
        .arg("--color")
        .arg("never")
        .arg("-c")
        .arg("mcp_servers={}")
        .arg("-c")
        .arg("notify=[]")
        .arg("-c")
        .arg("model_reasoning_effort=\"low\"")
        .arg("-o")
        .arg(&out_path)
        .arg("-");
    if let Some(dir) = cwd {
        cmd.arg("-C").arg(dir);
    }
    // stderr는 **파일로** 받는다 — 파이프로 열고 종료 후에 읽으면 codex가
    // 진행 로그로 버퍼를 채우는 순간 `write`에서 멈춘다(실측: 9분간 CPU 0.97초,
    // 스택이 `print_config_summary` → `write`에 고정). planner.rs와 같은 규약.
    let err_file = crate::planner::TempCapture::new("codex-err")?;
    cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(err_file.stdio()?);
    let mut child = cmd.spawn().map_err(|e| anyhow!("failed to spawn {}: {e}", bin.display()))?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("codex stdin unavailable"))?;
        stdin.write_all(prompt.as_bytes())?;
        // drop → EOF
    }

    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait()? {
            Some(st) => break st,
            None => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_file(&out_path);
                    return Err(anyhow!("codex timed out after {:?}", timeout));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };
    let text = std::fs::read_to_string(&out_path).unwrap_or_default();
    let _ = std::fs::remove_file(&out_path);
    if !status.success() {
        let stderr_text = err_file.read();
        let tail: String = stderr_text.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        return Err(anyhow!("codex exited with code {:?}: {}", status.code(), if tail.trim().is_empty() { "<no stderr>" } else { tail.trim() }));
    }
    let out = text.trim().to_string();
    if out.is_empty() {
        return Err(anyhow!("codex가 빈 응답을 반환"));
    }
    Ok(out)
}

/// P3-15: is `model` already resident in Ollama's memory? Cheap (~ms) probe of
/// `/api/ps` so the frontend can label a cold first run "모델 깨우는 중" instead
/// of a silent 40s wait. Any error (server down / refused) → false (treat as
/// cold; the generate call will surface the real error if it's truly down).
pub fn ollama_model_loaded(model: &str) -> bool {
    let Ok(resp) = ureq::get(OLLAMA_PS_URL).timeout(Duration::from_secs(2)).call() else {
        return false;
    };
    let Ok(v) = resp.into_json::<serde_json::Value>() else {
        return false;
    };
    v.get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter().any(|m| {
                m.get("model").and_then(|x| x.as_str()) == Some(model)
                    || m.get("name").and_then(|x| x.as_str()) == Some(model)
            })
        })
        .unwrap_or(false)
}

fn ollama_generate(prompt: &str, model: &str) -> Result<String> {
    let resp = ureq::post(OLLAMA_URL).timeout(OLLAMA_TIMEOUT).send_json(serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        // Coaching is phrasing-of-facts, not creative writing — low temp
        // buys instruction adherence (exact section headers, no invented
        // numbers) from mid-size local models.
        "options": { "temperature": 0.3 },
    }));
    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(404, _)) => {
            return Err(anyhow!(
                "ollama 모델 '{}' 없음 — `ollama pull {}` 필요",
                model, model
            ));
        }
        Err(ureq::Error::Status(code, r)) => {
            return Err(anyhow!("ollama http {}: {}", code, r.into_string().unwrap_or_default()));
        }
        Err(e) => {
            // Connection refused → server not running.
            return Err(anyhow!(
                "ollama 연결 실패 ({OLLAMA_URL}) — 켜져 있나요? `brew services start ollama`: {e}"
            ));
        }
    };
    let v: serde_json::Value = resp.into_json()?;
    let text = v
        .get("response")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("ollama 응답에 `response` 필드 없음"))?;
    let out = strip_think(text).trim().to_string();
    if out.is_empty() {
        return Err(anyhow!("ollama가 빈 응답을 반환"));
    }
    Ok(out)
}

/// Reasoning models (deepseek-r1 등) wrap chain-of-thought in
/// `<think>…</think>`. Coaching wants only the final prose, so drop it.
/// EXAONE isn't a reasoning model so this is a no-op there, but keeping it
/// makes the backend model-agnostic.
fn strip_think(s: &str) -> String {
    match (s.find("<think>"), s.find("</think>")) {
        (Some(a), Some(b)) if b > a => {
            let mut out = String::with_capacity(s.len());
            out.push_str(&s[..a]);
            out.push_str(&s[b + "</think>".len()..]);
            out
        }
        _ => s.to_string(),
    }
}

/// Extract the `result` text from `claude -p --output-format json`'s
/// envelope. (Moved out of retro.rs so both backends return plain prose.)
pub(crate) fn parse_claude_envelope(raw: &str) -> Result<String> {
    let env: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| anyhow!("non-JSON envelope from claude: {}", e))?;
    if env.get("subtype").and_then(|v| v.as_str()) != Some("success") {
        return Err(anyhow!(
            "claude returned non-success: {}",
            env.get("result").and_then(|v| v.as_str()).unwrap_or("<no result text>")
        ));
    }
    env.get("result")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .ok_or_else(|| anyhow!("envelope missing string `result`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_think_removes_block() {
        assert_eq!(strip_think("<think>reasoning</think>답"), "답");
        assert_eq!(strip_think("답 없는 think"), "답 없는 think");
        assert_eq!(strip_think("<think>only open 답"), "<think>only open 답");
    }

    #[test]
    fn from_settings_explicit_values() {
        match Backend::from_settings("ollama", "", None) {
            Backend::Ollama { model } => assert_eq!(model, DEFAULT_OLLAMA_MODEL),
            _ => panic!("expected ollama"),
        }
        match Backend::from_settings("claude", "", Some("codex")) {
            Backend::ClaudeP { model } => assert_eq!(model, DEFAULT_CLAUDE_MODEL),
            _ => panic!("explicit claude must ignore recent agent"),
        }
        assert!(matches!(Backend::from_settings("codex", "", Some("claude")), Backend::CodexExec));
    }

    /// v5 M5: 기본값 `auto`는 최근 활동 에이전트를 따른다 — Ollama로 떨어지지 않는다.
    #[test]
    fn auto_follows_recent_agent_never_ollama() {
        assert!(matches!(Backend::from_settings("auto", "", Some("codex")), Backend::CodexExec));
        assert!(matches!(Backend::from_settings("auto", "", Some("claude")), Backend::ClaudeP { .. }));
        // 활동 기록이 없어도 Ollama가 아닌 에이전트 CLI 중 하나로.
        assert!(!matches!(Backend::from_settings("auto", "", None), Backend::Ollama { .. }));
        assert!(!matches!(Backend::from_settings("", "", None), Backend::Ollama { .. }));
    }
}
