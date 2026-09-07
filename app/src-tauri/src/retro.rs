//! MVP-12 (pivot, v2) — Self-learning coaching retrospective.
//!
//! Earlier this summarized hook breadcrumbs (tool/file counts). That can
//! only describe *what* happened, never *how* the user drove the session,
//! because hook events don't contain the user's prompts.
//!
//! This version reads the full session transcript
//! (`~/.claude/projects/*/<session_id>.jsonl`) and distills it into an
//! objective trace — the user's verbatim prompts plus concrete friction
//! signals (errors, interrupts, re-read-after-edit, repeated bash). Those
//! facts are fed to `claude -p` (sonnet by default) with a Socratic
//! coaching prompt that is forbidden from generic advice and constrained
//! to what the *user* controls (how they framed requests, when they
//! intervened) — not what the agent did mechanically.
//!
//! Result is cached on `dungeons.retrospective`.

use crate::coach;
use crate::db::Db;
use anyhow::{anyhow, Result};
use chrono::{DateTime, Datelike, Local, Timelike, Utc};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

const MAX_PROMPTS: usize = 50; // verbatim user prompts to include
const MAX_PROMPT_CHARS: usize = 300;

/// Distilled, objective view of one session — the input to the coach.
#[derive(Default)]
struct SessionTrace {
    prompts: Vec<String>,
    n_turns: u32,
    thinking: u32,
    errors: u32,
    interrupts: u32,
    first_ts: Option<String>,
    last_ts: Option<String>,
    tool_counts: Vec<(String, u32)>,     // sorted desc
    top_edits: Vec<(String, u32)>,       // basename, count
    top_reads: Vec<(String, u32)>,
    reread_after_edit: Vec<(String, u32)>,
    /// V4-13 (B): net edit growth per file (Σ len(new) − len(old), chars).
    /// LENGTHS ONLY — edit content never stored. dump-before-DESIGN (2026-07-14)
    /// found EVERY recurring-edit file on real data shows large +net (healthy
    /// iteration) — NO thrash (flat/negative) file exists → B deferred. Retained
    /// as the substrate + `dump_edit_growth` diagnostic to re-check any week.
    #[allow(dead_code)]
    edit_growth: Vec<(String, i64)>, // basename, net growth
    repeated_bash: Vec<(String, u32)>,   // normalized family, count (>=3 within session) — 세션 회고 데이터
    /// V4-12 ⑦: every distinct normalized bash family seen ≥1× this session
    /// (low-value filtered). Cross-session recurrence of these = ritual → skill
    /// candidate, regardless of within-session count (git push is 1×/session).
    bash_families: Vec<String>,
    /// §3.3.2 B 3차 (2026-08-18): file-aware retry cycles (CodeBurn proxy) —
    /// a retry = the same file re-edited with ≥1 Bash since its previous edit
    /// (Edit A → Bash → Edit A). Paths only, living docs excluded (§3.3 가드
    /// 승계). Wired to NO card until `dump_retry_categories` shows thrash
    /// separating from iteration on real data (dump-before-wire).
    #[allow(dead_code)]
    retry_files: Vec<(String, u32, u32, u32, u32)>, // full path, edits, retries, reworks, max chain
    #[allow(dead_code)]
    edit_cycles: u32, // total edit events on non-doc files
    #[allow(dead_code)]
    retries: u32, // total retry cycles (one-shot rate = 1 − retries/edit_cycles)
    /// §3.3.2 B 4차 (2026-08-20): **영역 단위** rework — 이번 편집의 `old_string`이
    /// 이 세션에서 내가 쓴 `new_string`을 다시 먹었나. 파일 단위 retry는 같은
    /// 파일의 *다른* 영역을 넓혀가는 정상 작업과 겉돎을 구분하지 못해(1차 반복
    /// 편집이 죽은 바로 그 병) 축을 한 단계 내린 것. 내용은 저장하지 않는다.
    #[allow(dead_code)]
    reworks: u32,
    /// §3.3.4: deterministic tool-pattern category per assistant line (keyword
    /// axis deferred — Korean prompts). Denominator-first: normalizes friction
    /// per work kind (Debugging의 왕복=정상, Coding의 왕복=신호) before any
    /// card may use it.
    #[allow(dead_code)]
    category_counts: Vec<(&'static str, u32)>,
}

/// 4번 정체성("레벨이 오르는 만큼 AI 능력 증진")의 계량 — 레벨은 누적 토큰,
/// 즉 **소비**를 재므로 실력이 늘었는지는 못 말한다(오히려 겉돌수록 토큰을 더
/// 써서 레벨이 빨리 오른다). 이 지표는 **같은 사람의 주 대비 변화**만 보여준다:
/// 절대값 등급을 매기면 작업 종류에 confound돼 §3.3.1에서 신호 4종을 죽인 병을
/// 반복한다(디버깅 주간은 원래 왕복이 많다). 카테고리 분모(§3.3.4)가 서기 전까진
/// 추세만.
#[derive(serde::Serialize, Default, Clone)]
pub struct SkillWindow {
    pub sessions: usize,
    pub edits: u32,
    pub reworks: u32,
    pub one_shot: f64,   // 0~100 — 고친 게 한 번에 붙은 비율
    pub prompts: usize,
    pub short_prompts: usize,
    pub delegation: f64, // 0~100 — 방향 없이 넘긴 비율
    pub max_chain: u32,
}

fn skill_window(paths: &[PathBuf], since: DateTime<Utc>, until: Option<DateTime<Utc>>) -> SkillWindow {
    let mut w = SkillWindow::default();
    for p in paths {
        let Ok(t) = distill_window(p, Some(since), until) else { continue };
        // 사람 프롬프트가 0이면 봇 세션(engram 자동캡처·claude-gate 등) — 세션
        // 수와 프롬프트 분모를 오염시키므로 제외한다.
        if t.prompts.is_empty() {
            continue;
        }
        w.sessions += 1;
        w.edits += t.edit_cycles;
        w.reworks += t.reworks;
        w.prompts += t.prompts.len();
        w.short_prompts += t.prompts.iter().filter(|p| is_continuation(p)).count();
        w.max_chain = w.max_chain.max(t.retry_files.iter().map(|(_, _, _, _, c)| *c).max().unwrap_or(0));
    }
    w.one_shot = if w.edits == 0 { 100.0 } else { 100.0 * (w.edits - w.reworks) as f64 / w.edits as f64 };
    w.delegation = if w.prompts == 0 { 0.0 } else { 100.0 * w.short_prompts as f64 / w.prompts as f64 };
    w
}

#[derive(serde::Serialize, Clone)]
pub struct SkillTrend {
    pub now: SkillWindow,
    pub prev: SkillWindow,
    pub computed_at: String,
}

pub type SharedSkillTrend = Arc<std::sync::Mutex<Option<SkillTrend>>>;

/// 실측 11.6초(트랜스크립트 수백 개 전량 파싱) — 화면 진입 때 계산하면 못 쓴다.
/// 앰비언트 앱이니 백그라운드에서 30분마다 미리 계산해두고 UI는 읽기만 한다.
/// 부팅 직후는 20초 늦춰 시작(런치 경쟁 회피).
pub fn spawn_skill_trend(cache: SharedSkillTrend) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(20));
        let (now, prev) = skill_trend();
        eprintln!(
            "[skill] one-shot {:.1}% (지난주 {:.1}%) · 편집 {} · 되돌림 {}",
            now.one_shot, prev.one_shot, now.edits, now.reworks
        );
        if let Ok(mut g) = cache.lock() {
            *g = Some(SkillTrend { now, prev, computed_at: Local::now().to_rfc3339() });
        }
        std::thread::sleep(std::time::Duration::from_secs(30 * 60));
    });
}

/// (이번 주, 지난주). 지난주는 닫힌 구간이라 이번 주와 겹치지 않는다.
pub fn skill_trend() -> (SkillWindow, SkillWindow) {
    let now = Utc::now();
    let wk = chrono::Duration::days(7);
    let paths = recent_transcripts(14, 5, 200);
    (
        skill_window(&paths, now - wk, None),
        skill_window(&paths, now - wk * 2, Some(now - wk)),
    )
}

/// Locate a session's transcript by id. We glob rather than reconstruct
/// the path from cwd because Claude Code's project-dir encoding is lossy
/// (slashes and dots both collapse to dashes). The session id is unique,
/// so scanning each project subdir for `<id>.jsonl` is reliable — and it
/// also finds sessions that predate hook install.
fn find_transcript(session_id: &str) -> Option<PathBuf> {
    let base = dirs::home_dir()?.join(".claude").join("projects");
    let rd = std::fs::read_dir(&base).ok()?;
    let target = format!("{}.jsonl", session_id);
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            let candidate = p.join(&target);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Cheap existence check for the UI — does this session have a transcript
/// we could coach on? Used to enable/disable the RETRO button (the old
/// `event_count > 0` gate was wrong: retro reads the transcript now, and
/// `dungeon_events` is sparse — most real sessions have zero).
pub fn has_transcript(session_id: &str) -> bool {
    find_transcript(session_id).is_some()
}

fn basename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Navigation/inspection one-liners (`cd`, `ls`, …) recur in every session
/// without saying anything about the user's habits — they only drown the
/// real repeated-command signal (V4-6 tuning note). Compound commands
/// (`cd app && npm run build`) still count.
fn is_low_value_bash(cmd: &str) -> bool {
    let t = cmd.trim();
    if t.contains("&&") || t.contains('|') || t.contains(';') {
        return false;
    }
    matches!(
        t.split_whitespace().next().unwrap_or(""),
        "cd" | "ls" | "pwd" | "clear" | "echo" | "cat" | "which" | "true"
    )
}

/// V4-12 (⑦ 스킬 후보): fold a bash command into a "ritual family" key so the
/// same ritual counts across sessions instead of splitting on the session-
/// specific paths/numbers baked into each invocation (the exact problem the
/// proposal names — `tail /private/tmp/<run-a>/x` vs `.../<run-b>/y` are one
/// ritual but split under the old first-60-chars key). GENERAL, not command-
/// specific: quoted strings → `<str>`, path-like tokens → `<path>`,
/// pure/attached numbers → `<n>`, long hex → `<hash>`; pipe/redirect operators
/// kept verbatim (a piped ritual stays distinct from the bare command).
fn normalize_command(cmd: &str) -> String {
    let dequoted = strip_quoted(cmd);
    let normalized: Vec<String> = dequoted.split_whitespace().map(normalize_token).collect();
    normalized.join(" ").chars().take(80).collect()
}

/// Replace the contents of `"…"`/`'…'` spans with `<str>` so commit messages /
/// echo text don't fragment a family by their words. Unmatched quote → the
/// rest of the string becomes one `<str>`.
fn strip_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '"' || c == '\'' {
            out.push_str("<str>");
            for d in chars.by_ref() {
                if d == c {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn normalize_token(tok: &str) -> String {
    // Shell operators / redirections — keep verbatim (structure is signal).
    if matches!(tok, "|" | "&&" | "||" | ";" | "&")
        || tok.contains(">&")
        || tok.starts_with('>')
        || tok.starts_with('<')
    {
        return tok.to_string();
    }
    // Path-like — the main source of per-session fragmentation.
    if tok.contains('/') || tok.starts_with('~') {
        return "<path>".to_string();
    }
    // Flag carrying a number (`-30`, `-n30`, `--max=100`) → collapse the digits.
    if tok.starts_with('-') && tok.chars().any(|c| c.is_ascii_digit()) {
        return collapse_digits(tok);
    }
    // Bare number.
    if !tok.is_empty() && tok.chars().all(|c| c.is_ascii_digit()) {
        return "<n>".to_string();
    }
    // Long hex (sha/uuid chunk) — must contain a digit so plain words survive.
    if tok.len() >= 8
        && tok.chars().all(|c| c.is_ascii_hexdigit())
        && tok.chars().any(|c| c.is_ascii_digit())
    {
        return "<hash>".to_string();
    }
    tok.to_string()
}

/// Collapse each run of digits in a token to a single `<n>` (`-n30` → `-n<n>`).
fn collapse_digits(tok: &str) -> String {
    let mut out = String::new();
    let mut in_digits = false;
    for c in tok.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                out.push_str("<n>");
                in_digits = true;
            }
        } else {
            out.push(c);
            in_digits = false;
        }
    }
    out
}

/// Parse the JSONL transcript line-by-line into a SessionTrace.
/// §3.3.4: coarse Bash intent for the category 1-pass. Substring heuristics
/// on the raw command (compound one-liners hit the first matching kind by
/// priority: test > git > build). "build" also matches rebuild/tauri build —
/// acceptable for a denominator, documented.
fn bash_kind(cmd: &str) -> (bool, bool, bool) {
    let c = cmd.to_lowercase();
    let test = ["cargo test", "npm test", "npm run test", "pytest", "vitest", "jest", "go test"]
        .iter().any(|p| c.contains(p));
    let git = ["git push", "git commit", "git merge", "gh pr"].iter().any(|p| c.contains(p));
    let build = ["build", "docker", "pm2 "].iter().any(|p| c.contains(p));
    (test, git, build)
}

/// §3.3.4: per-assistant-line tool facts, collected during the block loop and
/// classified once per line.
#[derive(Default)]
struct LineFacts {
    agent: bool,
    plan: bool,
    edit: bool,
    readonly: bool,
    bash_test: bool,
    bash_git: bool,
    bash_build: bool,
    any_tool: bool,
    text: bool,
}

/// §3.3.4 category 1-pass — tool patterns only, priority order fixed by spec.
/// `err_pending` = a tool_result error arrived and no TOOL line consumed it
/// yet. Text-only lines don't consume the flag: real transcripts interleave
/// an analysis text line between error and the fixing Edit (error→text→Edit
/// must still classify as debugging — the first dump lost it, 3/8199 lines).
/// Returns None for lines with no signal (pure thinking) so they don't
/// dilute the shares.
fn turn_category(lf: &LineFacts, err_pending: bool) -> Option<&'static str> {
    Some(if lf.agent {
        "delegation"
    } else if lf.plan {
        "planning"
    } else if lf.bash_test {
        "testing"
    } else if lf.bash_git {
        "gitops"
    } else if lf.bash_build {
        "build"
    } else if err_pending && lf.edit {
        "debugging"
    } else if lf.edit {
        "coding"
    } else if lf.readonly {
        "exploration"
    } else if lf.any_tool {
        "general"
    } else if lf.text {
        "conversation"
    } else {
        return None;
    })
}

fn distill(path: &PathBuf) -> Result<SessionTrace> {
    distill_since(path, None)
}

/// 이벤트 단위 시간 창 (2026-08-20). `since`를 주면 그보다 오래된 **줄을 통째로
/// 건너뛴다** — 세션을 통째로 넣거나 빼는 게 아니라, 마라톤 세션이 "이번 주 몫만"
/// 기여하게 만든다. 이게 없으면 크로스 코칭이 딜레마에 빠진다: 시작 시각으로
/// 거르면 오래 켜둔 세션의 이번 주 실작업이 통째로 안 보이고(실측: 7일 활동
/// 세션 36개 중 11개 제외 → 덤프 edits 0), 활동 시각으로 거르면 몇 달 전
/// 이벤트가 "이번 주"로 집계돼 §3.3이 막으려던 '지난 분기 평균'이 부활한다.
fn distill_since(path: &PathBuf, since: Option<DateTime<Utc>>) -> Result<SessionTrace> {
    distill_window(path, since, None)
}

