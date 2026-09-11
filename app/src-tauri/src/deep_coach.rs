//! V5 — 딥 코칭 (claude -p 티어).
//!
//! 기존 크로스세션 코칭(§3.3)은 "결정론 신호 + 로컬 LLM 문장화" 헌법이라
//! 품질 상한이 신호 카드 수준에 고정된다 — "`for h`를 3세션 반복했어요,
//! 커맨드로 묶어보세요"가 천장. 사용자가 원하는 코칭(2026-08-31)은 실제
//! Claude 세션이 프롬프트 히스토리·스킬 인벤토리·기억을 훑고 추론해 내놓는
//! "statusline부터 세팅해라, /usage를 수십 번 치고 있더라" 급의 제안이다.
//!
//! 그 수준은 (a) 훨씬 풍부한 입력과 (b) 실제로 추론하는 모델이 둘 다 필요
//! 하다. 그래서 이 티어는:
//! - **수집은 결정론** (헌법 유지): `~/.claude/history.jsonl` 프롬프트 원문,
//!   커맨드/스킬/에이전트 인벤토리, CLAUDE.md, 메모리 인덱스, 7일 크로스세션
//!   집계 — 전부 코드가 모아 데이터 블록에 넣는다. 모델이 지어낼 수치가
//!   필요 없도록 근거를 미리 다 준다.
//! - **추론은 에이전트 CLI (claude -p sonnet / codex exec)**: 로컬 7.8B는 이 추론
//!   바를 못 넘는다 (spec §3.1이 결정론 distill을 택한 이유 그 자체). 쿼터
//!   아이러니(§3.1)는 존중한다 — 자동 실행 절대 없음, "딥 분석" 버튼의 명시적
//!   선택에만 발화, UI에 쿼터 소비와 나가는 데이터를 명시(spec §9.2).
//! - **기억은 감지 목록** (v5 M5, spec §6.4): 특정 도구 이름을 코드 기본
//!   지시문에 박지 않는다. 소스가 0이면 "기억 → 자산" 축을 지시문에서도 뺀다.
//!
//! 기존 로컬 코칭은 그대로 산다: 로컬 = 상시/무료 티어, 딥 = 온디맨드 티어.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Local, TimeZone, Utc};
use std::collections::HashMap;
use std::path::PathBuf;

// 모델 별칭은 백엔드가 정한다 (`coach::DEFAULT_CLAUDE_MODEL` = sonnet: haiku는
// 이 추론 바를 못 넘고 opus는 5h 쿼터를 과식). codex는 사용자 `config.toml`의
// 모델을 그대로 쓴다 — 딥 티어가 모델을 고르지 않는다.
/// 2026-08-31 2차: 7→30일. 7일 창은 최근 폭주(한 프로젝트의 버그 루프)가
/// 데이터를 지배해 "이번 주 불난 곳" 분석이 됐다(사용자 판정). 목표 수준의
/// 분석(Image #2)은 히스토리 전체를 봤다 — 여러 주에 걸친 반복이어야 자산화
/// 근거가 선다. 30일 실측 460엔트리 ≈ 90KB, sonnet엔 여유.
const HISTORY_DAYS: i64 = 30;
/// 프롬프트 예산 가드 — 히스토리 원문이 가장 크다. 엔트리 수·바이트 이중 캡.
const MAX_HISTORY_ENTRIES: usize = 1500;
const MAX_HISTORY_BYTES: usize = 240 * 1024;
/// 붙여넣기 대문 프롬프트 가드 — 한 엔트리가 블록을 다 먹지 못하게.
const MAX_ENTRY_CHARS: usize = 500;
/// 기억·규칙 소스 **총 예산** (spec §6.4). v4는 소스별 고정 캡이었다(engram 110KB /
/// CLAUDE.md 8KB / MEMORY.md 6KB) — engram 전용으로 짜인 값이라 소스를 일반화하며
/// 5등분하면 engram 사용자의 코칭이 그냥 나빠지고, MEMORY.md는 이미 45%가
/// 잘리고 있었다(실측 10.9KB). 그래서 존재하는 소스끼리 예산 하나를 나눈다:
/// 소스가 하나면 혼자 다 쓰고(=v4와 동일 프롬프트), 안 쓰는 소스 몫은 회수된다.
/// 기준선은 v4의 engram 캡 110KB — 늘리지도 줄이지도 않는다.
pub const MEMORY_BUDGET: usize = 110 * 1024;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeepCoaching {
    pub body: String,
    /// 분석에 들어간 히스토리 프롬프트 수 — UI가 "N개 프롬프트 분석"으로 표시.
    pub n_prompts: usize,
    pub n_days: u32,
    pub model: String,
    pub generated_at: String,
}

struct HistEntry {
    ts: DateTime<Local>,
    project: String,
    display: String,
}

/// `~/.claude/history.jsonl` — 사용자가 실제로 타이핑한 프롬프트의 전역 원장
/// (트랜스크립트와 달리 에이전트 발화가 전혀 섞이지 않는다). 한 줄 = 한 엔트리
/// {display, timestamp(ms), project}. 창 안의 엔트리를 시간순으로 돌려준다.
fn read_history(days: i64) -> Vec<HistEntry> {
    let Some(path) = dirs::home_dir().map(|h| h.join(".claude").join("history.jsonl")) else {
        return Vec::new();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let cutoff = Utc::now() - chrono::Duration::days(days);
    let mut out: Vec<HistEntry> = Vec::new();
    for line in raw.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let Some(ms) = v.get("timestamp").and_then(|t| t.as_i64()) else { continue };
        let Some(ts) = Utc.timestamp_millis_opt(ms).single() else { continue };
        if ts < cutoff {
            continue;
        }
        let display = v.get("display").and_then(|d| d.as_str()).unwrap_or("").trim().to_string();
        if display.is_empty() {
            continue;
        }
        let project = v
            .get("project")
            .and_then(|p| p.as_str())
            .map(short_project)
            .unwrap_or_else(|| "?".into());
        out.push(HistEntry { ts: ts.with_timezone(&Local), project, display });
    }
    out.sort_by_key(|e| e.ts);
    out
}

