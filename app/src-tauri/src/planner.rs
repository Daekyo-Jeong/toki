//! Planner subprocess wrapper — calls `claude -p` for one-shot planning
//! prompts. MVP-12a foundation: locate the binary, run with a hard
//! timeout, enforce single-flight via a static mutex, return raw stdout.
//!
//! Why subprocess and not API: the user pays for Claude Code already;
//! `claude` runs under their existing OAuth/keychain. Zero added cost,
//! and we don't have to handle API keys.
//!
//! Quota note: each call burns the user's own 5h block. **No automatic
//! callers** — this must only fire on explicit user action.
//!
//! Output: `--output-format json` returns a wrapper like
//!   { "type":"result", "subtype":"success", "result":"<text>", ... }
//! 12a returns the raw stdout as-is; 12b will parse the wrapper.

use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(60);
/// 딥 코칭(V5) — 큰 데이터 블록을 sonnet이 읽고 추론하는 호출은 60초를 훌쩍
/// 넘긴다(1차 27KB 실측 122s; 2차부터 30일 히스토리+engram ≈ 200KB로 커져
/// 여유를 더 둔다). 그 이상은 사용자 체감상 실패로 취급하는 게 낫다.
pub const DEEP_TIMEOUT: Duration = Duration::from_secs(480);

/// Single-flight gate. Tauri commands call `try_lock` — if another
/// planning call is in flight we return immediately with an error so the
/// UI can show "이미 계획 중" rather than blocking.
static IN_FLIGHT: Mutex<()> = Mutex::new(());

/// Resolve the `claude` binary path. Mirror of `usage_tracker.rs`'s
/// fallback chain:
///   1. `which claude` from PATH
///   2. Known install locations (`~/.local/bin`, `/usr/local/bin`,
///      Homebrew prefix on Apple Silicon and Intel).
///
/// On macOS, Tauri apps launched from Finder do NOT inherit the login
/// shell's PATH, so step 2 is what actually catches most users.
pub fn resolve_claude_bin() -> Option<PathBuf> {
    // 1. `which`
    if let Ok(out) = Command::new("which").arg("claude").output() {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() && std::path::Path::new(&p).exists() {
                return Some(PathBuf::from(p));
            }
        }
    }

    // 2. Common install paths.
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local").join("bin").join("claude"));
        candidates.push(home.join(".claude").join("local").join("claude"));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin/claude"));
    candidates.push(PathBuf::from("/usr/local/bin/claude"));
    candidates.push(PathBuf::from("/usr/bin/claude"));

    candidates.into_iter().find(|p| p.exists())
}

/// Whether the planner is currently usable (binary discoverable + no
/// in-flight call). UI can query this for a "Plan" button enabled state.
pub fn status() -> PlannerStatus {
    let bin = resolve_claude_bin();
    let busy = IN_FLIGHT.try_lock().is_err();
    PlannerStatus {
        available: bin.is_some(),
        bin_path: bin.map(|p| p.to_string_lossy().to_string()),
        busy,
    }
}

#[derive(Debug, serde::Serialize)]
pub struct PlannerStatus {
    pub available: bool,
    pub bin_path: Option<String>,
    pub busy: bool,
}

/// One-shot planning call. Returns the raw stdout (a `--output-format
/// json` wrapper). Caller is responsible for extracting `result`.
///
/// - `prompt`: the user/system prompt content
/// - `schema`: optional JSON Schema string; when provided, claude is
///   asked to emit structured output validated against it.
/// - `model`: alias like "haiku"/"sonnet" or full id; defaults to
///   "haiku" for quota friendliness.
/// - `target_dir`: working directory. claude auto-discovers a CLAUDE.md
///   in that folder, so the plan inherits the project's house style.
///   When `None`, we use the app's cwd which is rarely what the user
///   wants — UI should always pass an explicit value.
///
/// We intentionally do NOT pass `--bare`: that flag forces auth via
/// `ANTHROPIC_API_KEY` and ignores the user's OAuth keychain, causing
/// "Not logged in" errors for everyone who logged in via Claude Code.
pub fn call(
    prompt: &str,
    schema: Option<&str>,
    model: Option<&str>,
    target_dir: Option<&std::path::Path>,
) -> Result<String> {
    call_with_timeout(prompt, schema, model, target_dir, TIMEOUT)
}