/// `until`까지 주면 **닫힌 구간**이 된다 — "지난주"처럼 과거 한 조각만 떼어
/// 볼 때 쓴다(실력 추세의 비교군).
fn distill_window(
    path: &PathBuf,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> Result<SessionTrace> {
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);

    let mut t = SessionTrace::default();
    let mut reads: HashMap<String, u32> = HashMap::new();
    let mut edits: HashMap<String, u32> = HashMap::new();
    let mut reread: HashMap<String, u32> = HashMap::new();
    let mut tool_counts: HashMap<String, u32> = HashMap::new();
    let mut bash_counts: HashMap<String, u32> = HashMap::new();
    let mut growth: HashMap<String, i64> = HashMap::new(); // basename → Σ(len new − len old)
    let mut last_edit: Option<String> = None;
    // §3.3.2 B 3차: retry cycles. basename → (edits, retries) on non-living-doc
    // files; `last_edit_bash` remembers the Bash sequence number at each file's
    // previous edit — a later edit with a higher seq means an edit→verify→
    // re-edit loop happened.
    // 키는 **full path**. basename은 같은 이름의 다른 파일을 한 줄로 뭉쳐
    // (실측: 한 프로젝트에 route.ts 16개·page.tsx 11개) 서로 다른 파일 사이의
    // 이동을 retry로 오집계했다.
    let mut retry_map: HashMap<String, (u32, u32, u32, u32)> = HashMap::new();
    let mut last_edit_bash: HashMap<String, u32> = HashMap::new();
    let mut verify_seq: u32 = 0;
    // B 4차: path → 이 세션에서 내가 쓴 new_string(최근 12개). 겹침 판정에만
    // 쓰고 트레이스에 저장하지 않는다(내용 비보존 원칙).
    let mut mine: HashMap<String, Vec<String>> = HashMap::new();
    let mut chain: HashMap<String, u32> = HashMap::new();
    // §3.3.4: category tallies + cross-line error flag for `debugging`.
    let mut cat_counts: HashMap<&'static str, u32> = HashMap::new();
    let mut error_pending = false;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let typ = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if let Some(ts) = v.get("timestamp").and_then(|x| x.as_str()) {
            // 창 밖 이벤트는 세지 않는다. first_ts도 **창 안 첫 이벤트**가 되므로
            // span/추세가 "이번 주"를 가리킨다.
            let parsed = DateTime::parse_from_rfc3339(ts).ok().map(|d| d.with_timezone(&Utc));
            if let (Some(cut), Some(at)) = (since, parsed) {
                if at < cut {
                    continue;
                }
            }
            if let (Some(end), Some(at)) = (until, parsed) {
                if at >= end {
                    continue;
                }
            }
            if t.first_ts.is_none() {
                t.first_ts = Some(ts.to_string());
            }
            t.last_ts = Some(ts.to_string());
        }
        if typ == "assistant" {
            t.n_turns += 1;
        }

        let msg = v.get("message");

        // User prompts (the intent thread).
        // ⚠️ **사람이 친 것만** 센다. 이 사용자는 engram 자동 캡처·claude-gate가
        // Claude Code CLI를 통해 도니 `~/.claude/projects`에 봇 세션이 쌓인다 —
        // 실측(최근 14일): 141세션 중 **115개(81%)가 사람 프롬프트 0**이고 편집도
        // 0인데, 세션 수·프롬프트 수·턴·카테고리에는 전부 들어가 통계를 오염시켰다
        // (예: "지난주 356 프롬프트에 위임 0%" — 분모가 봇 프롬프트였다).
        // 판별자는 CLI가 이미 남기는 `origin.kind == "human"` (2026-04 데이터까지
        // 소급 확인). SDK/훅발 호출엔 이 필드가 없다.
        let human = v
            .get("origin")
            .and_then(|o| o.get("kind"))
            .and_then(|k| k.as_str())
            == Some("human");
        if typ == "user" && human {
            if let Some(m) = msg {
                let content = m.get("content");
                let mut texts: Vec<String> = Vec::new();
                match content {
                    Some(serde_json::Value::String(s)) => texts.push(s.clone()),
                    Some(serde_json::Value::Array(arr)) => {
                        for b in arr {
                            if b.get("type").and_then(|x| x.as_str()) == Some("text") {
                                if let Some(s) = b.get("text").and_then(|x| x.as_str()) {
                                    texts.push(s.to_string());
                                }
                            }
                        }
                    }
                    _ => {}
                }
                for tx in texts {
                    let tx = tx.trim();
                    if tx.contains("[Request interrupted") {
                        t.interrupts += 1;
                    }
                    // Skip system caveats / tool-result echoes / xml-ish.
                    // Char-safe prefix: byte-slicing panics mid-codepoint on
                    // Korean/multibyte prompts (`tx[..30]` split a 고).
                    let head: String = tx.chars().take(30).collect();
                    let is_noise = tx.is_empty()
                        || tx.starts_with("Caveat")
                        || tx.starts_with('<')
                        || head.contains("tool_result");
                    if !is_noise {
                        t.prompts.push(tx.chars().take(MAX_PROMPT_CHARS).collect());
                    }
                }
            }
        }

        // Content blocks of any message: tool_use / tool_result / thinking.
        let mut lf = LineFacts::default();
        if let Some(arr) = msg.and_then(|m| m.get("content")).and_then(|c| c.as_array()) {
            for b in arr {
                let bt = b.get("type").and_then(|x| x.as_str()).unwrap_or("");
                match bt {
                    "thinking" => t.thinking += 1,
                    "text" => lf.text = true,
                    "tool_use" => {
                        let nm = b.get("name").and_then(|x| x.as_str()).unwrap_or("");
                        *tool_counts.entry(nm.to_string()).or_insert(0) += 1;
                        lf.any_tool = true;
                        match nm {
                            "Task" | "Agent" => lf.agent = true,
                            "EnterPlanMode" | "ExitPlanMode" | "exit_plan_mode" | "TodoWrite"
                            | "TaskCreate" | "TaskUpdate" => lf.plan = true,
                            "Edit" | "Write" => lf.edit = true,
                            "Read" | "Grep" | "Glob" | "LS" | "WebSearch" | "WebFetch" => {
                                lf.readonly = true
                            }
                            _ => {}
                        }
                        let input = b.get("input");
                        let fp = input
                            .and_then(|i| i.get("file_path"))
                            .and_then(|x| x.as_str());
                        let cmd = input
                            .and_then(|i| i.get("command"))
                            .and_then(|x| x.as_str());
                        if nm == "Read" {
                            if let Some(fp) = fp {
                                *reads.entry(fp.to_string()).or_insert(0) += 1;
                                if last_edit.as_deref() == Some(fp) {
                                    *reread.entry(basename(fp)).or_insert(0) += 1;
                                }
                            }
                        }
                        if nm == "Edit" || nm == "Write" {
                            if let Some(fp) = fp {
                                *edits.entry(fp.to_string()).or_insert(0) += 1;
                                last_edit = Some(fp.to_string());
                                // V4-13 (B): net growth = len(new) − len(old),
                                // chars. Write has `content` (old=0); Edit has
                                // old_string/new_string. Lengths only, never content.
                                let clen = |k: &str| -> i64 {
                                    input
                                        .and_then(|i| i.get(k))
                                        .and_then(|x| x.as_str())
                                        .map(|s| s.chars().count() as i64)
                                        .unwrap_or(0)
                                };
                                let new_len = if nm == "Write" { clen("content") } else { clen("new_string") };
                                let old_len = clen("old_string");
                                *growth.entry(basename(fp)).or_insert(0) += new_len - old_len;
                                // §3.3.2 B 3차: retry cycle — this file was
                                // edited before AND ≥1 Bash ran since then.
                                if !is_doc_file(&basename(fp)) {
                                    let key = fp.to_string();
                                    let e = retry_map.entry(key.clone()).or_insert((0, 0, 0, 0));
                                    e.0 += 1;
                                    // 파일 단위(현행): 검증을 끼고 같은 파일로 돌아옴
                                    if last_edit_bash.get(&key).is_some_and(|prev| verify_seq > *prev) {
                                        e.1 += 1;
                                    }
                                    last_edit_bash.insert(key.clone(), verify_seq);
                                    // 영역 단위(B 4차): 내가 쓴 걸 다시 먹었나 + 연속 N회차
                                    let old_n = input
                                        .and_then(|i| i.get("old_string"))
                                        .and_then(|x| x.as_str())
                                        .map(norm_ws)
                                        .unwrap_or_default();
                                    let anchors = mine.entry(key.clone()).or_default();
                                    let hit = !old_n.is_empty()
                                        && anchors.iter().any(|prev| overlaps_own(&old_n, prev));
                                    if nm == "Edit" {
                                        let new_n = input
                                            .and_then(|i| i.get("new_string"))
                                            .and_then(|x| x.as_str())
                                            .map(norm_ws)
                                            .unwrap_or_default();
                                        if new_n.len() >= 12 && new_n.len() <= 4000 {
                                            anchors.push(new_n);
                                            if anchors.len() > 12 {
                                                anchors.remove(0);
                                            }
                                        }
                                    }
                                    if hit {
                                        e.2 += 1;
                                        let c = chain.entry(key).or_insert(0);
                                        *c += 1;
                                        e.3 = e.3.max(*c);
                                    } else {
                                        chain.insert(key, 0);
                                    }
                                }
                            }
                        }
                        if nm == "Bash" {
                            if let Some(cmd) = cmd {
                                if is_verify_bash(cmd) {
                                    verify_seq += 1;
                                }
                                // V4-12: key by the normalized ritual family so
                                // the same command counts across sessions despite
                                // per-run paths/numbers (was: raw first-60-chars).
                                let key = normalize_command(cmd);
                                *bash_counts.entry(key).or_insert(0) += 1;
                                let (bt, bg, bb) = bash_kind(cmd);
                                lf.bash_test |= bt;
                                lf.bash_git |= bg;
                                lf.bash_build |= bb;
                            }
                        }
                    }
                    "tool_result" => {
                        if b.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false) {
                            t.errors += 1;
                            error_pending = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        // §3.3.4: one category per assistant line (matches n_turns granularity).
        // The error flag is consumed by the next line that runs a TOOL — the
        // agent's reaction to the error — not by interleaved text lines.
        if typ == "assistant" {
            let err = if lf.any_tool {
                std::mem::take(&mut error_pending)
            } else {
                error_pending
            };
            if let Some(cat) = turn_category(&lf, err) {
                *cat_counts.entry(cat).or_insert(0) += 1;
            }
        }
    }

    // Sort + cap the aggregates.
    let sort_desc = |m: HashMap<String, u32>, n: usize| -> Vec<(String, u32)> {
        let mut v: Vec<(String, u32)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(n);
        v
    };
    t.tool_counts = sort_desc(tool_counts, 14);
    t.top_edits = sort_desc(edits, 6)
        .into_iter()
        .map(|(k, c)| (basename(&k), c))
        .collect();
    t.top_reads = sort_desc(reads, 6)
        .into_iter()
        .map(|(k, c)| (basename(&k), c))
        .collect();
    t.reread_after_edit = sort_desc(reread, 6);
    // V4-13 (B): net growth per file (i64, so negative rewrites survive). Sort
    // by |edit volume| via edits map isn't available here; keep all, cap 15.
    t.edit_growth = {
        let mut v: Vec<(String, i64)> = growth.into_iter().collect();
        v.sort_by(|a, b| b.1.abs().cmp(&a.1.abs()).then(a.0.cmp(&b.0)));
        v.truncate(15);
        v
    };
    // ⑦: distinct families seen ≥1× (low-value dropped) — the ritual-detection
    // set. Sorted by within-session count desc then name, capped.
    let mut families: Vec<(String, u32)> = bash_counts
        .iter()
        .filter(|(k, _)| !is_low_value_bash(k))
        .map(|(k, n)| (k.clone(), *n))
        .collect();
    families.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    families.truncate(40);
    t.bash_families = families.iter().map(|(k, _)| k.clone()).collect();
    // 세션 회고 데이터 블록용 — 한 세션 안에서 3회 이상 두드린 명령 계열.
    // 탐색·이동 계열(`sed`·`cat`·`grep`…)은 **에이전트의 파일 읽기 방식**이지
    // 사용자 습관이 아니므로 제외한다(⑦·구 ④와 같은 필터).
    t.repeated_bash = sort_desc(
        bash_counts
            .into_iter()
            .filter(|(k, n)| *n >= 3 && !is_low_value_bash(k) && !is_inspect_head(k))
            .collect(),
        6,
    );
    // §3.3.2 B 3차: per-file (edits, retries), retries desc; totals uncapped.
    t.edit_cycles = retry_map.values().map(|(e, _, _, _)| e).sum();
    t.retries = retry_map.values().map(|(_, r, _, _)| r).sum();
    t.reworks = retry_map.values().map(|(_, _, w, _)| w).sum();
    t.retry_files = {
        let mut v: Vec<(String, u32, u32, u32, u32)> = retry_map
            .into_iter()
            .map(|(k, (e, r, w, c))| (k, e, r, w, c))
            .collect();
        // rework(영역 단위) 우선 정렬 — 이게 B 4차의 판정 축이다.
        v.sort_by(|a, b| b.3.cmp(&a.3).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0)));
        v.truncate(20);
        v
    };
    // §3.3.4: category distribution over assistant lines.
    t.category_counts = {
        let mut v: Vec<(&'static str, u32)> = cat_counts.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    };

    if t.prompts.len() > MAX_PROMPTS {
        t.prompts.truncate(MAX_PROMPTS);
    }
    Ok(t)
}

/// Render the objective trace + the Socratic coaching instruction.
/// V4-14: 코칭 페르소나 — `active_character`(외형에서 고른 펫)에 따라 화자의
/// **어조만** 바꾼다. 헌법 불변: 그라운딩(수치 인용·섹션 구조·해요체)은 절대
/// 규칙이 항상 우선이고, 페르소나는 문장 표현에만 관여한다. 블록은 코드 소유
/// (외부화 템플릿과 별개) — instr 뒤·data 앞에 결정론으로 끼워지므로 사용자가
/// 템플릿을 편집해도 페르소나는 유지된다. id는 TamagotchiShell.tsx의
/// PET_VARIANTS와 짝. 동글이(dong)/미지정 = 기본 톤 = 블록 없음.
/// 페르소나 원장 — 프롬프트 주입(persona_block)과 UI 표시(coach_personas
/// 커맨드)가 공유하는 단일 소스. desc는 사람이 읽는 성격 설명, example은
/// EXAONE용 어투 예시(형용사보다 예시에 잘 반응 — 골든 샘플 때 검증).
/// 예시에 숫자 금지(예시 숫자가 출력에 누출된 전례 방지). example이 빈
/// 항목(동글이)은 기본 톤 = 프롬프트에 블록을 넣지 않는다.
pub struct PersonaInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub desc: &'static str,
    pub example: &'static str,
}

pub const PERSONAS: &[PersonaInfo] = &[
    // desc는 외형 화면에서 정확히 2줄로 보이도록 40~46자·두 문장으로 통일
    // (기준: 깡총이/네모). 프롬프트 주입에도 같은 문장이 쓰인다.
    PersonaInfo { id: "bunny", name: "깡총이", desc: "밝고 경쾌하게 말해요. 리듬감 있는 짧은 문장을 즐겨 써요.", example: "좋아요, 여기만 다듬으면 흐름이 훨씬 가벼워져요!" },
    PersonaInfo { id: "dong", name: "동글이", desc: "차분하고 다정한 기본 톤이에요. 꾸밈없이 또박또박 짚어요.", example: "" },
    PersonaInfo { id: "cat", name: "뾰족이", desc: "짧고 담백하게 끊어 말해요. 수식어 없이 핵심만 콕 집어요.", example: "짧게 짚을게요. 지시가 두루뭉술했어요." },
    PersonaInfo { id: "horn", name: "뿔이", desc: "직설적으로 말해요. 핵심을 찌르고 승부욕을 깨우는 질문을 던져요.", example: "솔직히 말할게요. 이 습관, 이대로 두실 건가요?" },
    PersonaInfo { id: "box", name: "네모", desc: "차분하고 체계적으로 말해요. 먼저·다음·마지막 순서로 짚어요.", example: "먼저 목표를 정하고, 다음에 한 단계씩 시켜요." },
    PersonaInfo { id: "sprout", name: "새싹이", desc: "성장의 관점으로 말해요. 변화를 자라나는 한 단계로 짚어요.", example: "이 부분은 아직 자라는 중이에요. 다음 세션에서 한 뼘 더 나아가요." },
    PersonaInfo { id: "hat", name: "모자이", desc: "위트 있는 능청을 한 마디 얹어요. 데이터 앞에선 진지해져요.", example: "모자를 고쳐 쓰고 말하자면, 같은 명령을 또 손으로 치고 계셨죠?" },
    PersonaInfo { id: "glasses", name: "안경이", desc: "분석가처럼 정밀하게 말해요. 수치를 문장의 중심에 두어요.", example: "데이터만 보면 결론은 하나예요." },
    PersonaInfo { id: "ghost", name: "유령이", desc: "담담하고 관조적으로 말해요. 짧게, 여운이 남게 마무리해요.", example: "흐름은 나쁘지 않았어요. 다만, 같은 자리를 맴돌았죠." },
    PersonaInfo { id: "crown", name: "왕관이", desc: "기품 있고 확신에 차 말해요. 명령이 아닌 권유로 조언해요.", example: "방향은 그대가 쥐고 있어요. 다음 한 수를 정해두시길 권해요." },
    PersonaInfo { id: "bear", name: "곰이", desc: "포근하고 느긋하게 말해요. 짚을 건 짚되 부드럽게 감싸요.", example: "천천히 봐도 괜찮아요. 다만 이 부분은 한번 짚고 가요." },
    PersonaInfo { id: "octo", name: "문어", desc: "유연하게 시각을 하나 더 얹어요. 같은 수치도 다른 각도로 봐요.", example: "이렇게 보면 반복이지만, 뒤집어 보면 인코딩할 기회예요." },
];

fn persona_block(pet_id: &str) -> Option<String> {
    let p = PERSONAS.iter().find(|p| p.id == pet_id)?;
    if p.example.is_empty() {
        return None; // 동글이: 기본 톤 (desc는 UI 표시용으로만 존재)
    }
    let (name, style) = (p.name, format!("{} (어투 예시: \"{}\")", p.desc, p.example));
    Some(format!(
        "# 화자 페르소나 (표현·어조에만 적용)\n당신(Toki)의 현재 모습은 '{name}'예요. {style}\n단, 페르소나는 문장 표현에만 적용해요 — 해요체 존댓말·섹션 구조·수치 인용 등 아래 지침의 절대 규칙이 항상 우선이에요. 어투 예시는 말투 참고용일 뿐 문장을 그대로 옮기지 않아요. 페르소나를 이유로 없는 사실·수치를 만들지 않아요."
    ))
}

fn build_prompt(t: &SessionTrace, persona: Option<&str>) -> String {
    let dur = match (&t.first_ts, &t.last_ts) {
        (Some(a), Some(b)) => {
            let a16: String = a.chars().take(16).collect();
            let bt: String = b.chars().skip(11).take(5).collect();
            format!("{} ~ {}", a16, bt)
        }
        _ => "(unknown)".to_string(),
    };

    let fmt_pairs = |v: &[(String, u32)]| -> String {
        v.iter()
            .map(|(k, c)| format!("{}×{}", k, c))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut data = String::new();
    data.push_str("# 세션 추적 데이터 (객관적 사실)\n");
    data.push_str(&format!(
        "기간: {} · assistant 턴 {} · thinking 블록 {} · 에러 결과 {} · 사용자 중단(interrupt) {}\n",
        dur, t.n_turns, t.thinking, t.errors, t.interrupts
    ));
    data.push_str(&format!("도구 사용: {}\n", fmt_pairs(&t.tool_counts)));
    data.push_str(&format!("가장 많이 편집한 파일: {}\n", fmt_pairs(&t.top_edits)));
    data.push_str(&format!("가장 많이 읽은 파일: {}\n", fmt_pairs(&t.top_reads)));
    if !t.reread_after_edit.is_empty() {
        data.push_str(&format!(
            "편집 직후 같은 파일 재읽기(에이전트 행동): {}\n",
            fmt_pairs(&t.reread_after_edit)
        ));
    }
    // ⑨ 겉도는 편집 — 파일 단위 "몇 번 고쳤나"가 아니라 **같은 자리를 되돌린**
    // 횟수다(spec §3.3.2 B 4차). 0이면 줄 자체를 넣지 않는다 — 없는 마찰을
    // 데이터로 제시하면 LLM이 그걸 소재로 지어낸다.
    if t.reworks > 0 {
        let worst: Vec<String> = t
            .retry_files
            .iter()
            .filter(|(_, _, _, w, _)| *w > 0)
            .take(3)
            .map(|(f, _, _, w, c)| {
                format!("{}(되돌림 {}회, 최다 연속 {}회)", basename(f), w, c)
            })
            .collect();
        data.push_str(&format!(
            "같은 자리를 되돌린 편집: 전체 편집 {}개 중 {}개 — {}\n",
            t.edit_cycles,
            t.reworks,
            worst.join(", ")
        ));
    }
    if !t.repeated_bash.is_empty() {
        data.push_str(&format!(
            "3회 이상 반복된 동일 bash: {}\n",
            fmt_pairs(&t.repeated_bash)
        ));
    }
    data.push_str("\n# 사용자가 입력한 프롬프트 (의도, 시간순)\n");
    for (i, p) in t.prompts.iter().enumerate() {
        data.push_str(&format!("{}. {}\n", i + 1, p));
    }

    let instr = load_prompt_template("session-retro", DEFAULT_SESSION_INSTR);
    // 배치 = [페르소나][지시][데이터(XML 태그)][톤 코다]. 2026-08-03 실측:
    // Claude 모범사례의 data-first([데이터][지시][코다])를 A/B했더니 EXAONE
    // 7.8B에선 합니다체 슬립 4~5건 vs 현행 0~1건 — 첫 토큰이 유저의 반말
    // 프롬프트 뭉치면 어조가 거기 앵커링된다. 소형 모델은 지시 양끝 배치
    // (선두 지시 + 말미 코다)가 승리. 태그 경계만 모범사례에서 채택.
    // claude 백엔드를 기본으로 바꾸는 날엔 data-first 재검토.
    let wrapped = format!("<session_data>\n{}</session_data>", data);
    match persona {
        Some(p) => format!("{}\n\n{}\n\n{}{}", p, instr, wrapped, TONE_CODA),
        None => format!("{}\n\n{}{}", instr, wrapped, TONE_CODA),
    }
}

/// Code-owned tone coda appended AFTER the data block — the very last tokens
/// before generation. The template already forbids 합니다체 (rules + bad
/// examples + a ★ reminder), yet session retros still slipped: the session
/// data block (dozens of verbatim prompts) pushes those rules far from the
/// generation point and EXAONE's instruction-following decays with distance
/// (same mechanism as mid-prompt persona being ignored — front placement
/// fixed that; recency fixes this end). Deterministic, template-independent.
const TONE_CODA: &str = "\n\n★ 이제 위 지침대로 회고를 써요. 모든 문장은 반드시 해요체('~해요/~예요/~했어요/~까요?')로 끝나야 해요 — '~습니다/~한다/~이다'는 한 문장도 금지예요.";

/// Append the actual TEXT of every "프롬프트 N" the retro cited — a bare number
/// isn't actionable ("프롬프트 9"가 뭐였는지 사용자가 알 수 없음). Deterministic:
/// pulls from the distilled verbatim prompts, so the quotes can't be wrong.
fn append_cited_prompts(body: &str, prompts: &[String]) -> String {
    const MARKER: &str = "프롬프트";
    let mut cited: Vec<usize> = Vec::new();
    let mut idx = 0;
    while let Some(pos) = body[idx..].find(MARKER) {
        let after = idx + pos + MARKER.len();
        let num: String = body[after..]
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(n) = num.parse::<usize>() {
            if n >= 1 && n <= prompts.len() && !cited.contains(&n) {
                cited.push(n);
            }
        }
        idx = after;
    }
    if cited.is_empty() {
        return body.to_string();
    }
    cited.sort_unstable();
    let mut out = String::from(body);
    out.push_str("\n\n## 언급한 프롬프트");
    for n in cited {
        let full = &prompts[n - 1];
        let snippet: String = full.chars().take(80).collect::<String>().replace('\n', " ");
        let ell = if full.chars().count() > 80 { "…" } else { "" };
        out.push_str(&format!("\n- **프롬프트 {n}**: \"{snippet}{ell}\""));
    }
    out
}

/// Built-in default for the SINGLE-session retrospective instruction (A/V4-14).
/// Same rigor as the cross-session prompt (해요체·이모지 금지·900자 캡·수치/
/// 프롬프트 인용 강제·예시 누출 가드·마찰 없으면 솔직히). Externalized to
/// `~/.toki/prompts/session-retro.md` — edit there, no rebuild.
const DEFAULT_SESSION_INSTR: &str = r#"당신은 Toki — 사용자가 Claude Code를 더 잘 쓰도록 돕는 회고 코치예요.
아래 <session_data> 태그 안이 한 코딩 세션의 객관적 추적 데이터(사용자가 입력한 프롬프트 + 마찰 신호)예요. 이걸 근거로 한국어 마크다운 회고를 써요.

# 방법론 (조언의 뼈대 — 원칙만 읊지 말고 데이터와 함께 자연스럽게)
숙련된 사용자가 AI 코딩을 잘 쓰는 법:
- **짧은 목줄 + 생성-검증 루프**(Karpathy): 다음 한 걸음을 구체적으로 지시하고 결과를 직접 검증했나. 방향 없이 "계속"으로 맡겼나.
- **작게 쪼갠 다음 한 걸음**: 큰 덩어리를 한 번에 시켰나, 점진적으로 나눠 갔나.
원칙 이름 나열·"Karpathy가 말하길" 식 인용 금지. 실제 프롬프트·수치에 얹어 자연스럽게.

절대 규칙:
- 데이터에 실제로 보이는 사실만. 프롬프트를 근거로 들 땐 "프롬프트 N"처럼 번호로 인용해요.
- ⚠️ 이 지침서에 나오는 예시 숫자·문구는 형식 설명일 뿐, 절대 실제 출력에 옮기지 말 것. 출력의 모든 근거는 위 데이터에서만.
- ⚠️ '습관적 위임'·'반복 편집' 같은 **신호 카드 이름을 언급하지 말 것** — 이건 단일 세션 회고라 그런 크로스세션 신호가 없어요. 근거는 데이터의 실제 항목(파일명·프롬프트 번호·에러/중단 수)만.
- 말투는 **해요체로 통일**: 사실은 단정("~했어요/~였어요"), 해석·관성 점검만 질문("~였을까요?"). '~습니다'·'~한다' 섞지 말 것.
- "더 체계적으로", "잘 활용했네요" 같은 일반론·덕담·빈말 금지. 파일 재읽기·도구 선택은 에이전트(Claude) 행동이니 그걸로 사용자를 탓하지 말고 "어디서 꼬였는지"의 증거로만.
- 에러·중단이 거의 없으면 억지 마찰을 만들지 말고 "막힘 없이 흘렀어요"라고 솔직히.
- 이모지·이모티콘 금지. 강조는 **굵게**만. **전체 900자 이내.** 각 섹션 짧게, 벽글 금지.
- 섹션 제목 다섯을 한 글자도 바꾸지 말 것. 다른 섹션 추가 금지. 서두 인사·맺음말 금지. 마크다운만.

정확히 이 섹션, 이 순서로:
## 이번 세션 한 줄
무엇을 시켰고 무엇이 나왔는지 1문장.

## 의도 — 어떻게 명령했나
프롬프트를 보고: 첫 지시가 명확했나 모호했나? 중간에 자주 수정·보충했나(=초기 지시가 덜 여물었다는 신호)? "프롬프트 N"으로 구체 인용.

## 과정 — 어디서 꼬였나
에러·중단·반복을 근거로 막혔던 지점을 사실로. 신호가 없으면 "막힘 없이 흘렀어요" 한 줄로 솔직히.

## 더 나았을 길
이 세션에 한해, 다르게 했다면 더 빨랐을 구체적 한두 가지. 일반론 금지.

## 관성 점검
이 세션에서 보이는, 무의식적으로 반복하는 듯한 습관 하나를 질문으로. (단정 말 것)

# 좋은 예시 (⚠️ **말투(해요체)·문장 구조·밀도만** 참고. 예시의 주제·소재는 이 세션과 무관한 자리표시일 뿐 — 로그인·토큰 같은 예시 주제를 실제 출력에 절대 옮기지 말 것. 출력의 주제·파일·수치는 반드시 위 데이터에서만.)
## 이번 세션 한 줄
어떤 기능을 고쳐달라 했고, 관련 파일을 수정해 마무리했어요.
## 의도 — 어떻게 명령했나
첫 지시(**프롬프트 1**)는 두루뭉술했고, 뒤(**프롬프트 3**)에서야 구체화됐어요. 초기 지시가 덜 여문 셈이에요.
## 과정 — 어디서 꼬였나
에러 몇 건이 한 지점에서 났고, 중간에 방향을 한 번 되돌렸어요.
## 더 나았을 길
- 첫 프롬프트에 완료 기준 한 줄을 붙였다면 뒤의 재설명을 아낄 수 있었을 거예요.
## 관성 점검
증상보다 "고쳐줘"를 먼저 던지는 흐름, 매번 반복되진 않나요?

# 이렇게 쓰지 마라
- "전반적으로 잘 하셨어요." (덕담·근거 없음)
- "일관성 문제가 드러났습니다." (합니다체 — 해요체로: "~드러났어요")
- "습관적 위임 신호 참조." (없는 신호 이름 — 데이터 항목만 인용)

★ 마지막으로 다시: **모든 문장의 끝을 '~해요 / ~예요 / ~했어요 / ~였어요 / ~까요?' 로.** '~다 / ~습니다 / ~이다'로 끝나는 문장이 하나라도 있으면 안 돼요. 서술도 질문도 전부 해요체."#;

/// Pull transcript, distill, run the coaching LLM, save the result.
///
/// Backend (V4-6): resolved from `app_settings.coach_backend`. Default is
/// the **local Ollama** backend (`exaone3.5:7.8b`) — zero 5h quota. The
/// `model` arg is a per-call *claude* alias override (from the old
/// opus/sonnet toggle) and is honored **only when** the backend is set to
/// `claude`; under the local backend it's ignored.
pub fn generate(
    db: &Arc<Db>,
    dungeon_id: &str,
    model: Option<&str>,
) -> Result<String> {
    let dungeon = db
        .dungeon_by_id(dungeon_id)?
        .ok_or_else(|| anyhow!("dungeon {} not found", dungeon_id))?;

    // dungeon_id == session_id. Find its transcript.
    let transcript = find_transcript(dungeon_id).ok_or_else(|| {
        anyhow!("이 세션의 transcript(.jsonl)를 찾을 수 없어요 — 회고를 만들 수 없습니다")
    })?;

    let trace = distill(&transcript)?;
    if trace.prompts.is_empty() {
        return Err(anyhow!("사람이 입력한 프롬프트가 없는 세션이라 회고할 내용이 없어요 (자동화·봇 호출)"));
    }

    let s = db.load_settings().unwrap_or_default();
    let persona = persona_block(&s.active_character);
    let prompt = build_prompt(&trace, persona.as_deref());

    // Only ClaudeP needs the project dir (for CLAUDE.md scope).
    let cwd = dungeon
        .project_path
        .as_deref()
        .map(PathBuf::from)
        .filter(|p| p.exists());
    let backend = match coach::Backend::resolve(db) {
        // Honor the per-call opus/sonnet override under the claude backend.
        coach::Backend::ClaudeP { .. } => coach::Backend::ClaudeP {
            model: model.unwrap_or(coach::DEFAULT_CLAUDE_MODEL).to_string(),
        },
        other => other,
    };
    eprintln!("[retro] coaching via {}", backend.label());

    let raw = coach::complete(&prompt, &backend, cwd.as_deref())?;
    // A/V4-14: same post-process as cross-session — strip emoji, force the first
    // heading to the spec'd one (EXAONE echoes the data-block title otherwise).
    let body = strip_emoji(&normalize_headings(&raw, "## 이번 세션 한 줄"));
    if body.is_empty() {
        return Err(anyhow!("empty retrospective body"));
    }

    db.save_retrospective(dungeon_id, &body, &Utc::now().to_rfc3339())?;
    Ok(body)
}

/// A/V4-14: single-session retro on the MOST RECENT session — the surface for
/// "이번 세션" coaching. Unlike `generate`, it needs no `dungeons` row (that only
/// supplied the ClaudeP cwd; the local Ollama backend ignores cwd). Finds the
/// newest transcript, distills, hardened-prompts, coaches. Returns the body.
pub fn generate_latest_session(db: &Arc<Db>) -> Result<String> {
    let path = recent_transcripts(1, 1, 1)
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("최근 세션 transcript를 찾을 수 없어요"))?;
    let trace = distill(&path)?;
    if trace.prompts.is_empty() {
        return Err(anyhow!("사람이 입력한 프롬프트가 없는 세션이라 회고할 내용이 없어요 (자동화·봇 호출)"));
    }
    let s = db.load_settings().unwrap_or_default();
    let persona = persona_block(&s.active_character);
    let prompt = build_prompt(&trace, persona.as_deref());

    let backend = coach::Backend::resolve(db);
    eprintln!("[retro] session retro (latest) via {}", backend.label());
    let raw = coach::complete(&prompt, &backend, None)?;
    let body = strip_emoji(&normalize_headings(&raw, "## 이번 세션 한 줄"));
    if body.is_empty() {
        return Err(anyhow!("empty retrospective body"));
    }
    // Show the actual text of every cited "프롬프트 N" (deterministic appendix).
    Ok(append_cited_prompts(&body, &trace.prompts))
}

/// One project the 회고 picker lists. `dir` = encoded project-dir name (command
/// arg); `label` = friendly name; `sessions` = its transcripts in the window.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectInfo {
    pub dir: String,
    pub label: String,
    pub last_active: String, // RFC3339 local (newest transcript mtime)
    pub sessions: usize,
}

/// Projects with activity in the last `within_days`, newest-first. Groups the
/// per-session transcripts by project dir so the picker shows "Toki" once, not
/// 10 sessions (user request: 한 프로젝트로 합쳐 보이기).
pub fn recent_projects(within_days: u64, max_projects: usize) -> Vec<ProjectInfo> {
    let Some(base) = dirs::home_dir().map(|h| h.join(".claude").join("projects")) else {
        return Vec::new();
    };
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(within_days * 86_400));
    let Ok(projects) = std::fs::read_dir(&base) else { return Vec::new() };
    let mut out: Vec<ProjectInfo> = Vec::new();
    for proj in projects.flatten() {
        let pp = proj.path();
        if !pp.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&pp) else { continue };
        let mut newest: Option<std::time::SystemTime> = None;
        let mut sample: Option<PathBuf> = None;
        let mut count = 0usize;
        for f in files.flatten() {
            let fp = f.path();
            if fp.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let mtime = f.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            if cutoff.map(|c| mtime >= c).unwrap_or(true) {
                count += 1;
                if newest.map(|n| mtime > n).unwrap_or(true) {
                    newest = Some(mtime);
                    sample = Some(fp);
                }
            }
        }
        if count == 0 {
            continue;
        }
        let last_active = newest
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| chrono::DateTime::<Utc>::from_timestamp(d.as_secs() as i64, 0))
            .map(|dt| dt.with_timezone(&Local).to_rfc3339())
            .unwrap_or_default();
        out.push(ProjectInfo {
            dir: proj.file_name().to_string_lossy().to_string(),
            label: sample.as_deref().map(project_label).unwrap_or_else(|| "?".into()),
            last_active,
            sessions: count,
        });
    }
    out.sort_by(|a, b| b.last_active.cmp(&a.last_active));
    out.truncate(max_projects);
    out
}