/// Codex 롤아웃 한 줄 → 사용자가 친 프롬프트. 두 모양을 받는다:
/// `event_msg`/`user_message`(`message`) 와 `response_item`/`message`/role=user
/// (`content[].input_text`). 후자에는 Codex 가 주입하는 `# AGENTS.md instructions`,
/// `<environment_context>` 같은 것도 role=user 로 섞여 있어 걸러낸다.
/// GPT(Codex)만 쓰는 사람은 `~/.claude/history.jsonl` 이 없어 딥 코칭이 "비어
/// 있다"고 거절했다(Windows 실기 보고, 2026-09-11).
fn codex_prompt_from_line(line: &str) -> Option<(DateTime<Utc>, String)> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let ts = v.get("timestamp")?.as_str()?;
    let ts = DateTime::parse_from_rfc3339(ts).ok()?.with_timezone(&Utc);
    let payload = v.get("payload")?;
    let text = match v.get("type")?.as_str()? {
        "event_msg" if payload.get("type").and_then(|t| t.as_str()) == Some("user_message") => {
            payload.get("message")?.as_str()?.to_string()
        }
        "response_item"
            if payload.get("type").and_then(|t| t.as_str()) == Some("message")
                && payload.get("role").and_then(|r| r.as_str()) == Some("user") =>
        {
            payload
                .get("content")?
                .as_array()?
                .iter()
                .filter(|c| c.get("type").and_then(|t| t.as_str()) == Some("input_text"))
                .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => return None,
    };
    let text = text.trim();
    // 주입된 문맥은 사용자 발화가 아니다.
    if text.is_empty() || text.starts_with('<') || text.starts_with("# AGENTS.md") || text.contains("<environment_context>") {
        return None;
    }
    Some((ts, text.to_string()))
}

/// `~/.codex/sessions/**/rollout-*.jsonl` 에서 창 안의 프롬프트. 파일 이름에 날짜가
/// 박혀 있어(`rollout-2026-09-11T…`) 창 밖 파일은 열지도 않는다. 한 파일 안에
/// `user_message` 이벤트가 있으면 그것만(둘이 같은 프롬프트를 두 번 담는다),
/// 없으면 response_item 으로 떨어진다. 프로젝트는 `turn_context`/`session_meta` 의 cwd.
fn read_codex_history(days: i64) -> Vec<HistEntry> {
    let Some(root) = dirs::home_dir().map(|h| h.join(".codex").join("sessions")) else {
        return Vec::new();
    };
    let cutoff = Utc::now() - chrono::Duration::days(days);
    let cutoff_day = cutoff.format("%Y-%m-%d").to_string();
    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                // rollout-YYYY-MM-DDT… — 날짜 문자열 비교로 창 밖을 거른다.
                if name.starts_with("rollout-") && name.ends_with(".jsonl") && name[8..].get(..10).map(|d| d >= cutoff_day.as_str()).unwrap_or(true) {
                    files.push(p);
                }
            }
        }
    }
    let mut out: Vec<HistEntry> = Vec::new();
    for f in files {
        let Ok(raw) = std::fs::read_to_string(&f) else { continue };
        let mut project = String::from("?");
        let mut events: Vec<(DateTime<Utc>, String)> = Vec::new();
        let mut items: Vec<(DateTime<Utc>, String)> = Vec::new();
        for line in raw.lines() {
            if project == "?" || line.contains("\"turn_context\"") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                    if let Some(cwd) = v.get("payload").and_then(|p| p.get("cwd")).and_then(|c| c.as_str()) {
                        if !crate::platform::is_transient_cwd(cwd) {
                            project = short_project(cwd);
                        }
                    }
                }
            }
            if let Some((ts, text)) = codex_prompt_from_line(line) {
                if ts < cutoff {
                    continue;
                }
                if line.contains("\"user_message\"") { events.push((ts, text)) } else { items.push((ts, text)) }
            }
        }
        let picked = if events.is_empty() { items } else { events };
        for (ts, display) in picked {
            out.push(HistEntry { ts: ts.with_timezone(&Local), project: project.clone(), display });
        }
    }
    out
}

/// Claude 와 GPT(Codex) 히스토리를 합쳐 시간순으로.
fn read_all_history(days: i64) -> Vec<HistEntry> {
    let mut all = read_history(days);
    all.extend(read_codex_history(days));
    all.sort_by_key(|e| e.ts);
    all
}

/// "/Users/me/work/projects/toki" → "toki". 프로젝트 경로는
/// 근거 표시용 라벨로만 쓰므로 leaf면 충분하다.
fn short_project(p: &str) -> String {
    p.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(p).to_string()
}

/// 슬래시 커맨드 사용 빈도 — Image-#2급 제안("statusline 세팅 — /usage를 수십
/// 번 치더라")의 근거가 되는 축. 히스토리에서 `/`로 시작하는 엔트리의 첫
/// 토큰을 센다.
fn slash_counts(entries: &[HistEntry]) -> Vec<(String, u32)> {
    let mut m: HashMap<String, u32> = HashMap::new();
    for e in entries {
        if let Some(rest) = e.display.strip_prefix('/') {
            let cmd = rest.split_whitespace().next().unwrap_or("");
            if !cmd.is_empty() && cmd.len() <= 40 {
                *m.entry(format!("/{cmd}")).or_insert(0) += 1;
            }
        }
    }
    let mut v: Vec<(String, u32)> = m.into_iter().filter(|(_, c)| *c >= 2).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(15);
    v
}

/// 히스토리 블록 — 시간순 원문. `[MM/DD HH:MM · project] 프롬프트`.
/// 슬래시 커맨드 단독 엔트리는 빈도표(slash_counts)가 대신 말하므로 여기선
/// 건너뛴다(원문 블록은 '무슨 일을 시켰나'를 보여주는 자리).
fn history_block(entries: &[HistEntry]) -> (String, usize) {
    let mut kept: Vec<&HistEntry> = entries
        .iter()
        .filter(|e| !e.display.starts_with('/') || e.display.contains(' '))
        .collect();
    // 캡은 최신 우선 — 오래된 쪽을 버린다.
    if kept.len() > MAX_HISTORY_ENTRIES {
        let cut = kept.len() - MAX_HISTORY_ENTRIES;
        kept.drain(..cut);
    }
    let mut s = String::new();
    for e in &kept {
        let mut d = e.display.replace('\n', " ");
        if d.chars().count() > MAX_ENTRY_CHARS {
            d = d.chars().take(MAX_ENTRY_CHARS).collect::<String>() + "…(잘림)";
        }
        s.push_str(&format!("[{} · {}] {}\n", e.ts.format("%m/%d %H:%M"), e.project, d));
        if s.len() > MAX_HISTORY_BYTES {
            break;
        }
    }
    (s, kept.len())
}