/// Same as `call` but with an explicit timeout — the deep-coaching path needs
/// minutes, not the planner's 60s.
pub fn call_with_timeout(
    prompt: &str,
    schema: Option<&str>,
    model: Option<&str>,
    target_dir: Option<&std::path::Path>,
    timeout: Duration,
) -> Result<String> {
    let _guard = IN_FLIGHT
        .try_lock()
        .map_err(|_| anyhow!("planner busy: another call is in flight"))?;

    let bin = resolve_claude_bin().ok_or_else(|| {
        anyhow!(
            "claude CLI not found. Install Claude Code or set a path in Settings"
        )
    })?;

    let mut cmd = Command::new(&bin);
    cmd.arg("-p")
        .arg("--model")
        .arg(model.unwrap_or("haiku"))
        .arg("--output-format")
        .arg("json");

    if let Some(s) = schema {
        cmd.arg("--json-schema").arg(s);
    }

    if let Some(dir) = target_dir {
        cmd.current_dir(dir);
    }

    cmd.arg(prompt);
    // **파이프 대신 임시 파일** — 자식의 stdout/stderr를 파이프로 열어두고
    // 종료 후에 읽으면, 자식이 파이프 버퍼(macOS 16~64KB)를 채우는 순간
    // `write`에서 블록돼 **영원히 안 끝난다**. 우리는 종료를 기다리고 자식은
    // 우리가 읽기를 기다리는 교착이다(2026-09-02 codex 경로에서 실제로 물렸다.
    // 9분간 CPU 0.97초, 스택이 write에 멈춰 있었다). 파일은 버퍼 한계가 없다.
    let out_file = TempCapture::new("claude-out")?;
    let err_file = TempCapture::new("claude-err")?;
    cmd.stdout(out_file.stdio()?).stderr(err_file.stdio()?);

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn {}", bin.display()))?;

    // Manual timeout — matches the pattern in usage_tracker.rs. tokio
    // is intentionally not used to avoid pulling async into this stack.
    let start = Instant::now();
    loop {
        match child.try_wait()? {
            Some(status) => {
                let out = out_file.read();
                if !status.success() {
                    let stderr_text = err_file.read().trim().to_string();
                    return Err(anyhow!(
                        "claude exited with code {:?}: {}",
                        status.code(),
                        if stderr_text.is_empty() {
                            "<no stderr>".to_string()
                        } else {
                            stderr_text
                        }
                    ));
                }
                return Ok(out);
            }
            None => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(anyhow!("claude timed out after {:?}", timeout));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// 자식 프로세스 출력을 받는 임시 파일. 파이프와 달리 버퍼 한계가 없어
/// **자식이 write에서 막히지 않는다** — 긴 실행에서 파이프를 안 읽고 기다리면
/// 반드시 교착이 난다. Drop에서 파일을 지운다.
pub(crate) struct TempCapture {
    path: PathBuf,
}

impl TempCapture {
    pub fn new(tag: &str) -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "toki-{tag}-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::File::create(&path).with_context(|| format!("create {}", path.display()))?;
        Ok(Self { path })
    }

    /// 자식에게 넘길 쓰기 핸들 (append 모드 — 여러 번 열려도 서로 안 덮는다).
    pub fn stdio(&self) -> Result<Stdio> {
        let f = std::fs::OpenOptions::new().append(true).open(&self.path)?;
        Ok(Stdio::from(f))
    }

    pub fn read(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap_or_default()
    }
}

impl Drop for TempCapture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-02 회귀 — 자식 출력을 파이프로 열고 **종료 후에** 읽는 구조는
    /// 파이프 버퍼(macOS 16~64KB)가 차는 순간 교착이다: 우리는 종료를 기다리고
    /// 자식은 우리가 읽기를 기다린다. codex 코칭이 이걸로 9분간 멈춰 있었다
    /// (CPU 0.97초, 스택이 `write`에 고정). 200KB는 어떤 파이프 버퍼보다 크므로
    /// 이 테스트는 파이프 구현에서 **반드시 걸린다**.
    #[test]
    fn child_output_larger_than_a_pipe_buffer_does_not_deadlock() {
        const BYTES: usize = 200 * 1024;
        let cap = TempCapture::new("deadlock-test").unwrap();
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(format!("head -c {BYTES} /dev/zero | tr '\\0' 'x' >&2; exit 3"))
            .stdout(Stdio::null())
            .stderr(cap.stdio().unwrap())
            .spawn()
            .unwrap();

        // 프로덕션 코드와 같은 모양의 대기 루프(읽기는 종료 후에).
        let start = Instant::now();
        let status = loop {
            match child.try_wait().unwrap() {
                Some(st) => break st,
                None => {
                    assert!(start.elapsed() < Duration::from_secs(20), "자식이 출력에 막혔다 = 교착");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        assert_eq!(status.code(), Some(3));
        assert_eq!(cap.read().len(), BYTES);
    }

    /// TempCapture는 Drop에서 파일을 지운다 — 코칭을 반복해도 /tmp가 안 쌓인다.
    #[test]
    fn temp_capture_cleans_up() {
        let path = {
            let cap = TempCapture::new("cleanup").unwrap();
            assert!(cap.path.exists());
            cap.path.clone()
        };
        assert!(!path.exists());
    }
}