/// Merge several session traces (chronological) into one — concat prompts,
/// sum friction/tool/edit counts. The unit a project retro coaches on.
fn merge_traces(traces: &[SessionTrace]) -> SessionTrace {
    let mut m = SessionTrace::default();
    let mut tool: HashMap<String, u32> = HashMap::new();
    let mut edits: HashMap<String, u32> = HashMap::new();
    let mut reads: HashMap<String, u32> = HashMap::new();
    let mut reread: HashMap<String, u32> = HashMap::new();
    let mut bash: HashMap<String, u32> = HashMap::new();
    let mut rework: HashMap<String, (u32, u32, u32, u32)> = HashMap::new();
    for t in traces {
        m.prompts.extend(t.prompts.iter().cloned());
        m.n_turns += t.n_turns;
        m.thinking += t.thinking;
        m.errors += t.errors;
        m.interrupts += t.interrupts;
        if m.first_ts.is_none() {
            m.first_ts = t.first_ts.clone();
        }
        if t.last_ts.is_some() {
            m.last_ts = t.last_ts.clone();
        }
        for (k, c) in &t.tool_counts { *tool.entry(k.clone()).or_insert(0) += c; }
        for (k, c) in &t.top_edits { *edits.entry(k.clone()).or_insert(0) += c; }
        for (k, c) in &t.top_reads { *reads.entry(k.clone()).or_insert(0) += c; }
        for (k, c) in &t.reread_after_edit { *reread.entry(k.clone()).or_insert(0) += c; }
        for (k, c) in &t.repeated_bash { *bash.entry(k.clone()).or_insert(0) += c; }
        // ⑨: 프로젝트 회고는 세션 여럿을 합치므로 rework도 파일 단위로 합산한다
        // (연속 회차 chain은 합이 아니라 최댓값 — 깊이는 더해지는 값이 아니다).
        m.edit_cycles += t.edit_cycles;
        m.retries += t.retries;
        m.reworks += t.reworks;
        for (f, e, r, w, c) in &t.retry_files {
            let x = rework.entry(f.clone()).or_insert((0, 0, 0, 0));
            x.0 += e;
            x.1 += r;
            x.2 += w;
            x.3 = x.3.max(*c);
        }
    }
    let sort_cap = |m: HashMap<String, u32>, n: usize| -> Vec<(String, u32)> {
        let mut v: Vec<(String, u32)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(n);
        v
    };
    m.tool_counts = sort_cap(tool, 14);
    m.top_edits = sort_cap(edits, 8);
    m.top_reads = sort_cap(reads, 6);
    m.reread_after_edit = sort_cap(reread, 6);
    m.repeated_bash = sort_cap(bash, 6);
    m.retry_files = {
        let mut v: Vec<(String, u32, u32, u32, u32)> =
            rework.into_iter().map(|(f, (e, r, w, c))| (f, e, r, w, c)).collect();
        v.sort_by(|a, b| b.3.cmp(&a.3).then(b.4.cmp(&a.4)).then(a.0.cmp(&b.0)));
        v.truncate(8);
        v
    };
    if m.prompts.len() > MAX_PROMPTS {
        m.prompts.truncate(MAX_PROMPTS);
    }
    m
}

/// Cached project retro (like the cross-session cache) — so re-entering a
/// project shows the last result instantly ("N분 전") with no LLM spend.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RetroCache {
    pub body: String,
    pub generated_at: String, // RFC3339 local
    pub label: String,
}

fn retro_cache_path(dir: &str) -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".toki").join("retro").join(format!("{dir}.json")))
}

fn save_retro_cache(dir: &str, label: &str, body: &str) {
    let Some(p) = retro_cache_path(dir) else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let c = RetroCache {
        body: body.to_string(),
        generated_at: Local::now().to_rfc3339(),
        label: label.to_string(),
    };
    if let Ok(j) = serde_json::to_string(&c) {
        let _ = std::fs::write(&p, j);
    }
}

/// The last cached retro for a project, if any (instant re-open, no LLM).
pub fn load_retro_cache(dir: &str) -> Option<RetroCache> {
    let p = retro_cache_path(dir)?;
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

/// Project retro — merge the project's recent sessions (last `within_days`, cap
/// `max_sessions`) and coach on the whole. Hardened prompt + cited-prompt list.
/// Collect a project's sessions ACTIVE in the last `within_days` — the same
/// criterion the picker (`recent_projects`, mtime-based) uses, so a project
/// that shows in the list always yields ≥1 session here. Start-date filtering
/// (the old behavior) excluded long-running sessions that started before the
/// window but were active inside it → "목록엔 있는데 세션이 없어요" bug.
/// Recency: last_ts (end of activity ≈ mtime) → first_ts → file mtime.
fn project_recent_traces(dir: &str, within_days: i64) -> Vec<SessionTrace> {
    let Some(base) = dirs::home_dir().map(|h| h.join(".claude").join("projects").join(dir)) else {
        return Vec::new();
    };
    let cutoff = Utc::now() - chrono::Duration::days(within_days);
    let mut traces: Vec<SessionTrace> = Vec::new();
    if let Ok(files) = std::fs::read_dir(&base) {
        for f in files.flatten() {
            let fp = f.path();
            if fp.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Ok(tr) = distill(&fp) {
                let ts_recent = |s: Option<&str>| {
                    s.and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&Utc) >= cutoff)
                };
                let recent = ts_recent(tr.last_ts.as_deref())
                    .or_else(|| ts_recent(tr.first_ts.as_deref()))
                    .unwrap_or_else(|| {
                        f.metadata()
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs() as i64 >= cutoff.timestamp())
                            .unwrap_or(false)
                    });
                if recent && !tr.prompts.is_empty() {
                    traces.push(tr);
                }
            }
        }
    }
    traces
}

pub fn generate_project_retro(db: &Arc<Db>, dir: &str) -> Result<String> {
    let base = dirs::home_dir()
        .ok_or_else(|| anyhow!("no home"))?
        .join(".claude").join("projects").join(dir);
    let mut traces = project_recent_traces(dir, 7);
    if traces.is_empty() {
        return Err(anyhow!("이 프로젝트의 최근 7일 세션이 없어요"));
    }
    traces.sort_by(|a, b| a.first_ts.cmp(&b.first_ts));
    if traces.len() > 6 {
        let from = traces.len() - 6;
        traces.drain(..from);
    }
    let trace = merge_traces(&traces);
    let s = db.load_settings().unwrap_or_default();
    let persona = persona_block(&s.active_character);
    let prompt = build_prompt(&trace, persona.as_deref());
    let backend = coach::Backend::resolve(db);
    eprintln!("[retro] project retro '{}': {} sessions, {} prompts via {}",
        dir, traces.len(), trace.prompts.len(), backend.label());
    let raw = coach::complete(&prompt, &backend, None)?;
    let body = strip_emoji(&normalize_headings(&raw, "## 이번 세션 한 줄"));
    if body.is_empty() {
        return Err(anyhow!("empty retrospective body"));
    }
    let final_body = append_cited_prompts(&body, &trace.prompts);
    save_retro_cache(dir, &project_label(&base.join("_.jsonl")), &final_body);
    Ok(final_body)
}