/// `max` **바이트** 이하로 자르되 UTF-8 문자 경계를 지킨다.
///
/// `String::truncate`는 바이트 인덱스가 문자 중간이면 **패닉**한다
/// (`assertion failed: self.is_char_boundary(new_len)`). 여기 들어오는 건
/// 전부 한글이 섞인 사용자 문서(CLAUDE.md·MEMORY.md·engram 인덱스)라 캡에
/// 걸리는 순간 거의 확실히 문자 중간을 짚는다 — 2026-08-31 딥 코칭이 이걸로
/// 죽었다(MEMORY.md에 한 줄 추가되며 6KB 캡을 넘긴 게 방아쇠). 캡이 있는
/// 모든 문자열 절단은 반드시 이 함수를 거친다.
fn truncate_utf8(s: &mut String, max: usize, note: &str) {
    if s.len() <= max {
        return;
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
    s.push_str(note);
}

/// frontmatter의 `description:` 값 (혹은 없으면 빈 문자열).
fn frontmatter_desc(text: &str) -> String {
    for line in text.lines().take(30) {
        if let Some(rest) = line.trim().strip_prefix("description:") {
            let d = rest.trim().trim_matches('"').trim_matches('\'');
            return d.chars().take(160).collect();
        }
    }
    String::new()
}

/// 이미 세팅된 자산 인벤토리 — 커맨드/스킬/에이전트 이름+설명. 딥 코칭의 핵심
/// 가드: 모델이 **이미 있는 것을 다시 제안하지 못하게** 명시적으로 보여준다
/// (크로스 코칭의 redundancy gate와 같은 역할을, 억제가 아니라 문맥 제공으로).
fn inventory_block(home: &std::path::Path) -> String {
    let claude = home.join(".claude");
    if !claude.exists() {
        return String::new();
    }
    let mut s = String::new();

    let list_md = |dir: &PathBuf, out: &mut String| {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut rows: Vec<String> = Vec::new();
        for f in rd.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let name = p.file_stem().and_then(|n| n.to_str()).unwrap_or("?").to_string();
            let desc = std::fs::read_to_string(&p).map(|t| frontmatter_desc(&t)).unwrap_or_default();
            rows.push(if desc.is_empty() {
                format!("- {}", name)
            } else {
                format!("- {} — {}", name, desc)
            });
        }
        rows.sort();
        for r in rows {
            out.push_str(&r);
            out.push('\n');
        }
    };

    s.push_str("### 슬래시 커맨드 (~/.claude/commands)\n");
    list_md(&claude.join("commands"), &mut s);
    s.push_str("### 에이전트 (~/.claude/agents)\n");
    list_md(&claude.join("agents"), &mut s);

    s.push_str("### 스킬 (~/.claude/skills)\n");
    if let Ok(rd) = std::fs::read_dir(claude.join("skills")) {
        let mut rows: Vec<String> = Vec::new();
        for f in rd.flatten() {
            let p = f.path();
            if !p.is_dir() {
                continue;
            }
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
            let desc = std::fs::read_to_string(p.join("SKILL.md"))
                .map(|t| frontmatter_desc(&t))
                .unwrap_or_default();
            rows.push(if desc.is_empty() {
                format!("- {}", name)
            } else {
                format!("- {} — {}", name, desc)
            });
        }
        rows.sort();
        for r in rows {
            s.push_str(&r);
            s.push('\n');
        }
    }
    s
}

/// 소스 종류. **기억**은 "쌓였는데 자산화 안 된 것"의 후보(기억 축을 켠다),
/// **규칙**은 이미 산문으로 인코딩된 자산이라 축을 켜지 않는다 — 규칙만 있는
/// 사람에게 "기억에 쌓인 걸 승격하라"고 시키면 없는 데이터를 찾으라는 지시가
/// 된다(spec §6.4가 막으려는 바로 그것).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Memory,
    Rules,
}

/// 감지된 기억·규칙 소스 하나. `text`는 캡 적용 전 원문.
#[derive(Debug, Clone)]
pub struct MemorySource {
    pub label: &'static str,
    pub kind: SourceKind,
    /// 표시용 경로(`~/…`). 여러 파일을 합친 소스는 글롭 형태.
    pub path: String,
    pub text: String,
    /// newest-first 파일(engram INDEX.md)은 꼬리=오래된 쪽을 자른다. 나머지도
    /// 꼬리를 자르지만 의미가 다르므로 잘림 주석이 다르다.
    pub newest_first: bool,
}

/// UI 고지용 요약(spec §9.2 "나가는 것" 표) — 원문 없이 라벨·경로·바이트만.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceInfo {
    pub label: String,
    pub kind: SourceKind,
    pub path: String,
    pub bytes: usize,
}