/* ─────────────────────────────────────────────────────────────
   V4-6 §3.2 — CROSS-SESSION habit coaching.
   Per-session `distill` gives objective facts about one session; here we
   distill the last N sessions and aggregate them into signals that only
   exist ACROSS sessions (recurring frictions, time-of-day error patterns,
   delegation inertia, trend). Detection stays deterministic; the local LLM
   only phrases. This is the actual moat.
   ───────────────────────────────────────────────────────────── */

/// A session's transcript path + when it was last modified (for recency
/// ordering) — cheap to collect before the expensive distill.
///
/// P0-B: recency is a **time window** (default 7 days), not a fixed session
/// count — "최근 습관"이어야지 지난 분기 평균이 되면 안 된다. The window is
/// primary; if too few sessions fall inside it to compute a trend, we relax to
/// the `min_n` most-recent regardless of age. `max_n` caps the batch (distill
/// cost). Result is newest-first.
fn recent_transcripts(within_days: u64, min_n: usize, max_n: usize) -> Vec<PathBuf> {
    let Some(base) = dirs::home_dir().map(|h| h.join(".claude").join("projects")) else {
        return Vec::new();
    };
    let mut all: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let Ok(projects) = std::fs::read_dir(&base) else { return Vec::new() };
    for proj in projects.flatten() {
        let pp = proj.path();
        if !pp.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&pp) else { continue };
        for f in files.flatten() {
            let fp = f.path();
            if fp.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                let mtime = f
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                all.push((mtime, fp));
            }
        }
    }
    all.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    // Count how many fall inside the window (contiguous at the front, since
    // sorted newest-first).
    let in_window = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(within_days * 86_400))
        .map(|cutoff| all.iter().take_while(|(m, _)| *m >= cutoff).count())
        .unwrap_or(all.len());
    let n = in_window.max(min_n).min(max_n).min(all.len());
    all.into_iter().take(n).map(|(_, p)| p).collect()
}

/// Short-project label from an encoded project dir path. Claude Code
/// collapses `/` and `.` to `-`, so we just take the last dash-segment
/// (usually the leaf folder name, e.g. "…-04-my-app" → "my-app").
fn project_label(transcript: &std::path::Path) -> String {
    transcript
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| {
            s.trim_matches('-')
                .rsplit('-')
                // Skip numeric segments AND hash/uuid chunks (worktree and
                // scratchpad dirs end in hex ids — "f2469d5ac03b" is not a
                // project name a coaching card should cite).
                .find(|seg| {
                    !seg.is_empty()
                        && !seg.chars().all(|c| c.is_ascii_digit())
                        // UUIDs split into 8-4-4-4-12 hex chunks by the dash
                        // encoding — any all-hex segment ≥4 chars is noise,
                        // not a nameable project.
                        && !(seg.len() >= 4 && seg.chars().all(|c| c.is_ascii_hexdigit()))
                })
                .unwrap_or(s)
                .to_string()
        })
        .unwrap_or_else(|| "?".into())
}

/// A short "just keep going" prompt — the delegation-inertia signal
/// toki.md names ("다음 단계 진행" 관성). Objective proxy: very short, or a
/// bare continuation marker carrying no new direction.
fn is_continuation(p: &str) -> bool {
    let t = p.trim();
    let n = t.chars().count();
    if n == 0 {
        return false;
    }
    const MARKERS: &[&str] = &[
        "ㄱㄱ", "ㄱ", "고고", "고", "계속", "진행", "다음", "ㅇㅇ", "ㅇ", "네", "응", "ok", "okay",
        "yes", "y", "가자", "그대로", "쭉", "계속해", "이어서", "continue", "go",
    ];
    // P2-9: length cut tightened 4→2. "고쳐줘"/"빌드해"(3자) 같은 실제 지시가
    // 추임새로 오분류돼 위임 관성 %가 과대평가되던 걸 좁힘. ≤2자이거나
    // 정확히 마커일 때만 추임새로 센다.
    n <= 2 || MARKERS.iter().any(|m| t == *m || t.to_lowercase() == *m)
}

/// V4-12 ⑧ (규칙 후보): does this prompt read as a *correction* — the user
/// redirecting the agent ("X 말고 Y", "하지마", "기본으로")? Interrupt-adjacent
/// corrections are the strongest, but a marker scan over all prompts is the
/// deterministic proxy. Kept tight to avoid false positives; the dump decides
/// if it's too sparse.
fn is_correction(p: &str) -> bool {
    let t = p.to_lowercase();
    const MARKERS: &[&str] = &[
        "말고", "하지마", "하지 마", "하지말", "대신", "아니라", "빼고", "기본으로",
        "기본값", "말게", "말아", "instead", "don't", "dont", "not that", "never ", "always ",
    ];
    MARKERS.iter().any(|m| t.contains(m))
}

/// Korean particle/stopword trim so the SAME correction theme keys the same
/// token across sessions ("한국어**가**"/"한국어**를**"/"한국어**로**" → "한국어").
/// The natural-language analog of `normalize_command` for ⑧ (advisor's named
/// risk: josa fragmentation under-counting real repeats below the threshold).
fn trim_josa(word: &str) -> String {
    let two: &[&str] = &["으로", "에서", "에게", "부터", "까지", "처럼", "보다", "마다", "라고", "이라", "한테", "께서"];
    let one: &[&str] = &["가", "를", "은", "는", "이", "에", "로", "도", "의", "과", "와", "을", "께", "야", "아", "고"];
    let chars: Vec<char> = word.chars().collect();
    let n = chars.len();
    if n >= 4 {
        let tail: String = chars[n - 2..].iter().collect();
        if two.contains(&tail.as_str()) {
            return chars[..n - 2].iter().collect();
        }
    }
    if n >= 3 {
        let tail: String = chars[n - 1..].iter().collect();
        if one.contains(&tail.as_str()) {
            return chars[..n - 1].iter().collect();
        }
    }
    word.to_string()
}

/// Extract deduped content tokens from a session's correction-style prompts.
/// Each token counts once per session (the aggregate then counts sessions).
/// Josa-trimmed + stopword-filtered so themes cluster; ≥2 chars kept.
fn session_correction_tokens(prompts: &[String]) -> Vec<String> {
    const STOP: &[&str] = &[
        "말고", "하지마", "하지", "대신", "아니라", "아니", "빼고", "기본", "기본으로", "기본값",
        "그거", "이거", "저거", "그건", "이건", "그냥", "다시", "제발", "해줘", "하지말", "말게",
        "instead", "dont", "don't", "not", "never", "always", "that", "the", "this",
    ];
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for p in prompts.iter().filter(|p| is_correction(p)) {
        for raw in p.split(|c: char| !(c.is_alphanumeric())) {
            if raw.is_empty() {
                continue;
            }
            let tok = trim_josa(&raw.to_lowercase());
            if tok.chars().count() < 2 || STOP.contains(&tok.as_str()) {
                continue;
            }
            if seen.insert(tok.clone()) {
                out.push(tok);
            }
        }
    }
    out
}

/// P0-A: living/planning docs (plan/spec/toki/readme/memory/changelog/…) are
/// edited every session **by design** — that's not friction, it's the workflow.
/// Excluding them from the "반복 마찰" signal keeps a coaching card from
/// scolding the user for doing the right thing. Code-file repetition (.rs/.tsx
/// …) stays a valid signal. Only `.md`/`.txt` docs are candidates.
pub(crate) fn is_living_doc(basename: &str) -> bool {
    let lower = basename.to_lowercase();
    let Some(stem) = lower
        .strip_suffix(".md")
        .or_else(|| lower.strip_suffix(".txt"))
    else {
        return false; // non-doc (source file) — never excluded
    };
    const DOCS: &[&str] = &[
        "plan", "spec", "toki", "readme", "memory", "changelog", "improvements",
        "design-brief", "claude", "agents", "todo", "notes", "roadmap", "contributing",
    ];
    DOCS.iter()
        .any(|d| stem == *d || stem.starts_with(&format!("{d}-")))
}

/// 문서 파일(.md/.txt) — 산문 퇴고의 "다시 고침"은 겉돎이 아니다. 기존
/// `is_living_doc`은 고정 이름 화이트리스트(plan/spec/…)라 임의 문서가 새어
/// 들어왔다(실측: 상위 rtry%에 status·gateway 문서가 87%·83%로 랭크).
fn is_doc_file(basename: &str) -> bool {
    let l = basename.to_lowercase();
    l.ends_with(".md") || l.ends_with(".txt")
}

/// 검증형 Bash — 실제로 **실행/검사**하는 명령만 retry 트리거로 본다.
/// 실측(30일 4,528회): 탐색·이동(cd·ls·cat·grep·sed)이 67%인데 그것까지 세면
/// retry%가 겉돎이 아니라 **Bash 사용 빈도(작업 스타일)** 를 재게 된다 —
/// §3.3.1에서 신호 4종을 죽인 confound와 같은 병.
fn is_verify_bash(cmd: &str) -> bool {
    let c = cmd.to_lowercase();
    const VERIFY: &[&str] = &[
        "cargo test", "cargo check", "cargo clippy", "cargo build", "cargo run",
        "npm test", "npm run", "pnpm ", "yarn ", "vite", "tsc", "eslint", "ruff", "mypy",
        "pytest", "vitest", "jest", "go test", "go run", "make ", "docker", "pm2 ",
        "python ", "python3 ", "node ", "open -a", "osascript",
    ];
    VERIFY.iter().any(|p| c.contains(p))
}

fn norm_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "내가 방금 쓴 것을 다시 먹었나". 통짜 blob 포함은 배제한다 — Write가 파일
/// 전체를 앵커로 남기면 이후 모든 편집이 자동 포함돼 지표가 천장에 붙는다
/// (프로토타입에서 상위가 전부 90%대로 saturate된 원인). 짧은 쪽이 긴 쪽의
/// 절반 이상일 때만 같은 영역으로 인정.
fn overlaps_own(old: &str, prev: &str) -> bool {
    if old.len() < 12 || prev.len() < 12 {
        return false;
    }
    if old == prev {
        return true;
    }
    let (lo, hi) = if old.len() <= prev.len() { (old, prev) } else { (prev, old) };
    hi.contains(lo) && lo.len() * 2 >= hi.len()
}

/// Lightweight per-session summary — the unit we aggregate over.
///
/// `project`/`local_hour`/`errors`/`interrupts`/`turns` are no longer read by
/// any card (the error/daypart/trend/project signals were removed 2026-07-14 —
/// see CrossAgg comment) but are retained: they're the raw substrate the
/// diagnostic dump uses (to prove the daypart artifact) and that a future
/// *length-normalized* error signal or an interrupt-based ⑧ would need. The
/// data was never wrong — the signals built on it were confounded.
struct SessionSummary {
    #[allow(dead_code)]
    project: String,
    started: Option<String>, // raw first_ts (RFC3339, sorts chronologically)
    #[allow(dead_code)]
    local_hour: Option<u32>,
    #[allow(dead_code)]
    dow: Option<u32>, // 0=Mon .. 6=Sun
    date: Option<String>,
    prompts: usize,
    short_prompts: usize,
    #[allow(dead_code)]
    errors: u32,
    #[allow(dead_code)]
    interrupts: u32,
    #[allow(dead_code)]
    turns: u32,
    repeated_bash: Vec<String>, // snippets repeated >=3x within the session
    // Retained (no card reads it after ③ 반복 편집 removal 2026-07-14) as the
    // substrate for a future line-level churn / thrash-vs-iterate friction signal.
    #[allow(dead_code)]
    heavy_edits: Vec<String>,   // files edited a lot this session
    bash_families: Vec<String>, // ⑦: distinct normalized families seen ≥1× this session
    /// ⑨ 겉도는 편집 (B, 2026-08-20 해결): 이 세션에서 **자기 출력을 다시 먹은**
    /// 파일들. (path, edits, reworks, 연속 최대 N회차). 파일 단위 retry가 아니라
    /// 영역 단위라 iteration과 갈린다 — 판정 근거는 spec §3.3.2.
    rework_files: Vec<(String, u32, u32, u32)>,
    // ⑧ detection is built + validated (dump_action_signals) but its CARD is
    // deferred: on real data the corrections are diverse one-offs with no theme
    // repeating ≥2 sessions, so a card would fabricate a pattern (§1.1). Kept
    // populated so ⑧ can be wired the moment a groundable theme exists.
    #[allow(dead_code)]
    corrections: Vec<String>, // ⑧: josa-trimmed content tokens from correction-style prompts
}

fn summarize(trace: &SessionTrace, transcript: &std::path::Path) -> SessionSummary {
    let (local_hour, dow, date) = match trace
        .first_ts
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
    {
        Some(dt) => {
            let l = dt.with_timezone(&Local);
            (
                Some(l.hour()),
                Some(l.weekday().num_days_from_monday()),
                Some(l.format("%m/%d").to_string()),
            )
        }
        None => (None, None, None),
    };
    let short = trace.prompts.iter().filter(|p| is_continuation(p)).count();
    SessionSummary {
        project: project_label(transcript),
        started: trace.first_ts.clone(),
        local_hour,
        dow,
        date,
        prompts: trace.prompts.len(),
        short_prompts: short,
        errors: trace.errors,
        interrupts: trace.interrupts,
        turns: trace.n_turns,
        repeated_bash: trace.repeated_bash.iter().map(|(k, _)| k.clone()).collect(),
        // P0-A: drop living/planning docs — recurring edits to them are the
        // intended workflow, not friction. Code files stay.
        heavy_edits: trace
            .top_edits
            .iter()
            .map(|(k, _)| k.clone())
            .filter(|f| !is_living_doc(f))
            .collect(),
        bash_families: trace.bash_families.clone(),
        rework_files: trace
            .retry_files
            .iter()
            .filter(|(_, _, _, w, _)| *w > 0)
            .map(|(f, e, _, w, c)| (f.clone(), *e, *w, *c))
            .collect(),
        corrections: session_correction_tokens(&trace.prompts),
    }
}

// V4-12 (2026-07-14): error-based observation signals (② 시간 리듬 · ⑤ 추세)
// and ⑥ 집중 분포 were REMOVED. `errors` = `tool_result.is_error` count, which
// (a) folds benign dead-ends (empty grep, speculative Read) in with real
// friction, (b) is the AGENT's tool behavior, not a user-controlled habit, and
// (c) is dominated by session length: the real data showed "오전이 거칠다"
// (4.6 vs 0.1 err/session) was purely that morning sessions run 378 turns vs
// evening's 26 — length-normalized (err/100turns) the daypart effect vanishes
// (1.2 vs 1.4 vs 0.4). Coaching keeps only actionable/user-controlled signals:
// ① 위임 관성 · ③ 반복 마찰 · ⑦ 스킬 후보(④ fallback).
struct CrossAgg {
    n_sessions: usize,
    span: (String, String), // earliest .. latest date
    total_prompts: usize,
    total_short: usize,
    /// V4-12 ⑦: ritual heads ("cargo check", "git push") by #sessions, ≥3,
    /// inspection verbs excluded. Redundancy gate (needs FS) is applied later
    /// in generate_cross, not here (aggregate stays pure/testable).
    skill_heads: Vec<(String, u32)>,
    /// ⑨: 겉돎 후보 1건 — (파일, edits, reworks, 연속 최대 N회차, 걸친 세션 수).
    /// 연속 회차가 가장 깊은 것 하나만. 카드 발화 게이트는 derive_signals에서.
    thrash: Option<(String, u32, u32, u32, u32)>,
}

/// Only the (ignored) diagnostic dump uses this now — it's how we proved the
/// daypart error signal was a session-length artifact (see CrossAgg comment).
#[allow(dead_code)]
fn daypart(hour: u32) -> &'static str {
    match hour {
        0..=5 => "새벽(0-5)",
        6..=11 => "오전(6-11)",
        12..=17 => "오후(12-17)",
        _ => "저녁·밤(18-23)",
    }
}

/// `summaries` must be **chronological, oldest-first** — the trend split and
/// span endpoints rely on it. Callers sort by `started` before this.
fn aggregate(summaries: &[SessionSummary]) -> CrossAgg {
    let total_prompts: usize = summaries.iter().map(|s| s.prompts).sum();
    let total_short: usize = summaries.iter().map(|s| s.short_prompts).sum();

    // recurring across sessions: count in how many distinct sessions each
    // key appears (dedupe keys within a session first).
    let count_across = |pick: &dyn Fn(&SessionSummary) -> &Vec<String>| -> Vec<(String, u32)> {
        let mut m: HashMap<String, u32> = HashMap::new();
        for s in summaries {
            let mut seen = std::collections::HashSet::new();
            for k in pick(s) {
                if seen.insert(k.clone()) {
                    *m.entry(k.clone()).or_insert(0) += 1;
                }
            }
        }
        let mut v: Vec<(String, u32)> = m.into_iter().filter(|(_, c)| *c >= 2).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(8);
        v
    };

    // ⑦: count ritual HEADS across sessions (not full normalized strings —
    // the user's compound one-liners are unique per session; the head is the
    // ritual identity). Dedupe heads per session, drop inspection verbs, ≥3.
    let skill_heads = {
        let mut m: HashMap<String, u32> = HashMap::new();
        for s in summaries {
            let mut seen = std::collections::HashSet::new();
            for fam in &s.bash_families {
                if let Some(h) = family_head(fam) {
                    if !is_inspect_head(&h) && seen.insert(h.clone()) {
                        *m.entry(h).or_insert(0) += 1;
                    }
                }
            }
        }
        let mut v: Vec<(String, u32)> = m.into_iter().filter(|(_, c)| *c >= 3).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(8);
        v
    };

    // span: first (oldest) .. last (newest) date, since summaries are sorted.
    let first_date = summaries.iter().find_map(|s| s.date.clone()).unwrap_or_default();
    let last_date = summaries.iter().rev().find_map(|s| s.date.clone()).unwrap_or_default();
    let span = (first_date, last_date);

    // ⑨: 파일별로 세션을 가로질러 합치되, 순위는 **연속 회차(chain)** 로 매긴다.
    // 총 rework 수는 큰 파일이 자동으로 이기지만, "같은 자리를 연속 몇 번 다시
    // 먹었나"는 파일 크기와 무관한 겉돎의 깊이다.
    let mut per_file: HashMap<String, (u32, u32, u32, u32)> = HashMap::new(); // e, w, chain, sessions
    for s in summaries {
        for (f, e, w, c) in &s.rework_files {
            let x = per_file.entry(f.clone()).or_insert((0, 0, 0, 0));
            x.0 += e;
            x.1 += w;
            x.2 = x.2.max(*c);
            x.3 += 1;
        }
    }
    let thrash = per_file
        .into_iter()
        .max_by(|a, b| a.1 .2.cmp(&b.1 .2).then(a.1 .1.cmp(&b.1 .1)).then(b.0.cmp(&a.0)))
        .map(|(f, (e, w, c, sess))| (f, e, w, c, sess));

    CrossAgg {
        n_sessions: summaries.len(),
        span,
        total_prompts,
        total_short,
        skill_heads,
        thrash,
    }
}