fn tilde(home: &std::path::Path, p: &std::path::Path) -> String {
    match p.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

fn read_nonempty(p: &std::path::Path) -> Option<String> {
    let t = std::fs::read_to_string(p).ok()?;
    if t.trim().is_empty() { None } else { Some(t) }
}

/// **기억 소스 감지 목록** (spec §6.4) — 소스가 늘면 여기 한 줄이다. 순서가 곧
/// 예산 배분·출력 순서: 사용자가 직접 넣은 것 → 에이전트 네이티브 기억 → 규칙 →
/// 외부 도구 원장. `home`을 받는 건 테스트가 가짜 홈으로 조립을 검증하기 위해서다.
pub fn detect_memory_sources(home: &std::path::Path) -> Vec<MemorySource> {
    let mut v: Vec<MemorySource> = Vec::new();

    // 1. 붙여넣은 기억 — 누구나 (spec §6.4.1). 파싱하지 않는다.
    let p = home.join(".toki").join("memory.md");
    if let Some(t) = read_nonempty(&p) {
        v.push(MemorySource { label: "붙여넣은 기억", kind: SourceKind::Memory, path: tilde(home, &p), text: t, newest_first: false });
    }

    // 2. Claude 네이티브 메모리 — 프로젝트별 MEMORY.md를 하나로 합친다.
    let projects = home.join(".claude").join("projects");
    if let Ok(rd) = std::fs::read_dir(&projects) {
        let mut entries: Vec<_> = rd.flatten().map(|f| f.path()).collect();
        entries.sort();
        let mut merged = String::new();
        for dir in entries {
            let idx = dir.join("memory").join("MEMORY.md");
            let Some(t) = read_nonempty(&idx) else { continue };
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("?");
            merged.push_str(&format!("## {}\n{}\n", name, t.trim_end()));
            merged.push('\n');
        }
        if !merged.trim().is_empty() {
            v.push(MemorySource {
                label: "Claude 메모리",
                kind: SourceKind::Memory,
                path: "~/.claude/projects/*/memory/MEMORY.md".into(),
                text: merged,
                newest_first: false,
            });
        }
    }

    // 3. 전역 규칙 — 이미 산문으로 인코딩된 것.
    let p = home.join(".claude").join("CLAUDE.md");
    if let Some(t) = read_nonempty(&p) {
        v.push(MemorySource { label: "CLAUDE.md", kind: SourceKind::Rules, path: tilde(home, &p), text: t, newest_first: false });
    }
    let p = home.join(".codex").join("AGENTS.md");
    if let Some(t) = read_nonempty(&p) {
        v.push(MemorySource { label: "AGENTS.md", kind: SourceKind::Rules, path: tilde(home, &p), text: t, newest_first: false });
    }

    // 4. 외부 기억 도구 — 있으면 자동 감지. 파일을 직접 읽는다(결정론·무의존).
    //    INDEX.md는 newest-first라 초과분은 꼬리(오래된 쪽)를 자른다.
    let p = home.join(".engram").join("INDEX.md");
    if let Some(t) = read_nonempty(&p) {
        v.push(MemorySource { label: "engram 인덱스", kind: SourceKind::Memory, path: tilde(home, &p), text: t, newest_first: true });
    }

    v
}

/// 총 예산을 존재하는 소스끼리 나눈다 — **물 채우기(water-filling)**. 예산을
/// 남은 소스 수로 등분해 작은 소스부터 필요한 만큼만 주고, 남는 몫은 큰 소스에
/// 넘긴다. 소스가 하나면 예산 전부, 합이 예산 이하면 아무도 안 잘린다.
pub fn allocate_budget(sizes: &[usize], budget: usize) -> Vec<usize> {
    let mut caps = vec![0usize; sizes.len()];
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| sizes[i]);
    let mut remaining = budget;
    let mut left = sizes.len();
    for i in order {
        let share = if left == 0 { 0 } else { remaining / left };
        let take = sizes[i].min(share);
        caps[i] = take;
        remaining -= take;
        left -= 1;
    }
    caps
}

/// 소스마다 배정된 캡으로 자른 (라벨, 본문) 목록. 조립·테스트가 같이 쓴다.
fn capped_sources(sources: &[MemorySource], budget: usize) -> Vec<(&MemorySource, String)> {
    let sizes: Vec<usize> = sources.iter().map(|s| s.text.len()).collect();
    let caps = allocate_budget(&sizes, budget);
    sources
        .iter()
        .zip(caps)
        .map(|(src, cap)| {
            let mut t = src.text.clone();
            let note = if src.newest_first { "\n…(오래된 항목 잘림)" } else { "\n…(잘림)" };
            truncate_utf8(&mut t, cap, note);
            (src, t)
        })
        .collect()
}

/// 기억 축을 켤지 — **기억** 종류 소스가 하나라도 있으면.
pub fn has_memory_axis(sources: &[MemorySource]) -> bool {
    sources.iter().any(|s| s.kind == SourceKind::Memory)
}

pub const MEMORY_AXIS_OPEN: &str = "<!-- memory-axis -->";
pub const MEMORY_AXIS_CLOSE: &str = "<!-- /memory-axis -->";