/// P0-B: narrow a chronological (oldest-first) session list to those that
/// actually STARTED within `days`. mtime-recency (how transcripts are picked)
/// is only a superset — a months-old session merely *touched* this week has a
/// fresh mtime but old start, and including it drags the span/averages back to
/// a "지난 분기 평균" (the exact thing the user flagged). When a quiet window
/// has fewer than `min_n` such sessions (too thin for a trend), relax to the
/// `min_n` most-recent by start. Finally cap at `max_n`. Stays oldest-first.
fn keep_recent_by_start(
    mut summaries: Vec<SessionSummary>,
    days: i64,
    min_n: usize,
    max_n: usize,
) -> Vec<SessionSummary> {
    let cutoff = Utc::now() - chrono::Duration::days(days);
    let started_recent = |s: &SessionSummary| -> bool {
        s.started
            .as_deref()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|dt| dt.with_timezone(&Utc) >= cutoff)
            .unwrap_or(false)
    };
    let recent_n = summaries.iter().filter(|s| started_recent(s)).count();
    if recent_n >= min_n {
        summaries.retain(|s| started_recent(s));
    } else if summaries.len() > min_n {
        let from = summaries.len() - min_n; // keep the min_n most-recent
        summaries.drain(..from);
    }
    if summaries.len() > max_n {
        let from = summaries.len() - max_n;
        summaries.drain(..from);
    }
    summaries
}

/* ─────────────────────────────────────────────────────────────
   V4-12 — action-proposal coaching (⑦ 스킬 후보 / ⑧ 규칙 후보).
   New signals go from "네 패턴은 이렇다" to "그래서 이렇게 인코딩해라".
   Constitution holds: the count is deterministic; the LLM only phrases.
   The Redundancy gate below suppresses proposing what the user ALREADY
   encoded (a skill/command/CLAUDE.md rule/memory note) — else ⑦ proudly
   proposes "make git push a skill" for the ritual already in memory.
   ───────────────────────────────────────────────────────────── */

/// The significant head of a normalized ritual family — the first two tokens
/// that aren't placeholders (`<path>` …), operators, or runner prefixes
/// (`npx`/`sudo`/…). Used both as the redundancy needle and the card label.
/// `"npx tauri build"` → `"tauri build"`, `"git push origin main"` → `"git push"`,
/// `"npm test 2>&1 | tail -<n>"` → `"npm test"`.
fn family_head(normalized: &str) -> Option<String> {
    let sig: Vec<&str> = normalized
        .split_whitespace()
        .filter(|t| {
            !t.starts_with('<')            // placeholders
                && !t.starts_with('-')     // flags
                && !matches!(*t, "|" | "&&" | "||" | ";" | "&")
                && !t.contains(">&")
                && !t.starts_with('>')
                // runner/nav prefixes carry no ritual identity
                && !matches!(*t, "npx" | "sudo" | "time" | "env" | "command" | "cd" | "then" | "do")
        })
        .take(2)
        .collect();
    if sig.is_empty() {
        None
    } else {
        Some(sig.join(" "))
    }
}

/// Verbs that are inspection / navigation / ad-hoc scripting — NOT rituals
/// worth encoding as a skill. Validated by the real-data dump: without this
/// the ⑦ list is dominated by `grep`/`ls`/`git status` noise. Read-only git
/// subcommands are 2-token heads ("git log").
const INSPECT_HEADS: &[&str] = &[
    "grep", "find", "cat", "wc", "head", "tail", "ls", "echo", "sed", "awk",
    "sort", "uniq", "strings", "which", "file", "stat", "sleep", "true", "open",
    "python3", "python", "node", "bash", "sh", "ruby", "perl", "osascript",
    "git status", "git log", "git diff", "git show", "git branch",
];
fn is_inspect_head(head: &str) -> bool {
    let first = head.split_whitespace().next().unwrap_or("");
    INSPECT_HEADS.contains(&first) || INSPECT_HEADS.contains(&head)
}

/// Concatenated, lowercased text of everywhere the user could have already
/// encoded a ritual/rule: global `~/.claude/{commands,skills}`, global
/// `CLAUDE.md`, and the local memory dir (`~/.claude/projects/*/memory/*.md`).
/// Read once per run and substring-matched — deterministic, no LLM. Bounded
/// (skip large files, cap total) so a stray big file can't blow memory.
fn redundancy_corpus() -> String {
    let Some(home) = dirs::home_dir() else { return String::new() };
    let claude = home.join(".claude");
    let mut roots: Vec<PathBuf> = vec![
        claude.join("commands"),
        claude.join("skills"),
    ];
    // memory dirs live under each project subdir
    if let Ok(projects) = std::fs::read_dir(claude.join("projects")) {
        for p in projects.flatten() {
            let mem = p.path().join("memory");
            if mem.is_dir() {
                roots.push(mem);
            }
        }
    }
    let mut out = String::new();
    // global CLAUDE.md (a single high-value file)
    if let Ok(s) = std::fs::read_to_string(claude.join("CLAUDE.md")) {
        out.push_str(&s.to_lowercase());
        out.push('\n');
    }
    const MAX_FILE: u64 = 256 * 1024;
    const MAX_TOTAL: usize = 3 * 1024 * 1024;
    let mut stack = roots;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let is_text = matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("md" | "txt" | "toml" | "json" | "sh")
            );
            if !is_text {
                continue;
            }
            if entry.metadata().map(|m| m.len()).unwrap_or(u64::MAX) > MAX_FILE {
                continue;
            }
            if let Ok(s) = std::fs::read_to_string(&path) {
                out.push_str(&s.to_lowercase());
                out.push('\n');
            }
            if out.len() > MAX_TOTAL {
                return out;
            }
        }
    }
    out
}

/// Is this ritual/rule already encoded? Case-insensitive substring of the
/// family head (or a rule keyword) in the redundancy corpus. Conservative:
/// only a real textual hit suppresses, so we under-suppress rather than hide
/// a genuinely new proposal.
fn already_encoded(needle: &str, corpus: &str) -> bool {
    let n = needle.trim().to_lowercase();
    !n.is_empty() && corpus.contains(&n)
}

/// One structured cross-session signal — the content of a report card in
/// the coaching modal. Derived 100% deterministically from the aggregate
/// (no LLM), so the card grid can never hallucinate a number. This is the
/// grounded replacement for the design mockup's fabricated "+23%" cards.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CoachSignal {
    pub cat: String,  // category chip, e.g. "위임 관성"
    pub stat: String, // headline figure, e.g. "23%"
    pub body: String, // one grounded sentence of context
}

/// Full cross-session coaching payload: deterministic signal cards + the
/// LLM-phrased markdown body. Serde round-trips through the file cache
/// (`~/.toki/coaching/*.json`), so it derives Deserialize too.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CrossCoaching {
    pub n_sessions: usize,
    pub span_start: String,
    pub span_end: String,
    pub signals: Vec<CoachSignal>,
    pub body: String,
    /// RFC3339 local time the analysis was generated — drives the "N시간 전
    /// 분석" label so a cached result reads as a snapshot, not live.
    #[serde(default)]
    pub generated_at: String,
}

/// `skill` is the top ⑦ ritual head that survived the Redundancy gate (None if
/// none qualified / all already encoded). Passed in because the gate needs FS
/// access, which `aggregate` (pure) can't do.
fn derive_signals(a: &CrossAgg, skill: Option<&(String, u32)>) -> Vec<CoachSignal> {
    let mut v: Vec<CoachSignal> = Vec::new();

    // ① Delegation inertia — share of bare "keep going" prompts. Only NOTABLE
    // delegation is a signal: at 0-1% ("you steer well") the card is a non-
    // finding, and forcing the LLM to write a "pattern" about it made it
    // fabricate (it copied the prompt's example number 23% over the real 0%).
    // Gate at a threshold so a near-zero value simply doesn't appear.
    const DELEGATION_MIN_PCT: f64 = 10.0;
    if a.total_prompts > 0 {
        let pct = a.total_short as f64 / a.total_prompts as f64 * 100.0;
        if pct >= DELEGATION_MIN_PCT {
            v.push(CoachSignal {
                cat: "습관적 위임".into(),
                stat: format!("{:.0}%", pct),
                body: format!(
                    "프롬프트 {}개 중 {}개가 'ㄱㄱ·계속' 류 추임새 — 새 방향 없이 진행만 맡긴 비율.",
                    a.total_prompts, a.total_short
                ),
            });
        }
    }

    // (② 시간 리듬 removed 2026-07-14 — was a session-length artifact, not a
    //  time-of-day habit; see CrossAgg comment.)

    // (③ 반복 편집 removed 2026-07-14 — "same file across N sessions" conflates
    //  healthy iterative building (adding features to your main file over days)
    //  with thrashing (returning to fix the same bug). Edit COUNT can't tell
    //  them apart deterministically, so it mislabeled the user's active work
    //  file as friction — the same confound as 시간 리듬/추세/집중 분포. A real
    //  friction signal would need line-level churn / reread-after-edit + errors,
    //  which we don't capture cleanly; don't fake it.)

    // ⑦ Skill candidate (supersedes ④ when a real ritual is found) — a command
    // ritual recurring across ≥3 sessions that isn't already encoded. This is
    // the "action proposal": not "you did X", but "encode X as a command/skill".
    if let Some((head, c)) = skill {
        v.push(CoachSignal {
            cat: "스킬 후보".into(),
            stat: format!("{}개 세션", c),
            body: format!(
                "`{}` 류를 {}개 세션에서 반복했어요. 하루 한 번 이상 반복하는 명령은 슬래시 커맨드나 스킬로 묶을 후보예요.",
                head, c
            ),
        });
    }
    // (④ 도구 습관 removed 2026-08-20 — ⑦이 빈손일 때 "그래도 뭔가 보여주려고"
    //  두었던 관찰 폴백. 관찰뿐이라 LLM이 행동을 지어냈다: `sed -n <str> <path>`
    //  를 두고 "/sed 슬래시 커맨드로 묶으세요"라고 권했는데, 사용자는 sed를 친
    //  적이 없고(에이전트의 파일 읽기 방식) 인자가 매번 달라 묶을 수도 없다.
    //  집중 분포를 지운 것과 같은 이유 — "행동 제안 없는 순수 기술". 할 말이
    //  없으면 정직한 빈 상태(canned body)가 이미 있다.)

    // ⑨ 겉도는 편집 (B — 2026-08-20, 4차 만에 해결). 파일 단위 "반복 편집"이
    // iteration과 안 갈려 3번 보류됐던 신호. 축을 **영역 단위**로 내려서 갈렸다:
    // 내가 방금 쓴 것을 다시 먹으면 rework, 그게 연속되면 그 자리에서 겉도는 것.
    // 게이트를 높게 잡는다 — 실측(30일)에서 rework는 전체 편집의 8%뿐이고
    // chain 3+는 소수 파일에만 나타난다. 흔한 일이면 코칭이 못 쓰는 소리가 된다.
    const THRASH_MIN_CHAIN: u32 = 3;
    const THRASH_MIN_REWORK: u32 = 3;
    if let Some((file, edits, reworks, chain, _sess)) = &a.thrash {
        if *chain >= THRASH_MIN_CHAIN && *reworks >= THRASH_MIN_REWORK {
            let name = basename(file);
            v.push(CoachSignal {
                cat: "겉도는 편집".into(),
                stat: format!("{}회 연속", chain),
                body: format!(
                    "`{}`에서 같은 자리를 {}회 연속 다시 고쳤어요 (편집 {}개 중 {}개가 직전 결과를 되돌림). 되돌리기가 이어지면 다음 수정을 시키기 전에 무엇이 맞는지부터 정하는 게 빨라요.",
                    name, chain, edits, reworks
                ),
            });
        }
    }

    // (⑤ 추세 · ⑥ 집중 분포 removed 2026-07-14 — ⑤ rode the same confounded
    //  error metric as 시간 리듬; ⑥ was pure description with no action. Coaching
    //  now keeps only actionable/user-controlled signals.)

    v.truncate(8);
    v
}

/// Built-in default for the cross-session coaching instruction. Materialized
/// to `~/.toki/prompts/cross-coaching.md` on first run; the file is the source
/// of truth thereafter (this is only the seed / reset target).
const DEFAULT_CROSS_INSTR: &str = r#"당신은 Toki — 사용자가 Claude Code를 여러 세션에 걸쳐 더 잘 쓰도록 돕는 습관 코치입니다.
아래 <cross_data> 태그 안이 사용자의 최근 여러 세션을 집계한 객관적 데이터와, 거기서 추출된 번호 붙은 핵심 신호입니다. 한 세션이 아니라 **세션들을 가로질러 반복되는 습관**에 대해 한국어 마크다운으로 코칭하세요.

# 방법론 (조언의 뼈대 — 원칙만 읊지 말고 항상 신호 수치와 함께 자연스럽게)
숙련된 사용자가 AI 코딩 에이전트를 잘 쓰는 검증된 방식. 각 원칙은 우리 신호와 맞물립니다:
- **짧은 목줄 + 생성-검증 루프**(Karpathy): 에이전트는 생성이 값싸서 끝없이 뱉고, 병목은 사람의 '검증'이다. "계속/ㄱㄱ"로 방향 없이 넘기는 건 목줄을 놓는 것 — 다음 한 걸음을 구체적으로 지시하고 매 단계 결과를 직접 확인하는 게 핵심. (↔ 습관적 위임 신호)
- **작게 쪼갠 다음 한 걸음**: 큰 덩어리 대신 "다음의 구체적·점진적 변경 하나"를 시키고 → 검토 → 테스트 → 커밋 → 다음. (방향을 줄 때 이 단위로 쪼개면 습관적 위임이 줄어요.)
- **반복은 인코딩**: 매 세션 같은 명령·의식을 손으로 치면 슬래시 커맨드·스킬·CLAUDE.md 규칙으로 박아 루프를 빠르게. 반복 교정도 규칙으로 승격. (↔ 스킬 후보/규칙 후보 신호)
원칙을 조언의 근거로만 삼되, "Karpathy가 말하길" 식 인용·원칙 이름 나열은 금지. 사용자의 실제 수치에 얹어 자연스럽게.

# 신호 선택 우선순위
신호가 여럿이면 이 순서로 **1-2개만** 고른다: ① 걸친 세션 수가 많은 것 → ② 행동으로 바로 바꿀 수 있는 것(스킬 후보처럼 인코딩 가능) → ③ 동률이면 최근 것. 나머지는 버린다(다 다루려 하지 말 것).

절대 규칙:
- 근거는 위 '핵심 신호'에 실제로 적힌 수치·항목만. **진단·권고 문장마다 핵심 신호의 실제 수치를 정확히 1회 인용**, 같은 수치를 여러 문장에 반복 인용하지 말 것. 수치 없이도 되는 문장이면 뺀다.
- ⚠️ **이 지침서에 나오는 예시 숫자(23%·14세션·3개 세션·N% 등)는 형식 설명일 뿐이다. 절대 실제 출력에 옮기지 말 것.** 출력의 모든 수치는 오직 위 '핵심 신호' 블록에 적힌 값에서만 가져온다. (핵심 신호에 없는 숫자를 쓰면 환각이다.)
- **말투는 '해요체'로 처음부터 끝까지 통일**: 서술은 "~예요 / ~했어요 / ~있어요", 질문은 "~할까요? / ~하지 않나요?". **'~습니다'(합니다체)·'~한다/~된다'(한다체)를 절대 섞지 말 것.** (예: "나타났습니다"❌ → "나타났어요"⭕, "편집되었다"❌ → "편집했어요"⭕)
- **수치 사실은 단정으로(질문으로 흐리지 말 것), 해석·관성 점검만 질문으로.** 단정도 해요체 서술형("추임새가 23%였어요")이지 물음표가 아님.
- "더 계획적으로", "잘 하고 있어요" 같은 일반론·덕담·빈말 금지. 에이전트(Claude) 행동으로 사용자를 탓하지 말 것(파일 반복 편집은 "어디서 고생하나"의 증거로만).
- 데이터가 빈약하면 억지 패턴 금지: 요약에 세션 부족을 밝히고, 반복되는 패턴은 "아직 뚜렷한 패턴 없음" 한 줄, 실험해볼 것은 데이터를 늘릴 행동 1개만.
- **이모지·이모티콘 금지**. 강조는 **굵게**만.
- **전체 900자 이내(공백 포함)**. 각 불릿 2문장 이내. 벽글 금지.
- 섹션 제목 넷을 한 글자도 바꾸지 말 것. 다른 섹션 추가 금지. 서두 인사·맺음말 금지. 마크다운만.

정확히 이 섹션, 이 순서로:
## 요약
1문장: 집계 기간·세션 수 + 가장 눈에 띄는 크로스세션 습관 하나(핵심 신호에서, 수치 인용).

## 반복되는 패턴
위 우선순위로 고른 신호 1-2개를 각각 `- ` 불릿 하나로. "신호N" 번호와 수치를 인용. 사실은 단정형, 해석만 질문형.

## 실험해볼 것
다음 세션에 실행 가능한 행동 1-2개를 `- ` 불릿으로. **각 불릿은 "무엇을 한다 → 다음 집계에서 확인할 수치"로 닫아요**(예: "'계속' 대신 다음 변경 하나를 명시해봐요 → 다음 집계에서 그 추임새 비율이 내려가는지 봐요"). 확인할 수치는 핵심 신호의 실제 값으로. 일반론 금지.
- **"스킬 후보" 신호가 있으면 반드시 첫 불릿**: 단일 반복 명령이면 **슬래시 커맨드**(`/이름`), 보조 파일·스크립트가 필요하면 **스킬**로 묶으라 구체 권고 + "N개 세션" 인용 + 다음 집계 확인 수치.
- **"규칙 후보" 신호가 있으면**: 반복 교정을 CLAUDE.md 규칙 한 줄로 승격(구체·검증가능). "항상/절대"류는 산문 대신 **훅**으로.

## 관성 점검
여러 세션에 걸쳐 무의식적으로 반복하는 습관 하나를 한 문장 질문으로.

# 좋은 예시 (형식·톤·밀도 참고용. 말투가 처음부터 끝까지 해요체인 것에 주목. ⚠️예시의 수치·명령·파일명은 절대 실제 출력에 옮기지 말 것 — 반드시 위 '핵심 신호'의 실제 값만 사용)
## 요약
최근 7일 14개 세션에서 `pytest` 실행을 4개 세션 반복한 게 가장 눈에 띄어요.
## 반복되는 패턴
- **신호3 (스킬 후보)**: `pytest`를 4개 세션에서 반복 실행했어요. 검증 명령을 매번 손으로 치는 흐름이에요.
## 실험해볼 것
- `pytest`를 `/test` 슬래시 커맨드로 묶어봐요 → 다음 집계에서 이 명령이 '스킬 후보'로 다시 뜨지 않는지 봐요.
## 관성 점검
검증 명령을 손으로 반복하는 흐름, 커맨드로 옮길 때가 되지 않았을까요?

# 이렇게 쓰지 마라 (나쁜 문장)
- "전반적으로 잘 하고 계세요." (덕담·수치 없음)
- "특정 부분에 대한 지속적인 수정이 필요함을 시사한다." (한다체 — 해요체로: "~수정이 필요해 보여요")
- "가장 두드러지는 습관으로 나타났습니다." (합니다체 — 해요체로: "~습관으로 나타났어요")
- "추임새가 많았을까요?" (수치 사실을 질문으로 흐림 — "추임새가 23%였어요"처럼 단정으로)"#;

/// Load a user-editable prompt template from `~/.toki/prompts/<name>.md`, read
/// at generation time (edits apply on the next run — no rebuild). On first use
/// the built-in `default` is written there so there's a file to find and edit;
/// **delete the file to regenerate the default**. A blank file falls back to
/// the default (so the prompt can't be accidentally emptied).
/// 사용자 편집 프롬프트가 사는 곳.
pub(crate) fn prompts_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".toki").join("prompts"))
}

/// 로컬 오버라이드 전부 삭제 → 다음 실행에 코드 기본값이 다시 씨앗으로 깔린다.
/// v5 M5: 로컬 파일이 코드 개선(engram 고유명사 제거 등)을 계속 덮는 걸
/// 사용자가 알고 풀 수 있어야 한다(spec §6.4 "지시문 두 층"). 지운 파일 수를 돌려준다.
pub fn reset_prompt_templates() -> usize {
    let Some(dir) = prompts_dir() else { return 0 };
    let Ok(rd) = std::fs::read_dir(&dir) else { return 0 };
    let mut n = 0;
    for f in rd.flatten() {
        let p = f.path();
        if p.extension().and_then(|e| e.to_str()) == Some("md") && std::fs::remove_file(&p).is_ok() {
            n += 1;
        }
    }
    n
}

pub(crate) fn load_prompt_template(name: &str, default: &str) -> String {
    let Some(dir) = dirs::home_dir().map(|h| h.join(".toki").join("prompts")) else {
        return default.to_string();
    };
    let path = dir.join(format!("{name}.md"));
    if let Ok(s) = std::fs::read_to_string(&path) {
        if !s.trim().is_empty() {
            eprintln!("[retro] coaching prompt ← {} (user-edited)", path.display());
            return s;
        }
    }
    let _ = std::fs::create_dir_all(&dir);
    if std::fs::write(&path, default).is_ok() {
        eprintln!("[retro] coaching prompt: seeded default → {} (편집하면 재빌드 없이 반영)", path.display());
    }
    default.to_string()
}