/// 지시문의 기억 축 조건부화 — **조립 단계에서** (spec §6.4). 마커 구간은
/// 기억 소스가 0이면 잘려 나간다. 코드 기본값이든 로컬 오버라이드든 똑같이
/// 적용되고, **마커가 없는 파일은 통째로** 쓴다(하위호환). 마커는 켜져 있을
/// 때도 출력에서 지운다(모델에게 HTML 주석을 보낼 이유가 없다).
pub fn apply_memory_axis(instr: &str, on: bool) -> String {
    let mut out = String::with_capacity(instr.len());
    let mut rest = instr;
    loop {
        let Some(a) = rest.find(MEMORY_AXIS_OPEN) else {
            out.push_str(rest);
            break;
        };
        let after_open = &rest[a + MEMORY_AXIS_OPEN.len()..];
        let Some(b) = after_open.find(MEMORY_AXIS_CLOSE) else {
            // 짝 없는 여는 마커 — 그대로 둔다(파일을 망가뜨리지 않는다).
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..a]);
        if on {
            out.push_str(after_open[..b].trim_matches('\n'));
        }
        rest = &after_open[b + MEMORY_AXIS_CLOSE.len()..];
        if !on {
            // 잘린 자리의 빈 줄 겹침 정리.
            rest = rest.trim_start_matches('\n');
            while out.ends_with("\n\n") {
                out.pop();
            }
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}

/// 딥 코칭 지시문 기본값. `~/.toki/prompts/deep-coaching.md`로 외부화 —
/// 파일 수정 → 다음 실행에 반영, 재빌드 불필요 (cross-coaching과 동일 규약).
const DEFAULT_DEEP_INSTR: &str = r#"당신은 Toki의 딥 코치 — 사용자의 실제 Claude Code 사용 기록 전체를 근거로, 이 사용자에게 지금 가장 효용이 큰 **구체적 세팅·습관 변화**를 제안하는 컨설턴트입니다.
아래 <deep_data> 안이 전부 실측 데이터입니다: 최근 30일 프롬프트 원문(사용자가 실제 타이핑한 것), 슬래시 커맨드 빈도, 이미 세팅된 자산(커맨드/스킬/에이전트), 규칙 파일(CLAUDE.md·AGENTS.md), 지속 기억(반복 교정·선호·프로젝트 상태 — 있는 경우), 최근 7일 크로스세션 집계. 블록이 없으면 그 데이터는 없는 것이고, 없는 걸 있는 척 채우지 마세요.

# 임무
"반복되는데 자산화 안 된 패턴"을 찾아, 새 스킬 / 슬래시 커맨드 / CLAUDE.md 규칙 / 훅 / 설정(statusline 등) / 에이전트 / 기존 자산 개선 중 무엇으로 인코딩할지 **근거 강한 순으로 4~6개** 제안하세요.

# 분석 층위 (전 층위를 훑고, 층위가 다양할수록 좋은 분석)
- **명령 반복** → 슬래시 커맨드: 같은 명령·조회를 손으로 반복 (예: 사용량 확인을 반복하면 statusline 상시 노출이 더 나은 답).
- **절차 반복** → 스킬: 매번 채팅으로 다시 설명하는 다단계 워크플로(배포 절차, 이식 규칙, 리포트 규격).
- **교정 반복** → 규칙/훅: 같은 지적·선호가 여러 날/여러 프로젝트에서 반복 ("~하지 마", "~로 해줘"). "항상/절대"류는 산문 규칙보다 훅이 확실.
- **역할 반복** → 에이전트: 같은 성격의 일(전수 훑기, 검수, 리서치)을 계속 시키면 역할 에이전트 후보.
- **구조 공백** → 자산 라인업 자체의 평가: 커맨드/스킬/에이전트/훅/설정 중 어떤 층위가 통째로 비었나 (예: 스킬은 많은데 에이전트는 모델 라우팅용뿐이라면 '역할 자산화'가 빈 것).

<!-- memory-axis -->
# 핵심 구분: 기억 ≠ 자산
지속 기억에 **기록돼 있다는 건 자산화가 아닙니다**. 기억은 회상돼야 작동하지만, 스킬/커맨드/훅/규칙은 필요한 순간 강제로 로드됩니다. 기억에 같은 테마의 교정·규칙이 여러 건 쌓여 있는데 실행 자산으로 승격 안 된 것 — 그게 **가장 좋은 제안 후보**입니다 (예: 이식 규칙이 기억에 4건 쌓였으면 그건 기억이 아니라 스킬이어야 함). 근거를 찾을 때 30일 히스토리와 기억을 같은 비중으로 훑으세요.
<!-- /memory-axis -->

# 이미 있는 자산
commands/skills/agents/CLAUDE.md/AGENTS.md에 이미 실행 가능하게 인코딩된 건 새로 제안하지 말고, 반복 패턴이 기존 자산과 겹치면 "왜 그 자산이 안 쓰이는지"를 짚으세요.

# 시간 균형 (중요)
- 여러 주에 걸쳐 반복되는 패턴이 최근 며칠의 폭주보다 **항상 근거가 강합니다**. 최근 몰린 단일 프로젝트 이슈에서 나오는 제안은 최대 1~2개로 제한하고, 나머지는 30일 전체에서 찾으세요.
- 근거에는 기간을 명시하세요 ("3주에 걸쳐", "N개 프로젝트에서").

# 절대 규칙
- **모든 제안에 실제 근거 인용 필수**: 반복 횟수·기간과, 가능하면 실제 프롬프트(또는 데이터에 있는 기억 항목)를 짧게 직접 인용("…"). 데이터에 없는 수치·인용은 환각 — 금지.
- 사용자가 실제로 안 한 일을 했다고 하지 마세요.
- 말투는 해요체("~예요/~했어요")로 통일. '~습니다/~한다' 금지.
- 이모지 금지. 코드펜스(```) 금지 — 인라인 `코드`와 불릿만 (렌더러가 펜스를 지원 안 함).
- 일반론·덕담 금지 ("더 계획적으로", "잘 하고 계세요" 류). 모든 문장이 이 사용자의 데이터에서만 나올 수 있는 문장이어야 해요.
- 마크다운 구조: `## 섹션`과 `### 항목`, 불릿, **굵게**, 인라인 코드만.

# 출력 형식 (정확히 이 섹션 순서)
## 지금 상태
3-5문장: 30일 사용 패턴 요약(프로젝트 분포·주된 작업 성격) + **자산 라인업 구조 평가**(어떤 층위가 차 있고 어떤 층위가 비었나). 수치 인용.

## 추천
근거 강한 순으로 `### 1. 이름 — 종류` (종류 = 스킬/커맨드/규칙/훅/설정/에이전트/개선). 각 항목 아래 불릿 3-4개:
- **근거**: 데이터에서 본 반복 패턴 + 횟수/기간, 실제 인용 1개.
- **무엇을**: 만들 것의 정확한 내용 한 줄.
- **어떻게**: 파일 경로·절차를 바로 실행 가능한 수준으로 (예: `~/.claude/commands/이름.md`에 어떤 내용).
- **확인**: 다음에 어떤 반복이 사라져야 성공인지.

## 한 줄 결론
뭐부터 만들지 하나 골라 한 문장으로."#;

fn build_deep_prompt(
    history: &str,
    n_prompts: usize,
    slash: &[(String, u32)],
    stats: Option<&str>,
    sources: &[MemorySource],
) -> String {
    let instr = crate::retro::load_prompt_template("deep-coaching", DEFAULT_DEEP_INSTR);
    let home = dirs::home_dir().unwrap_or_default();
    build_deep_prompt_with(&instr, &home, history, n_prompts, slash, stats, sources)
}

/// 지시문을 주입받는 조립 — 로컬 오버라이드 로딩과 조립을 분리한다. 테스트는
/// 실제 `~/.toki/prompts/deep-coaching.md`(개인 튜닝본)에 오염되면 안 되므로
/// 항상 코드 기본값을 명시적으로 넘긴다.
fn build_deep_prompt_with(
    instr: &str,
    home: &std::path::Path,
    history: &str,
    n_prompts: usize,
    slash: &[(String, u32)],
    stats: Option<&str>,
    sources: &[MemorySource],
) -> String {
    let instr = apply_memory_axis(instr, has_memory_axis(sources));

    let mut d = String::new();
    d.push_str(&format!(
        "# 크로스세션 집계 (최근 {}일, 트랜스크립트 결정론 집계)\n{}\n",
        HISTORY_DAYS,
        stats.unwrap_or("(집계 없음)")
    ));
    if !slash.is_empty() {
        d.push_str("\n# 슬래시 커맨드 사용 빈도 (최근 7일, 히스토리 실측)\n");
        for (cmd, c) in slash {
            d.push_str(&format!("- `{}` — {}회\n", cmd, c));
        }
    }
    d.push_str("\n# 이미 세팅된 자산 (이건 새로 제안하지 말 것)\n");
    d.push_str(&inventory_block(home));

    // 기억·규칙 소스 — 감지 목록 순서, 총 예산 안에서.
    for (src, text) in capped_sources(sources, MEMORY_BUDGET) {
        let head = match src.kind {
            SourceKind::Rules => format!("\n# 규칙 파일 — {} ({}) · 이미 산문 규칙으로 인코딩된 것\n", src.label, src.path),
            SourceKind::Memory => format!("\n# 지속 기억 — {} ({}) · '기억 ≠ 자산' 구분 적용 대상\n", src.label, src.path),
        };
        d.push_str(&head);
        d.push_str(&text);
        if !text.ends_with('\n') {
            d.push('\n');
        }
    }

    d.push_str(&format!(
        "\n# 프롬프트 히스토리 원문 (최근 {}일, 사용자가 실제 타이핑한 {}개, 시간순)\n",
        HISTORY_DAYS, n_prompts
    ));
    d.push_str(history);

    format!("{}\n\n<deep_data>\n{}</deep_data>", instr, d)
}

/// 실행 전 고지용 미리보기 (spec §9.2 "나가는 것" 표) — LLM 호출 없음.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeepPreview {
    pub backend: String,
    pub backend_kind: String,
    /// 프롬프트가 사용자 CLI를 타고 밖으로 나가는가. Ollama면 false.
    pub egresses: bool,
    pub n_prompts: usize,
    pub n_days: u32,
    pub sources: Vec<SourceInfo>,
    /// 자산 목록(커맨드/스킬/에이전트 파일명)이 잡혔는가.
    pub has_inventory: bool,
}

pub fn preview(backend: &crate::coach::Backend) -> DeepPreview {
    let entries = read_all_history(HISTORY_DAYS);
    let (_, n_prompts) = history_block(&entries);
    let home = dirs::home_dir().unwrap_or_default();
    let sources = detect_memory_sources(&home);
    let capped = capped_sources(&sources, MEMORY_BUDGET);
    DeepPreview {
        backend: backend.label(),
        backend_kind: backend.kind().to_string(),
        egresses: backend.egresses(),
        n_prompts,
        n_days: HISTORY_DAYS as u32,
        sources: capped
            .iter()
            .map(|(s, t)| SourceInfo { label: s.label.to_string(), kind: s.kind, path: s.path.clone(), bytes: t.len() })
            .collect(),
        has_inventory: !inventory_block(&home).trim().is_empty(),
    }
}

/// 딥 코칭 1회 실행. **호출자는 반드시 명시적 사용자 액션이어야 한다** —
/// 에이전트 CLI는 사용자 구독 쿼터를 소비한다(§3.1 쿼터 아이러니). 백엔드는
/// 설정을 따른다(`coach::Backend::resolve`) — Ollama를 골랐으면 로컬로 돈다
/// (문맥이 짧아 품질은 떨어지지만 밖으로 안 나간다는 게 그 선택의 이유다).
pub fn generate_deep(backend: &crate::coach::Backend) -> Result<DeepCoaching> {
    let entries = read_all_history(HISTORY_DAYS);
    if entries.is_empty() {
        return Err(anyhow!(
            "최근 {}일 프롬프트 히스토리가 비어 있어요 (Claude ~/.claude/history.jsonl · GPT ~/.codex/sessions)",
            HISTORY_DAYS
        ));
    }
    let slash = slash_counts(&entries);
    let (hist_block, n_prompts) = history_block(&entries);
    let stats = crate::retro::deep_stats_markdown();
    let home = dirs::home_dir().unwrap_or_default();
    let sources = detect_memory_sources(&home);
    let prompt = build_deep_prompt(&hist_block, n_prompts, &slash, stats.as_deref(), &sources);
    eprintln!(
        "[deep] {} prompts · {} slash cmds · {} memory sources ({}) · prompt {}KB → {}",
        n_prompts,
        slash.len(),
        sources.len(),
        sources.iter().map(|s| s.label).collect::<Vec<_>>().join(", "),
        prompt.len() / 1024,
        backend.label()
    );

    // 에이전트 CLI는 `~/.toki`를 cwd로 — 사용자 프로젝트의 CLAUDE.md/AGENTS.md가
    // 코칭 프롬프트에 끼어들지 않게(데이터 블록에 이미 전역 규칙이 들어 있다).
    let cwd = home.join(".toki");
    let _ = std::fs::create_dir_all(&cwd);
    let raw = crate::coach::complete_with_timeout(
        &prompt,
        backend,
        Some(&cwd),
        Some(backend.deep_timeout()),
    )?;
    let body = crate::retro::strip_emoji(&raw).trim().to_string();
    if body.is_empty() {
        return Err(anyhow!("딥 코치가 빈 응답을 반환"));
    }

    let dc = DeepCoaching {
        body,
        n_prompts,
        n_days: HISTORY_DAYS as u32,
        model: backend.label(),
        generated_at: Local::now().to_rfc3339(),
    };
    save_deep(&dc);
    Ok(dc)
}

/// `deep-<ts>.json` + 사람용 `.md` — cross 캐시와 같은 폴더, prefix로 구분
/// (latest_cross는 `cross-`만 읽는다).
fn save_deep(dc: &DeepCoaching) {
    let Some(dir) = crate::retro::coaching_dir() else { return };
    let stamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    if let Ok(json) = serde_json::to_string_pretty(dc) {
        let _ = std::fs::write(dir.join(format!("deep-{stamp}.json")), json);
    }
    let md = format!(
        "# 딥 코칭 · {}\n프롬프트 {}개 · 최근 {}일 · {}\n\n{}\n",
        dc.generated_at, dc.n_prompts, dc.n_days, dc.model, dc.body
    );
    let _ = std::fs::write(dir.join(format!("deep-{stamp}.md")), md);
}

/// 최신 딥 캐시 — 재진입 시 즉시 표시용 (쿼터 재소비 없음).
pub fn latest_deep() -> Option<DeepCoaching> {
    let dir = crate::retro::coaching_dir()?;
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for f in std::fs::read_dir(&dir).ok()?.flatten() {
        let p = f.path();
        let is_deep = p
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("deep-"))
            .unwrap_or(false);
        if is_deep && p.extension().and_then(|e| e.to_str()) == Some("json") {
            let m = f.metadata().and_then(|md| md.modified()).unwrap_or(std::time::UNIX_EPOCH);
            if newest.as_ref().map(|(t, _)| m > *t).unwrap_or(true) {
                newest = Some((m, p));
            }
        }
    }
    let (_, p) = newest?;
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-08-31 회귀 — 캡 경계가 한글 문자 중간에 떨어지면 String::truncate가
    /// 패닉했다("assertion failed: self.is_char_boundary"). 딥 코칭이 이걸로
    /// 죽었고(MEMORY.md가 6KB 캡을 넘김) 원인은 세 절단 지점 중 두 곳에만
    /// 경계 처리가 있었던 것.
    #[test]
    fn truncate_utf8_never_splits_a_char() {
        // "가"는 3바이트 → 캡 4는 두 번째 글자 중간(바이트 4)에 떨어진다.
        let mut s = "가나다".to_string();
        truncate_utf8(&mut s, 4, "…");
        assert_eq!(s, "가…");

        // 캡 경계를 1바이트씩 훑어 어느 위치에서도 패닉하지 않음을 보인다.
        for cap in 0.."한글 섞인 문서".len() + 2 {
            let mut t = "한글 섞인 문서".to_string();
            truncate_utf8(&mut t, cap, "");
            assert!(t.is_char_boundary(t.len()));
        }

        // 캡 이하면 손대지 않는다(주석도 안 붙인다).
        let mut u = "짧음".to_string();
        truncate_utf8(&mut u, 999, "…");
        assert_eq!(u, "짧음");
    }

    #[test]
    fn slash_counts_groups_and_gates() {
        let mk = |d: &str| HistEntry {
            ts: Local::now(),
            project: "x".into(),
            display: d.into(),
        };
        let entries = vec![mk("/usage"), mk("/usage"), mk("/model"), mk("ㄱㄱ"), mk("/usage arg")];
        let v = slash_counts(&entries);
        assert_eq!(v[0], ("/usage".into(), 3));
        // /model은 1회 → 게이트(≥2)에서 탈락
        assert!(!v.iter().any(|(c, _)| c == "/model"));
    }

    /// 실기 확인용(무시됨): 이 맥의 실제 ~/.codex/sessions 에서 30일치 프롬프트를 센다.
    /// `cargo test read_codex_history_on_this_machine -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn read_codex_history_on_this_machine() {
        let v = read_codex_history(30);
        eprintln!("codex prompts(30d) = {}", v.len());
        for e in v.iter().rev().take(3) {
            eprintln!("  {} [{}] {}", e.ts.format("%m-%d %H:%M"), e.project, e.display.chars().take(60).collect::<String>());
        }
    }

    /// Codex 롤아웃: 사용자 발화만 남고, 주입된 AGENTS.md·환경 문맥은 걸러진다.
    #[test]
    fn codex_prompt_line_filters_injected_context() {
        let user = r#"{"timestamp":"2026-09-11T01:02:03.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"read_graph 툴로 노드 개수만"}]}}"#;
        let (ts, t) = codex_prompt_from_line(user).unwrap();
        assert_eq!(t, "read_graph 툴로 노드 개수만");
        assert_eq!(ts.format("%Y-%m-%dT%H:%M:%S").to_string(), "2026-09-11T01:02:03");
        // `"#` 가 본문에 있어 r##"…"## 로 감싼다.
        let injected = r##"{"timestamp":"2026-09-11T01:02:03.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions\n<INSTRUCTIONS>..."}]}}"##;
        assert!(codex_prompt_from_line(injected).is_none());
        let env = r#"{"timestamp":"2026-09-11T01:02:03.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>cwd</environment_context>"}]}}"#;
        assert!(codex_prompt_from_line(env).is_none());
        let assistant = r#"{"timestamp":"2026-09-11T01:02:03.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"답"}]}}"#;
        assert!(codex_prompt_from_line(assistant).is_none());
        let ev = r#"{"timestamp":"2026-09-11T01:02:03.000Z","type":"event_msg","payload":{"type":"user_message","message":"안녕"}}"#;
        assert_eq!(codex_prompt_from_line(ev).unwrap().1, "안녕");
    }

    #[test]
    fn history_block_skips_bare_slash_commands() {
        let mk = |d: &str| HistEntry {
            ts: Local::now(),
            project: "x".into(),
            display: d.into(),
        };
        let entries = vec![mk("/usage"), mk("버그 고쳐줘"), mk("/graphify 이 문서")];
        let (block, n) = history_block(&entries);
        assert_eq!(n, 2); // 단독 슬래시는 빈도표 몫
        assert!(block.contains("버그 고쳐줘"));
        assert!(block.contains("/graphify 이 문서")); // 인자 있는 건 원문 유지
    }

    /// 가짜 홈 — 감지 목록은 `home`을 인자로 받으므로 진짜 `~`를 안 건드리고
    /// 조립 전체를 검증할 수 있다(인스톨러 테스트와 같은 sandbox 규약).
    fn mk_home(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let base = std::env::temp_dir().join(format!("toki-deep-test-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        for (rel, body) in files {
            let p = base.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        base
    }

    /// 코드 기본 지시문으로만 조립 — 개인 오버라이드 파일과 무관하게 공개
    /// 사용자가 보게 될 프롬프트를 검증한다.
    fn prompt_for(home: &std::path::Path) -> String {
        let sources = detect_memory_sources(home);
        build_deep_prompt_with(DEFAULT_DEEP_INSTR, home, "[09/01 10:00 · p] 뭐 좀 해줘\n", 1, &[], None, &sources)
    }

    /// spec §6.4 — 코드 기본 지시문에 특정 기억 도구 이름이 없어야 한다.
    /// (로컬 오버라이드 `~/.toki/prompts/deep-coaching.md`는 개인 튜닝이라 별개.)
    #[test]
    fn default_instruction_names_no_specific_memory_tool() {
        assert!(!DEFAULT_DEEP_INSTR.contains("engram"));
        assert!(DEFAULT_DEEP_INSTR.contains(MEMORY_AXIS_OPEN));
        assert!(DEFAULT_DEEP_INSTR.contains(MEMORY_AXIS_CLOSE));
    }

    /// 소스 0 → 마커 구간이 잘려 "기억 ≠ 자산" 지시문이 사라진다.
    /// 소스 1 이상 → 살아 있고, 마커 자체는 출력에 남지 않는다.
    #[test]
    fn memory_axis_cut_when_no_memory_source() {
        let on = apply_memory_axis(DEFAULT_DEEP_INSTR, true);
        assert!(on.contains("기억 ≠ 자산"));
        assert!(!on.contains(MEMORY_AXIS_OPEN) && !on.contains(MEMORY_AXIS_CLOSE));

        let off = apply_memory_axis(DEFAULT_DEEP_INSTR, false);
        assert!(!off.contains("기억 ≠ 자산"));
        assert!(!off.contains(MEMORY_AXIS_OPEN));
        // 축 밖의 지시문은 그대로 살아 있다.
        assert!(off.contains("# 출력 형식"));
        assert!(off.contains("# 임무"));
    }

    /// 하위호환 — 마커 없는 옛 로컬 파일은 통째로 쓴다(축이 꺼져도).
    #[test]
    fn marker_less_instruction_used_whole() {
        let old = "옛 지시문\nengram을 보세요\n";
        assert_eq!(apply_memory_axis(old, false), old);
        assert_eq!(apply_memory_axis(old, true), old);
    }

    /// 규칙 파일만 있는 사람에겐 기억 축을 켜지 않는다 — 없는 데이터를 찾으라는
    /// 지시가 되기 때문(spec §6.4).
    #[test]
    fn rules_only_does_not_enable_memory_axis() {
        let home = mk_home("rules", &[(".claude/CLAUDE.md", "규칙")]);
        let sources = detect_memory_sources(&home);
        assert_eq!(sources.len(), 1);
        assert!(!has_memory_axis(&sources));
        let p = prompt_for(&home);
        assert!(!p.contains("기억 ≠ 자산"));
        assert!(p.contains("규칙 파일 — CLAUDE.md"));
    }

    /// 기억 소스 5종을 각각 단독으로 놓고 조립이 정상인지.
    #[test]
    fn each_source_alone_assembles() {
        let cases: [(&str, &str, bool); 5] = [
            (".toki/memory.md", "붙여넣은 기억", true),
            (".claude/projects/proj/memory/MEMORY.md", "Claude 메모리", true),
            (".claude/CLAUDE.md", "CLAUDE.md", false),
            (".codex/AGENTS.md", "AGENTS.md", false),
            (".engram/INDEX.md", "engram 인덱스", true),
        ];
        for (i, (rel, label, is_memory)) in cases.into_iter().enumerate() {
            let home = mk_home(&format!("solo{i}"), &[(rel, "한 줄 기억 내용")]);
            let sources = detect_memory_sources(&home);
            assert_eq!(sources.len(), 1, "{rel}");
            assert_eq!(sources[0].label, label);
            assert_eq!(has_memory_axis(&sources), is_memory, "{rel}");
            let p = prompt_for(&home);
            assert!(p.contains(label), "{rel}: 라벨 누락");
            assert!(p.contains("한 줄 기억 내용"), "{rel}: 본문 누락");
            assert_eq!(p.contains("기억 ≠ 자산"), is_memory, "{rel}: 축 상태");
        }
    }

    /// 회귀 가드 — engram만 있는 환경(v4 세팅)에서 engram이 예산을 다 쓴다.
    /// 일반화 때문에 engram이 잘리면 실패.
    #[test]
    fn engram_only_keeps_v4_budget() {
        let big = "가".repeat(30 * 1024); // 90KB — 실측 규모
        let home = mk_home("engramonly", &[(".engram/INDEX.md", big.as_str())]);
        let sources = detect_memory_sources(&home);
        let capped = capped_sources(&sources, MEMORY_BUDGET);
        assert_eq!(capped.len(), 1);
        assert_eq!(capped[0].1.len(), big.len(), "engram이 잘렸다");
        assert!(!capped[0].1.contains("잘림"));
    }

    /// v4에서 6KB 캡에 45% 잘리던 MEMORY.md(실측 10.9KB)가 총 예산제에선
    /// 다른 소스와 같이 있어도 안 잘린다.
    #[test]
    fn memory_md_no_longer_truncated_next_to_engram() {
        let mem = "나".repeat(4 * 1024); // 12KB
        let eng = "다".repeat(30 * 1024); // 90KB
        let home = mk_home("memmd", &[
            (".claude/projects/p/memory/MEMORY.md", mem.as_str()),
            (".engram/INDEX.md", eng.as_str()),
        ]);
        let sources = detect_memory_sources(&home);
        let capped = capped_sources(&sources, MEMORY_BUDGET);
        for (src, text) in &capped {
            assert!(!text.contains("잘림"), "{} 잘림", src.label);
        }
        assert!(capped.iter().any(|(s, t)| s.label == "Claude 메모리" && t.contains(&mem)));
    }

    /// 총 예산제 — 합이 예산 이하면 아무도 안 자르고, 넘치면 작은 소스부터
    /// 필요한 만큼 주고 남는 몫을 큰 소스가 가져간다(회수 없음).
    #[test]
    fn budget_water_filling() {
        assert_eq!(allocate_budget(&[10, 20], 100), vec![10, 20]);
        // 예산 100, 소스 셋(10/10/500): 작은 둘이 10씩, 큰 하나가 나머지 80.
        assert_eq!(allocate_budget(&[10, 10, 500], 100), vec![10, 10, 80]);
        // 소스 하나면 예산 전부.
        assert_eq!(allocate_budget(&[500], 100), vec![100]);
        assert_eq!(allocate_budget(&[], 100), Vec::<usize>::new());
        // 배정 합은 예산을 절대 넘지 않는다.
        let caps = allocate_budget(&[70, 80, 90], 100);
        assert!(caps.iter().sum::<usize>() <= 100);
    }

    /// 기억·규칙이 하나도 없는 맥(=순정 공개 사용자)에서도 조립이 되고,
    /// 프롬프트에 특정 도구 이름이 안 나온다.
    #[test]
    fn bare_machine_assembles_without_memory() {
        let home = mk_home("bare", &[]);
        let sources = detect_memory_sources(&home);
        assert!(sources.is_empty());
        let p = prompt_for(&home);
        assert!(!p.contains("engram"));
        assert!(!p.contains("기억 ≠ 자산"));
        assert!(p.contains("# 임무"));
    }

    /// 실제 claude -p 1회 호출 (⚠️ 5h 쿼터 소비) — 골든 샘플 검증용:
    ///   cargo test --lib run_deep_once -- --ignored --nocapture
    #[test]
    #[ignore]
    fn run_deep_once() {
        let backend = crate::coach::Backend::from_settings("auto", "", None);
        match generate_deep(&backend) {
            Ok(d) => eprintln!(
                "=== deep coaching ({} prompts, {}) ===\n{}",
                d.n_prompts, d.model, d.body
            ),
            Err(e) => panic!("deep coaching failed: {e}"),
        }
    }

    /// 실데이터로 조립된 딥 프롬프트를 눈으로 확인 (LLM 호출 없음):
    ///   cargo test --lib dump_deep_prompt -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_deep_prompt() {
        let entries = read_history(HISTORY_DAYS);
        eprintln!("history entries in window: {}", entries.len());
        let slash = slash_counts(&entries);
        let (hist, n) = history_block(&entries);
        let stats = crate::retro::deep_stats_markdown();
        let home = dirs::home_dir().unwrap();
        let sources = detect_memory_sources(&home);
        let prompt = build_deep_prompt(&hist, n, &slash, stats.as_deref(), &sources);
        eprintln!("=== deep prompt ({} KB) ===\n{}", prompt.len() / 1024, prompt);
    }
}