fn build_cross_prompt(a: &CrossAgg, signals: &[CoachSignal], persona: Option<&str>) -> String {
    let pct = |num: usize, den: usize| -> f64 {
        if den == 0 { 0.0 } else { num as f64 / den as f64 * 100.0 }
    };
    let fmt_pairs = |v: &[(String, u32)]| -> String {
        if v.is_empty() {
            "(없음)".to_string()
        } else {
            v.iter()
                .map(|(k, c)| format!("{} ({}개 세션)", k, c))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };

    let mut d = String::new();
    d.push_str("# 크로스세션 추적 데이터 (객관적 사실, 최근 7일 세션 집계)\n");
    d.push_str(&format!(
        "세션 {}개 · 기간 {} ~ {}\n",
        a.n_sessions, a.span.0, a.span.1
    ));
    d.push_str(&format!(
        "짧은 추임새성 프롬프트(‘ㄱㄱ/계속/다음’ 등, 새 방향 없이 진행만): 전체 {}개 중 {}개 ({:.0}%)\n",
        a.total_prompts,
        a.total_short,
        pct(a.total_short, a.total_prompts)
    ));

    // The deterministic cards, numbered — so the model can (and must)
    // anchor its prose to them instead of re-deriving fuzzy claims.
    if !signals.is_empty() {
        d.push_str("\n# 핵심 신호 (코드가 추출, 사용자에게 카드로 그대로 표시됨)\n");
        for (i, s) in signals.iter().enumerate() {
            d.push_str(&format!("신호{}. [{}] {} — {}\n", i + 1, s.cat, s.stat, s.body));
        }
    }

    // Externalized (V4-12): the instruction template lives in a user-editable
    // file, materialized from DEFAULT_CROSS_INSTR on first run. Edit the file →
    // next "다시 분석" picks it up, no rebuild. The generated DATA block (`d`)
    // stays code-owned and is appended after the instruction.
    let instr = load_prompt_template("cross-coaching", DEFAULT_CROSS_INSTR);
    // 배치·근거는 build_prompt와 동일 — 지시 양끝, 데이터는 태그로 경계만.
    let wrapped = format!("<cross_data>\n{}</cross_data>", d);
    match persona {
        Some(p) => format!("{}\n\n{}\n\n{}{}", p, instr, wrapped, TONE_CODA),
        None => format!("{}\n\n{}{}", instr, wrapped, TONE_CODA),
    }
}

/// V4-12 강-티어: deterministic copy-paste draft for a gate-passed skill
/// candidate (What/Why/How/Impact + the actual command-file path & content).
/// No LLM — every field derives from the signal, so it can't hallucinate.
/// Avoids ``` fences: renderMarkdown has no fenced-code support and would show
/// the backticks literally; uses inline `code` + bullets instead.
fn skill_draft(head: &str, sessions: u32) -> String {
    let name = head.split_whitespace().collect::<Vec<_>>().join("-");
    format!(
        "\n\n## 바로 쓰기 (복붙)\n\
         - **What**: `{head}` 를 `/{name}` 슬래시 커맨드로 묶기\n\
         - **Why**: 최근 7일 {sessions}개 세션에서 반복했어요 — 하루 한 번 이상 반복하는 명령은 커맨드 후보예요.\n\
         - **How**: `~/.claude/commands/{name}.md` 파일을 만들고, 본문에 실행할 명령 `{head}` 한 줄을 넣어요. (원하면 맨 위에 `description:` 프론트매터 추가.)\n\
         - **Impact**: 매번 손으로 안 쳐도 돼요. 다음 집계에서 `{head}` 가 '스킬 후보'로 다시 뜨지 않는지 봐요.",
        head = head,
        name = name,
        sessions = sessions
    )
}

/// Distill the last `limit` sessions, aggregate, and coach on the
/// cross-session habits. Backend resolved from settings (local by default).
/// Returns the deterministic signal cards + the LLM-phrased body.
pub fn generate_cross(db: &Arc<Db>, limit: usize) -> Result<CrossCoaching> {
    // 2026-08-20: 창을 **이벤트 단위**로 바꿨다. 이전에는 시작 시각으로 세션을
    // 통째로 걸렀는데, 오래 켜두는 사용자는 이번 주 실작업이 전부 마라톤 세션
    // 안에 있어 코칭이 아무것도 못 봤다(실측: 7일 활동 36세션 중 11개 제외,
    // 훅 기준 하루 Edit 180~198건인데 덤프는 edits 0). 이제 세션은 **활동
    // 기준**으로 고르고 `distill_since`가 창 밖 이벤트를 잘라내므로, 오래된
    // 이벤트가 "이번 주"로 새는 일 없이 실작업이 보인다. spec §3.3.2.
    const WINDOW_DAYS: i64 = 7;
    let cutoff = Utc::now() - chrono::Duration::days(WINDOW_DAYS);
    let paths = recent_transcripts(WINDOW_DAYS as u64, 5, limit.max(40));
    if paths.is_empty() {
        return Err(anyhow!("최근 세션 transcript를 찾을 수 없습니다"));
    }
    let mut summaries: Vec<SessionSummary> = Vec::new();
    for p in &paths {
        // A single unparseable/huge transcript shouldn't sink the batch.
        if let Ok(trace) = distill_since(p, Some(cutoff)) {
            // 창 안에 실제 활동이 있는 세션만. 창 밖 세션은 잘라내고 나면 빈다.
            if !trace.prompts.is_empty() {
                summaries.push(summarize(&trace, p));
            }
        }
    }
    if summaries.len() < 2 {
        return Err(anyhow!(
            "집계할 세션이 부족합니다 (유효 세션 {}개) — 크로스세션 코칭은 최소 2개 필요",
            summaries.len()
        ));
    }
    // Chronological (oldest-first) for a trustworthy trend/span — mtime order
    // (how we picked "recent") isn't the same as session start order.
    summaries.sort_by(|a, b| a.started.cmp(&b.started));
    // 이벤트 창이 이미 적용됐으므로 시작 시각 재필터는 하지 않는다(그게 바로
    // 실작업을 통째로 날리던 단계). 개수만 캡.
    summaries.truncate(limit.max(2));
    if summaries.len() < 2 {
        return Err(anyhow!(
            "최근 {}일 활동 세션이 부족합니다 (유효 {}개) — 최소 2개 필요",
            WINDOW_DAYS,
            summaries.len()
        ));
    }

    let agg = aggregate(&summaries);
    // ⑦ Redundancy gate: read the encode-corpus once, keep the first ritual head
    // that ISN'T already a skill/command/CLAUDE.md rule/memory note. On a user
    // who codifies their rituals this correctly stays None most of the time.
    let corpus = redundancy_corpus();
    let skill = agg
        .skill_heads
        .iter()
        .find(|(head, _)| !already_encoded(head, &corpus));
    if let Some((h, c)) = skill {
        eprintln!("[retro] ⑦ skill candidate: '{h}' in {c} sessions (gate-passed)");
    }
    let signals = derive_signals(&agg, skill);

    // No notable signal (disciplined user: steers well, rituals already
    // encoded) — DON'T invoke the LLM. With nothing real to cite it fabricates
    // (it copied a prompt example's "23%" over the actual 0%). Return a fixed,
    // honest, positive body instead. Deterministic → zero hallucination risk.
    if signals.is_empty() {
        eprintln!("[retro] no notable signal → honest empty coaching (LLM skipped)");
        let body = "## 요약\n최근 세션들에서 뚜렷이 짚을 습관이 없어요. 방향도 잘 주고 있고, 자주 쓰는 명령도 이미 커맨드·스킬로 정리돼 있다는 뜻이에요.\n\n## 반복되는 패턴\n- 여러 세션에 걸쳐 반복되는 문제 패턴은 눈에 띄지 않아요.\n\n## 실험해볼 것\n- 지금 흐름을 그대로 유지해요. 새로 반복되는 명령 의식이 쌓이면 그때 '스킬 후보'로 짚어줄게요.\n\n## 관성 점검\n지금처럼 방향을 명확히 주는 흐름, 계속 이어갈 수 있을까요?".to_string();
        let coaching = CrossCoaching {
            n_sessions: agg.n_sessions,
            span_start: agg.span.0.clone(),
            span_end: agg.span.1.clone(),
            signals,
            body,
            generated_at: Local::now().to_rfc3339(),
        };
        save_cross(&coaching);
        return Ok(coaching);
    }

    let s = db.load_settings().unwrap_or_default();
    let persona = persona_block(&s.active_character);
    let prompt = build_cross_prompt(&agg, &signals, persona.as_deref());

    let backend = coach::Backend::resolve(db);
    eprintln!(
        "[retro] cross-session coaching: {} sessions, {} signals via {}",
        summaries.len(),
        signals.len(),
        backend.label()
    );
    let raw = coach::complete(&prompt, &backend, None)?;
    let mut body = strip_emoji(&normalize_headings(&raw, "## 요약"));
    // V4-12 강-티어: when a skill candidate fired (already gate-passed = worth
    // proposing), append a DETERMINISTIC copy-paste draft. Name/path/command
    // all come from the signal → no LLM → zero hallucination. Complements the
    // LLM's "실험해볼 것" bullet with the exact "how".
    if let Some((h, c)) = skill {
        body.push_str(&skill_draft(h, *c));
    }
    let coaching = CrossCoaching {
        n_sessions: agg.n_sessions,
        span_start: agg.span.0.clone(),
        span_end: agg.span.1.clone(),
        signals,
        body,
        generated_at: Local::now().to_rfc3339(),
    };
    // Snapshot to ~/.toki/coaching/ (best-effort) so the result survives a mode
    // switch / restart and can be diffed against future runs.
    save_cross(&coaching);
    Ok(coaching)
}

/// V5 딥 코칭용 — 최근 7일 트랜스크립트 집계를 마크다운 한 블록으로.
/// generate_cross와 같은 이벤트-창 수집(distill_since)을 타되, 신호 게이트 없이
/// **원자료 통계**를 그대로 내보낸다: 딥 코칭은 강한 모델이 직접 추론하므로
/// 게이트로 걸러줄 필요가 없고, 오히려 걸러낸 데이터(bash 의식 전체, rework
/// 상위 파일들)가 근거가 된다. 실패는 None — 딥 코칭은 히스토리만으로도 돈다.
pub(crate) fn deep_stats_markdown() -> Option<String> {
    const WINDOW_DAYS: i64 = 7;
    let cutoff = Utc::now() - chrono::Duration::days(WINDOW_DAYS);
    let paths = recent_transcripts(WINDOW_DAYS as u64, 5, 60);
    if paths.is_empty() {
        return None;
    }
    let mut summaries: Vec<SessionSummary> = Vec::new();
    for p in &paths {
        if let Ok(trace) = distill_since(p, Some(cutoff)) {
            if !trace.prompts.is_empty() {
                summaries.push(summarize(&trace, p));
            }
        }
    }
    if summaries.is_empty() {
        return None;
    }
    summaries.sort_by(|a, b| a.started.cmp(&b.started));
    let agg = aggregate(&summaries);

    let mut s = String::new();
    s.push_str(&format!(
        "세션 {}개 · 기간 {} ~ {}\n",
        agg.n_sessions, agg.span.0, agg.span.1
    ));
    if agg.total_prompts > 0 {
        s.push_str(&format!(
            "프롬프트 {}개, 그중 짧은 추임새('ㄱㄱ/계속' 류) {}개 ({:.0}%)\n",
            agg.total_prompts,
            agg.total_short,
            agg.total_short as f64 / agg.total_prompts as f64 * 100.0
        ));
    }
    if !agg.skill_heads.is_empty() {
        s.push_str("여러 세션에서 반복된 bash 의식 (검사·탐색 명령 제외):\n");
        for (head, c) in &agg.skill_heads {
            s.push_str(&format!("- `{}` — {}개 세션\n", head, c));
        }
    }
    if let Some((file, edits, reworks, chain, sess)) = &agg.thrash {
        s.push_str(&format!(
            "겉돎 후보 최상위: `{}` — 편집 {} 중 직전 결과 되돌림 {}, 최대 연속 {}회, {}개 세션\n",
            basename(file), edits, reworks, chain, sess
        ));
    }
    // 프로젝트 분포 — "어디에 시간이 갔나"는 새 자산 제안의 좋은 근거다.
    let projects = recent_projects(WINDOW_DAYS as u64, 8);
    if !projects.is_empty() {
        s.push_str("프로젝트별 세션 분포 (최근 7일):\n");
        for p in &projects {
            s.push_str(&format!("- {} — {}세션\n", p.label, p.sessions));
        }
    }
    Some(s)
}

/// Strip decorative emoji/pictographs (the design system is emoji-free —
/// pixel glyphs and unicode arrows/box shapes only). Removes the emoji &
/// dingbat planes but KEEPS geometric shapes (U+25xx: ▸◂■) and arrows
/// (U+21xx: ↗→) that the UI legitimately uses. Collapses the leftover space
/// where an emoji sat (e.g. "## 💡 실험" → "## 실험").
pub(crate) fn strip_emoji(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|&c| {
            let u = c as u32;
            !((0x1F000..=0x1FAFF).contains(&u)   // emoticons, pictographs, symbols-ext
                || (0x2600..=0x27BF).contains(&u) // misc symbols + dingbats (💡✅❌ live here)
                || (0x1F1E6..=0x1F1FF).contains(&u) // regional indicators
                || u == 0xFE0F                     // emoji variation selector
                || u == 0x200D)                    // zero-width joiner
        })
        .collect();
    // Tidy the gap emoji removal leaves, per line (don't touch newlines).
    cleaned
        .lines()
        .map(|line| {
            let mut out = String::with_capacity(line.len());
            let mut prev_space = false;
            for ch in line.chars() {
                let is_space = ch == ' ';
                if is_space && prev_space {
                    continue;
                }
                out.push(ch);
                prev_space = is_space;
            }
            out.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `~/.toki/coaching/` — where each cross-session analysis is snapshotted so
/// the user can reopen the last one instantly and diff runs over time.
pub(crate) fn coaching_dir() -> Option<PathBuf> {
    let d = dirs::home_dir()?.join(".toki").join("coaching");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

/// Human-readable snapshot for eyeball comparison across runs (the `.md`
/// companion to the canonical `.json`).
fn render_md_snapshot(c: &CrossCoaching) -> String {
    let mut s = String::new();
    s.push_str(&format!("# 크로스세션 코칭 · {}\n", c.generated_at));
    s.push_str(&format!("세션 {}개 · 기간 {} ~ {}\n\n", c.n_sessions, c.span_start, c.span_end));
    s.push_str("## 신호\n");
    for sig in &c.signals {
        s.push_str(&format!("- [{}] {} — {}\n", sig.cat, sig.stat, sig.body));
    }
    s.push_str("\n## 코치의 말\n");
    s.push_str(c.body.trim());
    s.push('\n');
    s
}

/// Persist a generated analysis: canonical `cross-<ts>.json` (the app reloads
/// this) + a `cross-<ts>.md` sibling (human diffable). Best-effort — a cache
/// write failure must never sink a successful coaching run.
fn save_cross(c: &CrossCoaching) {
    let Some(dir) = coaching_dir() else { return };
    let stamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    if let Ok(json) = serde_json::to_string_pretty(c) {
        let _ = std::fs::write(dir.join(format!("cross-{stamp}.json")), json);
    }
    let _ = std::fs::write(dir.join(format!("cross-{stamp}.md")), render_md_snapshot(c));
}

/// The most recent cached analysis (newest `cross-*.json` by mtime), or None
/// if coaching was never run. Backs `coaching_cross_latest` so re-entering the
/// coaching screen shows the last result instantly — no LLM re-run. File-based
/// (reads what `save_cross` wrote), so it's self-contained and needs no DB.
pub fn latest_cross() -> Option<CrossCoaching> {
    let dir = coaching_dir()?;
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for f in std::fs::read_dir(&dir).ok()?.flatten() {
        let p = f.path();
        // `deep-*.json`(딥 코칭, V5)이 같은 폴더에 살므로 prefix로 갈라야
        // 한다 — 딥 캐시가 더 최신이면 여기서 역직렬화가 실패해 "캐시 없음"으로
        // 오판하고, 재진입 즉시 표시 UX가 통째로 죽는다.
        let is_cross = p
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("cross-"))
            .unwrap_or(false);
        if is_cross && p.extension().and_then(|e| e.to_str()) == Some("json") {
            let m = f
                .metadata()
                .and_then(|md| md.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            if newest.as_ref().map(|(t, _)| m > *t).unwrap_or(true) {
                newest = Some((m, p));
            }
        }
    }
    let (_, p) = newest?;
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

/// EXAONE reliably copies the data block's title into its first heading
/// ("# 크로스세션 추적 데이터 요약") no matter how the instructions pin the
/// section names. Two formatting-only fixes:
///  1. The FIRST heading → "## 요약" (EXAONE echoes the data block's title).
///  2. Every OTHER heading → normalized to `## ` level. The model is
///     inconsistent (some sections come back as `# 반복되는 패턴`, single
///     hash), and renderMarkdown only styles `## `/`### ` — a lone `#` would
///     render as literal body text. Uniforming to `## ` keeps the section
///     hierarchy intact. Content lines are untouched.
fn normalize_headings(body: &str, first_heading: &str) -> String {
    let mut first = true;
    let normalized: Vec<String> = body
        .lines()
        .map(|l| {
            if l.trim_start().starts_with('#') {
                if first {
                    first = false;
                    return first_heading.to_string();
                }
                let text = l.trim_start().trim_start_matches('#').trim();
                return format!("## {}", text);
            }
            l.to_string()
        })
        .collect();
    // Drop a heading that duplicates the previous one (only blanks between) —
    // forcing the first heading can duplicate the model's own correct section
    // heading. Content between two identical headings keeps both.
    let mut out: Vec<String> = Vec::with_capacity(normalized.len());
    let mut last_heading: Option<String> = None;
    for line in normalized {
        if line.starts_with("## ") {
            if last_heading.as_deref() == Some(line.as_str()) {
                continue;
            }
            last_heading = Some(line.clone());
        } else if !line.trim().is_empty() {
            last_heading = None;
        }
        out.push(line);
    }
    out.join("\n")
}

#[cfg(test)]
mod quality_check {
    //! V4-6 quality gate helper — NOT a CI test. Runs the real pipeline
    //! (distill → build_prompt → local LLM) on a real transcript so we can
    //! eyeball whether the local model produces non-generic coaching.
    //!
    //! Usage:
    //!   TOKI_TEST_TRANSCRIPT=/path/to/session.jsonl \
    //!   TOKI_TEST_MODEL=exaone3.5:7.8b \
    //!   cargo test --lib coach_on_real_transcript -- --ignored --nocapture
    use super::*;

    #[test]
    fn normalize_headings_forces_sections() {
        let s = "# 크로스세션 추적 데이터 요약\n본문\n## 반복되는 패턴\n- x";
        assert_eq!(normalize_headings(s, "## 요약"), "## 요약\n본문\n## 반복되는 패턴\n- x");
        // Already-correct output passes through unchanged.
        let ok = "## 요약\na\n## 반복되는 패턴\nb";
        assert_eq!(normalize_headings(ok, "## 요약"), ok);
    }

    #[test]
    fn low_value_bash_filtered() {
        assert!(is_low_value_bash("cd app"));
        assert!(is_low_value_bash("ls -la"));
        assert!(!is_low_value_bash("cd app && npm run build"));
        assert!(!is_low_value_bash("npm run build"));
    }

    #[test]
    fn normalize_command_folds_ritual_families() {
        // Stable commands pass through unchanged.
        assert_eq!(normalize_command("git push origin main"), "git push origin main");
        // Per-session paths collapse → one family.
        assert_eq!(normalize_command("tail /private/tmp/abc-123/x.output"), "tail <path>");
        assert_eq!(
            normalize_command("tail /private/tmp/def-999/y.log"),
            normalize_command("tail /private/tmp/abc-123/x.output"),
            "different tmp paths must be the same family",
        );
        // Pipe/redirect structure kept, trailing number generalized.
        assert_eq!(normalize_command("npm test 2>&1 | tail -30"), "npm test 2>&1 | tail -<n>");
        // Quoted message collapses so commits don't fragment by wording.
        assert_eq!(normalize_command("git commit -m \"fix: the bug 42\""), "git commit -m <str>");
        assert_eq!(
            normalize_command("git commit -m \"totally different words\""),
            normalize_command("git commit -m \"fix: the bug 42\""),
        );
        // Relative path arg also folds.
        assert_eq!(normalize_command("rm -rf build/xyz"), "rm -rf <path>");
        // Compound survives (not low-value) and both halves normalize.
        assert_eq!(
            normalize_command("cd app && npx tauri build"),
            "cd app && npx tauri build",
        );
    }

    /// V4-13 (B) dump-before-DESIGN. Per-file across recent-7d sessions:
    /// net edit growth (Σ len new − len old), edit count, reread-after-edit,
    /// #sessions. Control = TamagotchiShell.tsx (known healthy iteration →
    /// should show large +net). Question: is there any file with many edits
    /// across ≥2 sessions but FLAT/NEGATIVE net (= rewriting = thrash)? If none
    /// separates from the iteration baseline, B is deferred (no data).
    ///   cargo test --lib dump_edit_growth -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_edit_growth() {
        let cutoff = Utc::now() - chrono::Duration::days(7);
        let paths = recent_transcripts(7, 5, 60);
        // file → (net_growth, edit_count, reread_count, #sessions)
        let mut agg: HashMap<String, (i64, u32, u32, u32)> = HashMap::new();
        let mut n = 0;
        for p in &paths {
            // 실제 경로와 동일하게 **이벤트 단위** 창을 쓴다 — 시작 시각으로
            // 거르면 마라톤 세션의 이번 주 실작업이 통째로 빠진다(§3.3.2).
            let Ok(tr) = distill_since(p, Some(cutoff)) else { continue };
            if tr.prompts.is_empty() { continue } // 사람 프롬프트 0 = 봇 세션
            n += 1;
            let edits: HashMap<&str, u32> = tr.top_edits.iter().map(|(k, c)| (k.as_str(), *c)).collect();
            let reread: HashMap<&str, u32> = tr.reread_after_edit.iter().map(|(k, c)| (k.as_str(), *c)).collect();
            for (file, net) in &tr.edit_growth {
                let e = agg.entry(file.clone()).or_insert((0, 0, 0, 0));
                e.0 += *net;
                e.1 += edits.get(file.as_str()).copied().unwrap_or(0);
                e.2 += reread.get(file.as_str()).copied().unwrap_or(0);
                e.3 += 1;
            }
        }
        eprintln!("\n=== {n} recent-7d sessions · per-file edit growth ===");
        eprintln!("(iterate=큰 +net · thrash 후보=편집 많고 ≥2세션인데 net flat/음수)");
        eprintln!("{:>4} {:>6} {:>8} {:>6} | file", "sess", "edits", "net", "rerd");
        let mut v: Vec<_> = agg.into_iter().collect();
        v.sort_by(|a, b| b.1.1.cmp(&a.1.1).then(a.0.cmp(&b.0)));
        for (file, (net, edits, reread, sess)) in v.into_iter().take(25) {
            eprintln!("{sess:>4} {edits:>6} {net:>+8} {reread:>6} | {file}");
        }
    }

    /// §3.3.2 B 3차 dump-before-wire (2026-08-18) — CodeBurn retry proxy +
    /// §3.3.4 category denominator, on the REAL corpus. Question: do thrash
    /// candidates (high retry share) separate from healthy iteration (many
    /// edits, few retries)? net-growth (dump_edit_growth) said "no thrash";
    /// this axis looks at the edit→verify→re-edit loop instead. Read BEFORE
    /// wiring any card. Window: TOKI_DUMP_DAYS (default 30).
    ///   cargo test --lib dump_retry_categories -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_retry_categories() {
        let days: u64 = std::env::var("TOKI_DUMP_DAYS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);
        let cutoff = Utc::now() - chrono::Duration::days(days as i64);
        let paths = recent_transcripts(days, 5, 500);
        // path → (edits, retries, reworks, max chain, #sessions)
        let mut files: HashMap<String, (u32, u32, u32, u32, u32)> = HashMap::new();
        let mut cats: HashMap<&'static str, u32> = HashMap::new();
        let mut sess_rows: Vec<(String, u32, u32, u32)> = Vec::new(); // date, edits, retries, reworks
        let (mut tot_edits, mut tot_retries, mut tot_rework, mut n) = (0u32, 0u32, 0u32, 0usize);
        for p in &paths {
            let Ok(tr) = distill_since(p, Some(cutoff)) else { continue };
            // 실제 경로와 동일한 이벤트 단위 창(§3.3.2) — 시작 시각으로 거르면
            // 마라톤 세션의 이번 주 실작업이 통째로 빠진다.
            if tr.prompts.is_empty() { continue } // 사람 프롬프트 0 = 봇 세션
            n += 1;
            tot_edits += tr.edit_cycles;
            tot_retries += tr.retries;
            tot_rework += tr.reworks;
            for (f, e, r, w, c) in &tr.retry_files {
                let x = files.entry(f.clone()).or_insert((0, 0, 0, 0, 0));
                x.0 += e;
                x.1 += r;
                x.2 += w;
                x.3 = x.3.max(*c);
                x.4 += 1;
            }
            for (c, k) in &tr.category_counts { *cats.entry(c).or_insert(0) += k; }
            let date: String = tr.first_ts.as_deref().unwrap_or("").chars().take(10).collect();
            sess_rows.push((date, tr.edit_cycles, tr.retries, tr.reworks));
        }
        let osr = |e: u32, r: u32| if e == 0 { 100.0 } else { 100.0 * (e - r) as f64 / e as f64 };
        eprintln!("\n=== {n} sessions ({days}d) · B 4차: 파일 단위(rtry) vs 영역 단위(rwrk) ===");
        eprintln!("totals: edits {tot_edits} · retries {tot_retries} ({:.0}%) · reworks {tot_rework} ({:.0}%) · one-shot {:.0}%",
            100.0 * tot_retries as f64 / tot_edits.max(1) as f64,
            100.0 * tot_rework as f64 / tot_edits.max(1) as f64,
            osr(tot_edits, tot_rework));
        eprintln!("(rwrk% = 내가 쓴 걸 다시 먹은 비율 = 겉돎 후보 · chain = 같은 자리 연속 N회차)");
        eprintln!("{:>4} {:>5} {:>6} {:>6} {:>5} | file", "sess", "edits", "rtry%", "rwrk%", "chain");
        let mut v: Vec<_> = files.into_iter().collect();
        v.sort_by(|a, b| b.1.2.cmp(&a.1.2).then(b.1.0.cmp(&a.1.0)).then(a.0.cmp(&b.0)));
        let pct = |x: u32, e: u32| 100.0 * x as f64 / e.max(1) as f64;
        for (f, (e, r, w, c, s)) in v.into_iter().take(22) {
            let short: String = f.rsplit('/').take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("/");
            eprintln!("{s:>4} {e:>5} {:>5.0}% {:>5.0}% {c:>5} | {short}", pct(r, e), pct(w, e));
        }
        let total: u32 = cats.values().sum();
        eprintln!("\n=== §3.3.4 category distribution ({total} assistant lines) ===");
        let mut cv: Vec<_> = cats.into_iter().collect();
        cv.sort_by(|a, b| b.1.cmp(&a.1));
        for (c, k) in cv {
            eprintln!("{k:>6} {:>5.1}% | {c}", 100.0 * k as f64 / total.max(1) as f64);
        }
        sess_rows.sort_by(|a, b| b.3.cmp(&a.3));
        eprintln!("\n=== top sessions by reworks ===");
        eprintln!("{:>10} {:>5} {:>7} {:>7} {:>6}", "date", "edits", "retries", "reworks", "1shot");
        for (d, e, r, w) in sess_rows.into_iter().take(10) {
            eprintln!("{d:>10} {e:>5} {r:>7} {w:>7} {:>5.0}%", osr(e, w));
        }
    }

    /// §3.3.2/§3.3.4 gate — retry cycle, living-doc guard, category priority
    /// on a synthetic transcript.
    #[test]
    fn retry_cycles_and_categories() {
        let tool = |name: &str, input: serde_json::Value| {
            serde_json::json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "name": name, "input": input}
            ]}})
        };
        let edit = |fp: &str| tool("Edit", serde_json::json!({"file_path": fp, "old_string": "a", "new_string": "ab"}));
        let bash = |cmd: &str| tool("Bash", serde_json::json!({"command": cmd}));
        let long_edit = |fp: &str, old: &str, new: &str| tool(
            "Edit",
            serde_json::json!({"file_path": fp, "old_string": old, "new_string": new}),
        );
        let lines = vec![
            edit("/w/foo.rs"),                                     // coding · foo edit#1 (seq 0)
            bash("cargo test --lib"),                              // testing · seq→1
            edit("/w/foo.rs"),                                     // coding · retry #1 (1 > 0)
            edit("/w/foo.rs"),                                     // coding · no retry (seq unchanged)
            edit("/w/plan.md"),                                    // coding · living doc — retry 제외
            bash("git push origin main"),                          // gitops · seq→2
            edit("/w/plan.md"),                                    // coding · living doc — still excluded
            serde_json::json!({"type": "user", "message": {"content": [
                {"type": "tool_result", "is_error": true}
            ]}}),
            edit("/w/foo.rs"),                                     // debugging (err pending) · retry #2 (2 > 1)
            tool("Read", serde_json::json!({"file_path": "/w/foo.rs"})), // exploration
            serde_json::json!({"type": "assistant", "message": {"content": [
                {"type": "text", "text": "설명"}
            ]}}),                                                  // conversation
            tool("Task", serde_json::json!({"prompt": "go"})),     // delegation
            // ── B 4차: 영역 단위 rework + 연속 N회차 ──
            long_edit("/w/bar.ts", "", "fn alpha() { return 1; }"),                        // 앵커만 (old 없음)
            long_edit("/w/bar.ts", "fn alpha() { return 1; }", "fn alpha() { return 2; }"), // rework #1 · chain 1
            long_edit("/w/bar.ts", "fn alpha() { return 2; }", "fn alpha() { return 3; }"), // rework #2 · chain 2
            long_edit("/w/bar.ts", "const somethingElse = 42;", "const somethingElse = 43;"), // 다른 영역 → chain 0
            // ── 구멍① 회귀: 같은 basename·다른 경로는 retry가 아니다 ──
            edit("/w/a/page.tsx"),
            bash("cargo test --lib"),
            edit("/w/b/page.tsx"),
        ];
        let path = std::env::temp_dir().join("toki-retry-fixture.jsonl");
        let body: String = lines.iter().map(|l| l.to_string() + "\n").collect();
        std::fs::write(&path, body).expect("fixture write");
        let t = distill(&path).expect("distill");
        // foo ×4 + bar ×4 + page ×2 = 10 (plan.md는 문서라 제외)
        assert_eq!(t.edit_cycles, 10, "docs must not count as edit cycles");
        // 구멍②: `git push`는 검증이 아니므로 seq를 올리지 않는다 → foo 재편집은
        // retry가 아니다. 검증(cargo test) 뒤의 재편집 1건만 retry.
        assert_eq!(t.retries, 1, "only verification bash may trigger a retry");
        // 구멍①: a/page.tsx → 검증 → b/page.tsx 는 서로 다른 파일이라 retry 아님
        let by_path: HashMap<&str, (u32, u32, u32, u32)> = t
            .retry_files
            .iter()
            .map(|(f, e, r, w, c)| (f.as_str(), (*e, *r, *w, *c)))
            .collect();
        assert_eq!(by_path.get("/w/a/page.tsx"), Some(&(1, 0, 0, 0)));
        assert_eq!(by_path.get("/w/b/page.tsx"), Some(&(1, 0, 0, 0)));
        assert_eq!(by_path.get("/w/foo.rs"), Some(&(4, 1, 0, 0)), "짧은 문자열은 rework 앵커가 아니다");
        // B 4차: bar.ts는 자기 출력을 2연속 다시 먹었고(chain 2), 마지막 편집은
        // 다른 영역이라 rework가 아니다.
        assert_eq!(by_path.get("/w/bar.ts"), Some(&(4, 0, 2, 2)));
        assert_eq!(t.reworks, 2);
        let cats: HashMap<&str, u32> = t.category_counts.iter().cloned().collect();
        assert_eq!(cats.get("coding"), Some(&11)); // foo ×3 + plan.md ×2 + bar ×4 + page ×2
        assert_eq!(cats.get("debugging"), Some(&1));
        assert_eq!(cats.get("testing"), Some(&2));
        assert_eq!(cats.get("gitops"), Some(&1));
        assert_eq!(cats.get("exploration"), Some(&1));
        assert_eq!(cats.get("conversation"), Some(&1));
        assert_eq!(cats.get("delegation"), Some(&1));
    }

    /// A/V4-14 quality gate — hardened single-session retro on the newest
    /// transcript. Eyeball: 해요체 통일·이모지 없음·프롬프트 번호 인용·마찰
    /// 없으면 "막힘 없이"·900자 안·예시 누출 없음.
    ///   cargo test --lib session_retro_on_latest -- --ignored --nocapture
    #[test]
    #[ignore]
    fn session_retro_on_latest() {
        let path = recent_transcripts(1, 1, 1).into_iter().next().expect("no transcript");
        eprintln!("=== latest session: {} ===", path.display());
        let trace = distill(&path).expect("distill");
        eprintln!("prompts {}, turns {}, errors {}, interrupts {}",
            trace.prompts.len(), trace.n_turns, trace.errors, trace.interrupts);
        let persona = std::env::var("TOKI_TEST_PERSONA").ok().and_then(|id| persona_block(&id));
        let prompt = build_prompt(&trace, persona.as_deref());
        let model = std::env::var("TOKI_TEST_MODEL").unwrap_or_else(|_| coach::DEFAULT_OLLAMA_MODEL.to_string());
        let backend = coach::Backend::Ollama { model };
        let raw = coach::complete(&prompt, &backend, None).expect("coach");
        let body = append_cited_prompts(
            &strip_emoji(&normalize_headings(&raw, "## 이번 세션 한 줄")),
            &trace.prompts,
        );
        eprintln!("\n=== session retro ({} chars) ===\n{}\n=== end ===", body.chars().count(), body);
        assert!(!body.is_empty());
    }

    #[test]
    #[ignore]
    fn dump_recent_projects() {
        let ps = recent_projects(7, 20);
        eprintln!("\n=== {} recent projects (7d) ===", ps.len());
        for p in &ps {
            eprintln!("  {:>2} sessions · {:<16} · {} · dir={}", p.sessions, p.label, p.last_active, p.dir);
        }
    }

    /// Picker/generation consistency — every project the picker lists must
    /// yield ≥1 session from the generation-side filter (they now share the
    /// activity-based criterion; start-date filtering used to break this for
    /// long-running sessions).
    ///   cargo test --lib picker_generation_window_consistency -- --ignored --nocapture
    #[test]
    #[ignore]
    fn picker_generation_window_consistency() {
        let ps = recent_projects(7, 20);
        assert!(!ps.is_empty(), "picker returned no projects — nothing to check");
        for p in &ps {
            let traces = project_recent_traces(&p.dir, 7);
            eprintln!("  {:<16} picker={} sessions, generator={} traces", p.label, p.sessions, traces.len());
            assert!(
                !traces.is_empty(),
                "'{}' (dir={}) is listed by the picker but the generator finds no sessions",
                p.label, p.dir
            );
        }
    }

    #[test]
    fn append_cited_prompts_lists_referenced_text() {
        let prompts = vec![
            "첫 지시".to_string(),
            "두번째".to_string(),
            "사내 배포용 Tauri 앱 패키징 요청".to_string(),
        ];
        let body = "본문에서 **프롬프트 1**과 프롬프트 3을 인용했어요. 프롬프트 초기 단계는 번호 아님.";
        let out = append_cited_prompts(body, &prompts);
        assert!(out.contains("## 언급한 프롬프트"));
        assert!(out.contains("**프롬프트 1**: \"첫 지시\""));
        assert!(out.contains("**프롬프트 3**: \"사내 배포용 Tauri 앱 패키징 요청\""));
        assert!(!out.contains("프롬프트 2")); // not cited → not listed
        // No citations → unchanged.
        assert_eq!(append_cited_prompts("인용 없음", &prompts), "인용 없음");
    }

    #[test]
    fn skill_draft_is_deterministic_and_grounded() {
        let d = skill_draft("cargo check", 3);
        assert!(d.contains("## 바로 쓰기"));
        assert!(d.contains("`/cargo-check`"));              // hyphenated command name
        assert!(d.contains("~/.claude/commands/cargo-check.md")); // concrete path
        assert!(d.contains("3개 세션"));                    // grounded session count
        assert!(d.contains("`cargo check`"));               // the actual command
        assert!(!d.contains("```"));                        // no fenced code (renderMarkdown-safe)
    }

    #[test]
    fn trim_josa_folds_particles() {
        assert_eq!(trim_josa("한국어가"), "한국어");
        assert_eq!(trim_josa("한국어를"), "한국어");
        assert_eq!(trim_josa("한국어로"), "한국어");
        assert_eq!(trim_josa("프롬프트로"), "프롬프트");
        assert_eq!(trim_josa("이미지"), "이미지"); // no false trim
        assert_eq!(trim_josa("git"), "git");
    }

    /// DUMP (not an assertion gate) — V4-12 detection-first. Prints, on the
    /// REAL recent-7d corpus: ⑦ normalized bash families by #sessions (mark
    /// ≥3 + redundancy-gate decision) and ⑧ correction tokens by #sessions
    /// (mark ≥2). Read this BEFORE wiring cards to confirm the proxies fire
    /// with stable keys and meaningful content (advisor: dump-before-wire).
    ///   cargo test --lib dump_action_signals -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_action_signals() {
        let paths = recent_transcripts(7, 5, 60);
        let mut summaries: Vec<SessionSummary> = Vec::new();
        for p in &paths {
            if let Ok(tr) = distill_since(p, Some(Utc::now() - chrono::Duration::days(7))) {
                if !tr.prompts.is_empty() {
                    summaries.push(summarize(&tr, p));
                }
            }
        }
        summaries.sort_by(|a, b| a.started.cmp(&b.started));
        let summaries = keep_recent_by_start(summaries, 7, 5, 40);
        eprintln!("\n=== {} sessions in window ===", summaries.len());

        let count_across = |pick: &dyn Fn(&SessionSummary) -> &Vec<String>| -> Vec<(String, u32)> {
            let mut m: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
            for s in &summaries {
                let mut seen = std::collections::HashSet::new();
                for k in pick(s) {
                    if seen.insert(k.clone()) {
                        *m.entry(k.clone()).or_insert(0) += 1;
                    }
                }
            }
            let mut v: Vec<(String, u32)> = m.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            v
        };

        // Is "morning is rougher" a real time effect, or just longer sessions?
        // Raw error/session vs error/turn (length-normalized) by daypart.
        eprintln!("=== daypart: raw err/session vs err/turn (length-normalized) ===");
        let mut dp: std::collections::HashMap<&str, (u32, u32, u32)> = std::collections::HashMap::new(); // (n, err, turns)
        for s in &summaries {
            if let Some(h) = s.local_hour {
                let e = dp.entry(daypart(h)).or_insert((0, 0, 0));
                e.0 += 1; e.1 += s.errors; e.2 += s.turns;
            }
        }
        for d in ["새벽(0-5)", "오전(6-11)", "오후(12-17)", "저녁·밤(18-23)"] {
            if let Some((n, err, turns)) = dp.get(d) {
                let per_sess = *err as f64 / *n as f64;
                let per_turn = if *turns > 0 { *err as f64 / *turns as f64 * 100.0 } else { 0.0 };
                eprintln!("  {d}: {n}세션 · 평균턴 {:.0} · 에러/세션 {per_sess:.1} · 에러/100턴 {per_turn:.1}", *turns as f64 / *n as f64);
            }
        }
        eprintln!();

        let corpus = redundancy_corpus();
        eprintln!("[redundancy corpus] {} chars\n", corpus.len());

        // ⑦ counted by ritual HEAD (first 2 significant tokens) — the ritual
        // identity, robust to the compound one-liners that make full strings
        // unique per session.
        let head_sessions = {
            let mut m: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
            for s in &summaries {
                let mut seen = std::collections::HashSet::new();
                for fam in &s.bash_families {
                    if let Some(h) = family_head(fam) {
                        if seen.insert(h.clone()) {
                            *m.entry(h).or_insert(0) += 1;
                        }
                    }
                }
            }
            let mut v: Vec<(String, u32)> = m.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            v
        };
        eprintln!("=== ⑦ ritual HEADS by #sessions (after inspect/gate filters) ===");
        for (head, c) in head_sessions.into_iter().take(30) {
            let insp = is_inspect_head(&head);
            let mark = if c >= 3 && !insp { "★≥3" } else if c >= 3 { "ins" } else { "   " };
            let gated = if c >= 3 && !insp && already_encoded(&head, &corpus) { " [ENCODED]" } else { "" };
            eprintln!("  {mark} {c:>2} | {head}{gated}");
        }

        eprintln!("\n=== ⑧ correction tokens by #sessions ===");
        for (tok, c) in count_across(&|s| &s.corrections).into_iter().take(20) {
            let mark = if c >= 2 { "★≥2" } else { "   " };
            eprintln!("  {mark} {c:>2} | {tok}");
        }
        eprintln!("\n(sessions with ≥1 correction prompt: {})",
            summaries.iter().filter(|s| !s.corrections.is_empty()).count());

        // VERBATIM correction prompts — the real ground truth for ⑧. Are these
        // actual redirections? Does a theme repeat a human/LLM could name?
        eprintln!("\n=== ⑧ verbatim correction prompts (is_correction=true) ===");
        for (i, p) in paths.iter().enumerate() {
            if let Ok(tr) = distill(p) {
                for pr in tr.prompts.iter().filter(|p| is_correction(p)) {
                    let head: String = pr.chars().take(90).collect();
                    eprintln!("  s{i:02} | {head}");
                }
            }
        }
    }

    #[test]
    fn living_docs_excluded_from_friction() {
        // planning/living docs — edited every session by design
        for f in ["plan.md", "plan-v4.md", "spec.md", "toki.md", "README.md",
                  "MEMORY.md", "CHANGELOG.md", "improvements.md", "CLAUDE.md"] {
            assert!(is_living_doc(f), "{f} should be a living doc");
        }
        // real code/content files — friction repetition still counts
        for f in ["retro.rs", "TamagotchiShell.tsx", "main.tsx", "db.rs", "index.html"] {
            assert!(!is_living_doc(f), "{f} should NOT be excluded");
        }
    }

    /// 실력 추세 실측 — UI가 진입 때마다 호출하므로 **속도**도 같이 본다.
    ///   cargo test --lib dump_skill_trend -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_skill_trend() {
        let t0 = std::time::Instant::now();
        let (now, prev) = skill_trend();
        let el = t0.elapsed();
        let row = |n: &str, w: &SkillWindow| {
            eprintln!(
                "{n:>6} | 세션 {:>3} · 편집 {:>4} · 되돌림 {:>3} · one-shot {:>5.1}% · 위임 {:>4.1}% ({}/{}) · 최다연속 {}",
                w.sessions, w.edits, w.reworks, w.one_shot, w.delegation, w.short_prompts, w.prompts, w.max_chain
            );
        };
        eprintln!("\n=== 실력 추세 (계산 {:?}) ===", el);
        row("이번주", &now);
        row("지난주", &prev);
        eprintln!("one-shot 변화: {:+.1}%p · 위임 변화: {:+.1}%p",
            now.one_shot - prev.one_shot, now.delegation - prev.delegation);
        assert!(el.as_secs() < 30, "너무 느리면 UI가 못 쓴다");
    }

    /// ⑨가 **회고 프롬프트 데이터 블록**까지 실리는지 — 신호가 코칭에만 있고
    /// 회고에 없으면 창(window)이 정상인 쪽에서 못 쓴다(spec §3.3.2 후속).
    /// rework 0이면 줄 자체가 없어야 한다: 없는 마찰을 제시하면 LLM이 지어낸다.
    #[test]
    fn retro_prompt_carries_rework() {
        let mut t = SessionTrace {
            n_turns: 3,
            edit_cycles: 40,
            reworks: 9,
            retry_files: vec![("/w/css/style.css".into(), 40, 18, 9, 3)],
            ..Default::default()
        };
        let p = build_prompt(&t, None);
        assert!(p.contains("전체 편집 40개 중 9개"), "rework 수치가 데이터에 없음");
        assert!(p.contains("style.css(되돌림 9회, 최다 연속 3회)"), "파일·연속 회차가 없음");
        t.reworks = 0;
        t.retry_files.clear();
        assert!(
            !build_prompt(&t, None).contains("되돌린 편집"),
            "rework 0이면 줄을 넣지 말 것"
        );
    }

    /// 프로젝트 회고는 세션 여럿을 합친다 — rework도 파일 단위로 합산되고,
    /// 연속 회차(chain)는 합이 아니라 **최댓값**이어야 한다.
    #[test]
    fn merge_traces_sums_rework_keeps_max_chain() {
        let mk = |w: u32, c: u32| SessionTrace {
            n_turns: 1,
            edit_cycles: 10,
            reworks: w,
            retry_files: vec![("/w/a.ts".into(), 10, 5, w, c)],
            ..Default::default()
        };
        let m = merge_traces(&[mk(4, 2), mk(3, 5)]);
        assert_eq!(m.reworks, 7);
        assert_eq!(m.edit_cycles, 20);
        assert_eq!(m.retry_files, vec![("/w/a.ts".to_string(), 20, 10, 7, 5)]);
    }

    /// ⑨ 겉도는 편집 게이트 — 영역 단위 rework가 **연속 3회 이상**일 때만 카드가
    /// 뜬다. 실측(30일)에서 rework는 전체 편집의 8%뿐이라 흔한 일을 짚으면
    /// 코칭이 못 쓰는 소리가 된다(§3.3.2 B 4차 판정 근거).
    #[test]
    fn thrash_signal_gate() {
        let with = |chain: u32, reworks: u32| {
            let mut s = summary_started(1);
            s.rework_files = vec![("/w/style.css".into(), 40, reworks, chain)];
            let a = aggregate(&[s]);
            derive_signals(&a, None)
                .into_iter()
                .find(|c| c.cat == "겉도는 편집")
        };
        // 연속 3회 + rework 3건 → 발화, 파일명·수치가 카드에 그대로
        let fired = with(3, 9).expect("chain 3 must fire");
        assert_eq!(fired.stat, "3회 연속");
        assert!(fired.body.contains("style.css") && fired.body.contains("40개 중 9개"));
        // 경계 아래는 침묵 — 두 조건 모두 필요
        assert!(with(2, 9).is_none(), "chain 2 is normal iteration");
        assert!(with(3, 2).is_none(), "too few reworks to be worth a card");
        // rework 0인 파일은 애초에 후보로 올라오지 않는다
        let mut clean = summary_started(1);
        clean.rework_files = vec![];
        assert!(aggregate(&[clean]).thrash.is_none());
    }

    fn summary_started(days_ago: i64) -> SessionSummary {
        let ts = (Utc::now() - chrono::Duration::days(days_ago)).to_rfc3339();
        SessionSummary {
            project: "p".into(), started: Some(ts), local_hour: None, dow: None,
            date: None, prompts: 0, short_prompts: 0, errors: 0, interrupts: 0,
            turns: 1, repeated_bash: vec![], heavy_edits: vec![],
            bash_families: vec![], corrections: vec![], rework_files: vec![],
        }
    }

    #[test]
    fn keep_recent_prefers_7day_window() {
        // 6 sessions in-window (0..6d) + 3 old (20,40,60d), oldest-first.
        let mut v: Vec<SessionSummary> =
            [60, 40, 20, 6, 5, 4, 3, 2, 1].iter().map(|d| summary_started(*d)).collect();
        v.sort_by(|a, b| a.started.cmp(&b.started));
        let kept = keep_recent_by_start(v, 7, 5, 20);
        // ≥5 started within 7d → old ones dropped, only the 6 recent remain.
        assert_eq!(kept.len(), 6);
    }

    #[test]
    fn keep_recent_relaxes_when_quiet() {
        // Only 2 in-window, but 8 total → relax to the 5 most-recent by start.
        let mut v: Vec<SessionSummary> =
            [90, 60, 40, 30, 20, 15, 5, 2].iter().map(|d| summary_started(*d)).collect();
        v.sort_by(|a, b| a.started.cmp(&b.started));
        let kept = keep_recent_by_start(v, 7, 5, 20);
        assert_eq!(kept.len(), 5);
    }

    /// Exercises the real file-cache write path against ~/.toki/coaching/ so
    /// we can confirm the .json + .md snapshots actually land on disk and read
    /// the .md format back. Side-effecting (writes real files), hence #[ignore].
    ///   cargo test --lib cache_files_written -- --ignored --nocapture
    #[test]
    #[ignore]
    fn cache_files_written() {
        let c = CrossCoaching {
            n_sessions: 4, span_start: "07/09".into(), span_end: "07/10".into(),
            signals: vec![CoachSignal { cat: "추세".into(), stat: "0.6→1.3".into(), body: "테스트.".into() }],
            body: "## 요약\n캐시 쓰기 테스트.".into(),
            generated_at: Local::now().to_rfc3339(),
        };
        save_cross(&c);
        let dir = coaching_dir().expect("coaching dir");
        let jsons: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten()
            .filter(|f| f.path().extension().and_then(|e| e.to_str()) == Some("json")).collect();
        let mds: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten()
            .filter(|f| f.path().extension().and_then(|e| e.to_str()) == Some("md")).collect();
        eprintln!("coaching dir: {}", dir.display());
        eprintln!("json files: {}, md files: {}", jsons.len(), mds.len());
        assert!(!jsons.is_empty() && !mds.is_empty(), "both .json and .md written");
        // The fix: latest_cross() must read the newest json back (this is what
        // the coaching screen calls on re-entry instead of re-analyzing).
        let loaded = latest_cross().expect("latest_cross returns the cached run");
        eprintln!("latest_cross → n_sessions={}, generated_at={}", loaded.n_sessions, loaded.generated_at);
        assert_eq!(loaded.span_start, "07/09");
        assert!(loaded.body.contains("캐시 쓰기 테스트"));
    }

    #[test]
    fn md_snapshot_is_human_readable() {
        let c = CrossCoaching {
            n_sessions: 3,
            span_start: "07/08".into(),
            span_end: "07/10".into(),
            signals: vec![CoachSignal {
                cat: "추세".into(),
                stat: "0.6→1.3".into(),
                body: "악화 추세.".into(),
            }],
            body: "## 요약\n본문.".into(),
            generated_at: "2026-07-10T10:00:00+09:00".into(),
        };
        let md = render_md_snapshot(&c);
        assert!(md.starts_with("# 크로스세션 코칭 · 2026-07-10T10:00:00+09:00"));
        assert!(md.contains("세션 3개 · 기간 07/08 ~ 07/10"));
        assert!(md.contains("- [추세] 0.6→1.3 — 악화 추세."));
        assert!(md.contains("## 코치의 말\n## 요약\n본문."));
    }

    #[test]
    fn strip_emoji_drops_pictographs_keeps_glyphs() {
        // section heading with an emoji → emoji gone, gap collapsed
        assert_eq!(strip_emoji("## 💡 실험해볼 것"), "## 실험해볼 것");
        assert_eq!(strip_emoji("## 🔁 관성 점검"), "## 관성 점검");
        // inline emoji removed, prose intact
        assert_eq!(strip_emoji("잘했어요 ✅ 다음은?"), "잘했어요 다음은?");
        // geometric shapes / arrows the UI uses are KEPT
        assert_eq!(strip_emoji("▸ 항목 → 결과 ◂"), "▸ 항목 → 결과 ◂");
        // plain Korean untouched
        assert_eq!(strip_emoji("추임새 23%"), "추임새 23%");
    }

    #[test]
    fn continuation_tightened_to_two_chars() {
        // bare continuation — counts as delegation inertia
        for p in ["ㄱㄱ", "ㅇㅇ", "계속", "다음", "go", "ok"] {
            assert!(is_continuation(p), "{p} should be continuation");
        }
        // 3-char real instructions — must NOT be miscounted (P2-9 fix)
        for p in ["고쳐줘", "빌드해", "지워줘"] {
            assert!(!is_continuation(p), "{p} is a real instruction, not filler");
        }
    }

    #[test]
    #[ignore]
    fn coach_on_real_transcript() {
        let path = std::env::var("TOKI_TEST_TRANSCRIPT")
            .expect("set TOKI_TEST_TRANSCRIPT=/path/to/session.jsonl");
        let model = std::env::var("TOKI_TEST_MODEL").unwrap_or_else(|_| coach::DEFAULT_OLLAMA_MODEL.to_string());
        let trace = distill(&PathBuf::from(&path)).expect("distill failed");
        eprintln!(
            "\n=== distilled: {} prompts, {} turns, {} errors, {} interrupts ===",
            trace.prompts.len(), trace.n_turns, trace.errors, trace.interrupts
        );
        // Spot-check grounding: print the numbered prompts EXAONE cites so we
        // can confirm "프롬프트 9/12/17" reference real, on-topic prompts.
        for (i, p) in trace.prompts.iter().enumerate() {
            let head: String = p.chars().take(70).collect();
            eprintln!("  {}. {}", i + 1, head);
        }
        let persona = std::env::var("TOKI_TEST_PERSONA").ok().and_then(|id| persona_block(&id));
        let prompt = build_prompt(&trace, persona.as_deref());
        let backend = coach::Backend::Ollama { model };
        let t0 = std::time::Instant::now();
        let body = coach::complete(&prompt, &backend, None).expect("coach failed");
        eprintln!("\n=== {} ({:?}) ===\n{}\n=== end ===\n", backend.label(), t0.elapsed(), body);
        assert!(!body.is_empty());
    }

    /// Cross-session (§3.2) quality gate — distill the real recent sessions,
    /// aggregate, and coach on the habits. Prints the aggregated data block
    /// (so we can confirm the numbers are grounded) and the LLM output.
    ///
    /// Usage:
    ///   TOKI_TEST_LIMIT=20 TOKI_TEST_MODEL=exaone3.5:7.8b \
    ///   cargo test --lib cross_on_real_sessions -- --ignored --nocapture
    #[test]
    #[ignore]
    fn cross_on_real_sessions() {
        let limit: usize = std::env::var("TOKI_TEST_LIMIT").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
        let model = std::env::var("TOKI_TEST_MODEL").unwrap_or_else(|_| coach::DEFAULT_OLLAMA_MODEL.to_string());
        let paths = recent_transcripts(7, 5, limit.max(40));
        eprintln!("\n=== {} recent transcripts (mtime net) ===", paths.len());
        let mut summaries = Vec::new();
        for p in &paths {
            if let Ok(tr) = distill_since(p, Some(Utc::now() - chrono::Duration::days(7))) {
                if !tr.prompts.is_empty() {
                    summaries.push(summarize(&tr, p));
                }
            }
        }
        summaries.sort_by(|a, b| a.started.cmp(&b.started));
        // Mirror the real generate_cross path: 이벤트 창이 이미 적용됐으므로
        // 시작 시각 재필터 없이 개수만 캡한다(§3.3.2 후속 픽스).
        let mut summaries = summaries;
        summaries.truncate(limit.max(2));
        let agg = aggregate(&summaries);
        let corpus = redundancy_corpus();
        let skill = agg.skill_heads.iter().find(|(h, _)| !already_encoded(h, &corpus));
        let signals = derive_signals(&agg, skill);
        eprintln!("\n=== signal cards (deterministic) ===");
        for s in &signals {
            eprintln!("[{}] {} — {}", s.cat, s.stat, s.body);
        }
        // Mirror generate_cross: no notable signal → the app returns a fixed
        // honest body and does NOT call the LLM (else it fabricates whole fake
        // signals). Stop here so this diagnostic reflects real behavior.
        if signals.is_empty() {
            eprintln!("\n(no notable signal → app returns canned honest body, LLM skipped)");
            return;
        }
        let persona = std::env::var("TOKI_TEST_PERSONA").ok().and_then(|id| persona_block(&id));
        let prompt = build_cross_prompt(&agg, &signals, persona.as_deref());
        // Print the deterministic data block (everything before the instr).
        eprintln!("\n=== aggregated data ===\n{}", prompt.split("# 크로스세션").nth(1).map(|s| format!("# 크로스세션{}", s)).unwrap_or_default());
        let backend = coach::Backend::Ollama { model };
        let t0 = std::time::Instant::now();
        let raw = coach::complete(&prompt, &backend, None).expect("coach failed");
        // Mirror generate_cross post-processing so the eyeball matches the app.
        let body = strip_emoji(&normalize_headings(&raw, "## 요약"));
        eprintln!("\n=== {} ({:?}) — {} sessions ===\n{}\n=== end ===\n", backend.label(), t0.elapsed(), summaries.len(), body);
        assert!(!body.is_empty());
    }
}
