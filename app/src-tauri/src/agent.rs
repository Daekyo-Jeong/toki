//! 에이전트 어댑터 — Claude Code와 Codex CLI의 차이를 트레잇 하나 뒤로 숨긴다.
//!
//! 이 경계가 없으면 Codex 지원이 `retro.rs`(3000줄+)까지 번진다. 아래 계층은
//! `Event`만 보고 어느 에이전트에서 왔는지 모른 채 동작해야 한다. spec §5.1.
//!
//! M1은 Claude 구현체 하나만 둔다. Codex는 M2에서 붙이며, 그때 추가되는 건
//! `impl AgentSource`와 `sources()`의 한 줄이어야 한다.

use std::path::{Path, PathBuf};

/// 어느 에이전트에서 온 이벤트인가. `processed_events.agent`에 그대로 들어간다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentId {
    Claude,
    Codex,
}

impl AgentId {
    pub const fn as_str(self) -> &'static str {
        match self {
            AgentId::Claude => "claude",
            AgentId::Codex => "codex",
        }
    }
}

/// 소스가 준 사용률 창.
///
/// **창 길이를 하드코딩하지 않는다.** Claude는 5시간이지만 Codex는 플랜에
/// 따라 다르다(실측 `window_minutes: 10080` = 주간). 소스가 준 값을 그대로
/// 싣고 다닌다. spec §5.3.
///
/// UI에는 퍼센트만 노출하고 창 종류는 **표시하지 않는다** — 배터리는 펫의
/// 허기 게이지지 쿼터 대시보드가 아니다. `window_minutes`는 표시용이 아니라
/// 폴백 판단·디버깅용이다.
#[derive(Debug, Clone, Copy)]
pub struct UsageWindow {
    pub used_percent: f32,
    pub window_minutes: u32,
    pub resets_at: Option<i64>,
}

/// 에이전트별 로그 한 줄을 정규화한 것. 여기서부터 아래는 에이전트를 모른다.
#[derive(Debug, Clone)]
pub struct Event {
    pub agent: AgentId,
    pub uuid: String,
    pub session_id: String,
    pub timestamp: String,
    pub model: Option<String>,
    /// ⚠️ Claude에선 streaming placeholder라 **못 쓰는 값**이다(누적 실측
    /// 7.4M — 같은 기간 cache_read 22.7B에 견주면 무의미). 원장 보존용으로만
    /// 저장하고 XP·집계에는 쓰지 않는다. spec §5.2.
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub project_path: Option<String>,
    /// 에이전트 원본 도구 블록. 지금은 Claude의 `message.content` 모양 그대로라
    /// `thrash_watch`가 이 모양을 안다. M2에서 Codex의 `custom_tool_call`을
    /// 붙일 때 공통 모양으로 정규화한다.
    pub raw_content: Option<serde_json::Value>,
}

impl Event {
    /// XP 화폐 (spec §6.1).
    ///
    /// `input_tokens`는 뺀다. Claude에선 못 쓰는 값이고, 포함시키면 에이전트
    /// 사이 스케일이 어긋난다. Codex의 `total_tokens`는 input을 포함하지만
    /// 그쪽 input은 신뢰 가능하므로, M2에서 매핑할 때 같은 산식으로 맞춘다.
    pub fn xp_tokens(&self) -> i64 {
        self.cache_read_tokens + self.cache_creation_tokens + self.output_tokens
    }
}

pub trait AgentSource: Send + Sync {
    fn id(&self) -> AgentId;

    /// 로그 루트. `None`이면 홈 디렉토리를 못 찾은 것.
    fn watch_root(&self) -> Option<PathBuf>;

    /// 이 에이전트가 이 머신에 있나. 루트 존재로 판단한다.
    fn detect(&self) -> bool {
        self.watch_root().map(|p| p.exists()).unwrap_or(false)
    }

    /// 스캔 대상 파일인가. 확장자를 상위에 박지 않으려고 트레잇에 둔다.
    fn is_log_file(&self, p: &Path) -> bool {
        p.extension().and_then(|e| e.to_str()) == Some("jsonl")
    }

    /// 한 줄 → 공통 `Event`. 토큰이 안 실린 줄은 `None`.
    fn parse_line(&self, line: &str) -> Option<Event>;
}

/* ─── Claude Code ────────────────────────────────────────────────── */

pub struct ClaudeSource;

impl AgentSource for ClaudeSource {
    fn id(&self) -> AgentId {
        AgentId::Claude
    }

    fn watch_root(&self) -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".claude").join("projects"))
    }

    fn parse_line(&self, line: &str) -> Option<Event> {
        let entry = crate::jsonl::parse_line(line)?;
        // uuid 없는 줄은 dedup 키가 없어 저장할 수 없다.
        let uuid = entry.uuid?;
        let msg = entry.message.as_ref();
        let usage = msg.and_then(|m| m.usage.as_ref());
        Some(Event {
            agent: AgentId::Claude,
            uuid,
            session_id: entry.session_id.clone().unwrap_or_default(),
            timestamp: entry.timestamp.clone().unwrap_or_default(),
            model: msg.and_then(|m| m.model.clone()),
            input_tokens: usage.map(|u| u.input_tokens).unwrap_or(0),
            output_tokens: usage.map(|u| u.output_tokens).unwrap_or(0),
            cache_creation_tokens: usage.map(|u| u.cache_creation_input_tokens).unwrap_or(0),
            cache_read_tokens: usage.map(|u| u.cache_read_input_tokens).unwrap_or(0),
            project_path: entry.cwd.clone(),
            raw_content: msg.and_then(|m| m.content.clone()),
        })
    }
}

/* ─── Codex CLI (M2) ─────────────────────────────────────────────── */

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// M2 "파싱 실패율 집계" — JSON이 안 열리거나, token_count인데 우리가 아는
/// 모양이 아닌 줄만 실패로 센다. 토큰이 없는 정상 라인(response_item 등)은
/// 실패가 아니라 스킵이다. spec §10.2(포맷 변경 대비)의 조기 경보 재료.
pub static CODEX_PARSE_OK: AtomicU64 = AtomicU64::new(0);
pub static CODEX_PARSE_FAILED: AtomicU64 = AtomicU64::new(0);

pub fn codex_parse_stats() -> (u64, u64) {
    (CODEX_PARSE_OK.load(Ordering::Relaxed), CODEX_PARSE_FAILED.load(Ordering::Relaxed))
}

/// M3: rollout의 `token_count` 줄에 실려 오는 `rate_limits.primary` 최신값.
/// **추가 호출 0** — watcher가 어차피 매 줄을 읽으니 지나가는 김에 꽂아둔다.
/// 두 번째는 그 줄의 timestamp(unix초). 신선도는 벽시계가 아니라 **로그 시각**
/// 기준이다 — 초기 스캔이 옛 파일을 늦게 읽어도 옛 값이 새 값 행세를 못 한다.
static CODEX_WINDOW: Mutex<Option<(UsageWindow, i64)>> = Mutex::new(None);

/// statusLine 피드와 같은 30분. 이보다 오래됐으면 codex를 안 쓰고 있는 것이고,
/// 그땐 "최근 활동 에이전트" 규칙이 어차피 다른 쪽을 고른다.
const CODEX_WINDOW_MAX_AGE_SECS: i64 = 30 * 60;

pub fn codex_usage_window() -> Option<UsageWindow> {
    let (w, ts) = (*CODEX_WINDOW.lock().ok()?)?;
    let now = chrono::Utc::now().timestamp();
    (now - ts <= CODEX_WINDOW_MAX_AGE_SECS).then_some(w)
}

/// `payload.rate_limits.primary` → 창. 순수 함수라 따로 테스트한다.
/// `secondary`는 실측 전부 null이라 안 본다(spec §5.3).
fn parse_rate_limits(payload: &serde_json::Value) -> Option<UsageWindow> {
    let p = payload.get("rate_limits")?.get("primary")?;
    let used = p.get("used_percent")?.as_f64()? as f32;
    let window_minutes = p.get("window_minutes")?.as_u64()? as u32;
    let resets_at = p.get("resets_at").and_then(|x| x.as_i64());
    Some(UsageWindow { used_percent: used.clamp(0.0, 100.0), window_minutes, resets_at })
}

/// 프로젝트 귀속 후보에서 빼는 임시 경로 (spec §5.2 — 실측: 에이전트가 작업
/// 중 임시 폴더로 잠깐 샌다). macOS에서 `/var`는 `/private/var`의 심링크라
/// 로그에 어느 쪽으로도 찍힐 수 있어 둘 다 거른다.
fn is_transient_cwd(cwd: &str) -> bool {
    crate::platform::is_transient_cwd(cwd)
}

/// rollout 스트림에서 세션 문맥을 기억하는 상태. watcher는 소스당 스레드
/// 하나에서 파일을 순차 처리하고, 각 rollout 파일은 `session_meta`로 시작해
/// 그 세션의 줄만 담으므로 "현재 세션" 추적이 성립한다.
#[derive(Default)]
struct CodexState {
    current_session: String,
    current_model: Option<String>,
    /// session → (cwd → 등장 횟수). 최빈 cwd 귀속 (spec §5.2).
    votes: HashMap<String, HashMap<String, u32>>,
    /// session → 마지막으로 본 누적 total_tokens — last_token_usage가 없는
    /// 포맷에서 누적치를 델타로 바꾸는 데 쓴다.
    last_cum: HashMap<String, i64>,
}

impl CodexState {
    fn vote(&mut self, cwd: &str) {
        if cwd.is_empty() || is_transient_cwd(cwd) {
            return;
        }
        let sid = self.current_session.clone();
        *self.votes.entry(sid).or_default().entry(cwd.to_string()).or_insert(0) += 1;
    }

    fn modal_cwd(&self) -> Option<String> {
        self.votes
            .get(&self.current_session)?
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            .map(|(k, _)| k.clone())
    }
}

pub struct CodexSource {
    state: Mutex<CodexState>,
}

impl CodexSource {
    pub fn new() -> Self {
        CodexSource { state: Mutex::new(CodexState::default()) }
    }
}

impl AgentSource for CodexSource {
    fn id(&self) -> AgentId {
        AgentId::Codex
    }

    fn watch_root(&self) -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".codex").join("sessions"))
    }

    /// rollout-*.jsonl만 — 같은 트리의 다른 파일은 토큰 원장이 아니다.
    fn is_log_file(&self, p: &Path) -> bool {
        p.extension().and_then(|e| e.to_str()) == Some("jsonl")
            && p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-"))
    }

    fn parse_line(&self, line: &str) -> Option<Event> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            CODEX_PARSE_FAILED.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        let typ = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
        let ts = v.get("timestamp").and_then(|x| x.as_str()).unwrap_or("");
        let payload = v.get("payload");

        let mut st = self.state.lock().unwrap();
        match typ {
            "session_meta" => {
                // 파일 첫 줄 — 이후 줄들의 세션 문맥이 된다.
                let sid = payload
                    .and_then(|p| p.get("session_id").or_else(|| p.get("id")))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                st.current_session = sid.to_string();
                st.current_model = None;
                if let Some(cwd) = payload.and_then(|p| p.get("cwd")).and_then(|x| x.as_str()) {
                    st.vote(cwd);
                }
                None
            }
            "turn_context" => {
                if let Some(cwd) = payload.and_then(|p| p.get("cwd")).and_then(|x| x.as_str()) {
                    st.vote(cwd);
                }
                if let Some(m) = payload.and_then(|p| p.get("model")).and_then(|x| x.as_str()) {
                    st.current_model = Some(m.to_string());
                }
                None
            }
            "event_msg" => {
                if payload.and_then(|p| p.get("type")).and_then(|x| x.as_str())
                    != Some("token_count")
                {
                    return None;
                }
                // M3: rate_limits는 info 유무와 무관하게 실려 온다 — info를
                // 보기 전에 먼저 챙긴다. 배터리 재료가 여기서 나온다.
                if let Some(w) = payload.and_then(parse_rate_limits) {
                    let t = chrono::DateTime::parse_from_rfc3339(ts)
                        .map(|d| d.timestamp())
                        .unwrap_or(0);
                    if let Ok(mut g) = CODEX_WINDOW.lock() {
                        // 로그 시각이 더 최신일 때만 덮는다 — 초기 스캔은 파일
                        // 순서라 시간 순서가 아닐 수 있다.
                        if g.map(|(_, prev)| t >= prev).unwrap_or(true) {
                            *g = Some((w, t));
                        }
                    }
                }
                // token_count인데 info가 없으면(스트림 중간의 rate_limits-only
                // 줄) 스킵 — 실패가 아니다.
                let info = payload.and_then(|p| p.get("info"));
                let Some(info) = info.filter(|i| !i.is_null()) else {
                    return None;
                };
                let get = |u: &serde_json::Value, k: &str| -> i64 {
                    u.get(k).and_then(|x| x.as_i64()).unwrap_or(0)
                };
                let cum_total = info
                    .get("total_token_usage")
                    .map(|u| get(u, "total_tokens"))
                    .unwrap_or(0);
                let sid = st.current_session.clone();
                // 턴 단위 사용량: last_token_usage가 정본. 없으면 누적치의
                // 델타로 복원한다(포맷 관용 — spec §10.2).
                let (usage, from_delta) = match info.get("last_token_usage") {
                    Some(u) if !u.is_null() => (u.clone(), false),
                    _ => match info.get("total_token_usage") {
                        Some(u) if !u.is_null() => (u.clone(), true),
                        _ => {
                            CODEX_PARSE_FAILED.fetch_add(1, Ordering::Relaxed);
                            return None;
                        }
                    },
                };
                let prev = if from_delta { *st.last_cum.get(&sid).unwrap_or(&0) } else { 0 };
                st.last_cum.insert(sid.clone(), cum_total);
                if from_delta && prev > 0 {
                    // 누적-only 포맷의 두 번째 이후 줄: 필드별 이전 누적이
                    // 없어 델타를 복원할 근거가 없다(총량 비례 분배는 지어낸
                    // 숫자다). 실패로 세서 배지로 드러나게 한다 — spec §10.2.
                    // (첫 줄은 누적==델타라 그대로 쓴다.)
                    CODEX_PARSE_FAILED.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
                let input = get(&usage, "input_tokens");
                let cached = get(&usage, "cached_input_tokens");
                let cache_write = get(&usage, "cache_write_input_tokens");
                let output = get(&usage, "output_tokens");
                if input == 0 && cached == 0 && cache_write == 0 && output == 0 {
                    return None; // 빈 카운트 — 스킵
                }
                CODEX_PARSE_OK.fetch_add(1, Ordering::Relaxed);
                Some(Event {
                    agent: AgentId::Codex,
                    // rollout 줄엔 uuid가 없다 — 세션+타임스탬프+누적총량으로
                    // 결정론 합성(누적은 세션 내 단조증가라 재스캔 dedup에 안전).
                    uuid: format!("cx-{sid}-{ts}-{cum_total}"),
                    session_id: sid,
                    timestamp: ts.to_string(),
                    model: st.current_model.clone(),
                    // Codex input은 신뢰 가능하지만 XP 산식(§6.1)은 에이전트
                    // 공통으로 input을 제외한다 — 원장 보존용으로만 싣는다.
                    input_tokens: input,
                    output_tokens: output,
                    cache_creation_tokens: cache_write,
                    cache_read_tokens: cached,
                    project_path: st.modal_cwd(),
                    // Codex 도구 블록(custom_tool_call)은 token_count 줄에
                    // 없다 — thrash 피드는 Codex에서 조용히 비활성(정규화는
                    // 후속, M1 노트).
                    raw_content: None,
                })
            }
            _ => None,
        }
    }
}

/// 이 머신에서 감시할 소스들.
pub fn sources() -> Vec<Box<dyn AgentSource>> {
    vec![Box::new(ClaudeSource), Box::new(CodexSource::new())]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// XP 화폐가 `input_tokens`를 빼는지 — spec §6.1의 핵심 계약.
    #[test]
    fn xp_excludes_input_tokens() {
        let e = Event {
            agent: AgentId::Claude,
            uuid: "u".into(),
            session_id: String::new(),
            timestamp: String::new(),
            model: None,
            input_tokens: 999_999, // 못 쓰는 값 — 결과에 영향 없어야 한다
            output_tokens: 3,
            cache_creation_tokens: 20,
            cache_read_tokens: 100,
            project_path: None,
            raw_content: None,
        };
        assert_eq!(e.xp_tokens(), 123);
    }

    /// Claude 실제 로그 한 줄이 그대로 정규화되는지.
    #[test]
    fn claude_parses_usage_line() {
        let line = r#"{"uuid":"abc","sessionId":"s1","timestamp":"2026-09-01T00:00:00Z",
            "cwd":"/tmp/proj","message":{"role":"assistant","model":"sonnet",
            "usage":{"input_tokens":7,"output_tokens":11,
                     "cache_creation_input_tokens":22,"cache_read_input_tokens":333}}}"#;
        let e = ClaudeSource.parse_line(line).expect("파싱돼야 함");
        assert_eq!(e.agent.as_str(), "claude");
        assert_eq!(e.uuid, "abc");
        assert_eq!(e.project_path.as_deref(), Some("/tmp/proj"));
        assert_eq!(e.cache_read_tokens, 333);
        assert_eq!(e.xp_tokens(), 333 + 22 + 11);
    }

    /// M3: rollout `rate_limits.primary`가 창으로 나오는지 — 실측 페이로드 모양.
    #[test]
    fn codex_rate_limits_primary_becomes_window() {
        let p = serde_json::json!({"type":"token_count","info":null,
            "rate_limits":{"limit_id":"codex","primary":{"used_percent":42.0,
                "window_minutes":10080,"resets_at":1787035858},"secondary":null}});
        let w = parse_rate_limits(&p).expect("primary가 있으면 창이 나와야 함");
        assert_eq!(w.window_minutes, 10080, "실측 주간 창");
        assert_eq!(w.resets_at, Some(1787035858));
        assert!((w.used_percent - 42.0).abs() < 1e-6);
    }

    /// rate_limits가 없거나 primary가 null이면 창도 없다 — 지어내지 않는다.
    #[test]
    fn codex_rate_limits_missing_is_none() {
        assert!(parse_rate_limits(&serde_json::json!({"type":"token_count"})).is_none());
        assert!(parse_rate_limits(&serde_json::json!({"rate_limits":{"primary":null}})).is_none());
    }

    /// uuid 없는 줄은 버린다 — dedup 키가 없다.
    #[test]
    fn claude_skips_line_without_uuid() {
        assert!(ClaudeSource.parse_line(r#"{"timestamp":"x"}"#).is_none());
    }

    /* ─── Codex (M2) ─────────────────────────────────────────── */

    /// 실측 rollout 흐름 그대로: session_meta(임시 cwd) → turn_context(진짜
    /// cwd+model) → token_count(last_token_usage). 매핑(spec §5.2)·최빈 cwd
    /// 귀속(임시 제외)·uuid 합성까지 한 번에 게이트.
    #[test]
    fn codex_parses_token_count_with_modal_cwd() {
        let s = CodexSource::new();
        assert!(s
            .parse_line(r#"{"timestamp":"2026-09-01T00:00:00.000Z","type":"session_meta","payload":{"session_id":"s-1","cwd":"/var/folders/tf/xyz/T"}}"#)
            .is_none());
        assert!(s
            .parse_line(r#"{"timestamp":"2026-09-01T00:00:01.000Z","type":"turn_context","payload":{"cwd":"/Users/me/proj","model":"gpt-5.5-codex"}}"#)
            .is_none());
        let e = s
            .parse_line(r#"{"timestamp":"2026-09-01T00:00:02.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":80,"cache_write_input_tokens":5,"output_tokens":40,"total_tokens":145},"last_token_usage":{"input_tokens":100,"cached_input_tokens":80,"cache_write_input_tokens":5,"output_tokens":40,"total_tokens":145}}}}"#)
            .expect("token_count는 Event가 돼야 함");
        assert_eq!(e.agent.as_str(), "codex");
        assert_eq!(e.session_id, "s-1");
        assert_eq!(e.cache_read_tokens, 80); // cached_input_tokens
        assert_eq!(e.cache_creation_tokens, 5); // cache_write_input_tokens
        assert_eq!(e.output_tokens, 40);
        assert_eq!(e.xp_tokens(), 80 + 5 + 40); // input 제외 — 공통 산식
        assert_eq!(e.project_path.as_deref(), Some("/Users/me/proj")); // 임시 cwd 제외
        assert_eq!(e.model.as_deref(), Some("gpt-5.5-codex"));
        assert_eq!(e.uuid, "cx-s-1-2026-09-01T00:00:02.000Z-145");
    }

    /// 임시 경로만 있는 세션(실측: 이 머신의 easel SDK 세션)은 귀속 없음.
    #[test]
    fn codex_transient_only_session_has_no_project() {
        let s = CodexSource::new();
        s.parse_line(r#"{"timestamp":"t0","type":"session_meta","payload":{"session_id":"s-2","cwd":"/private/var/folders/ab/T"}}"#);
        let e = s
            .parse_line(r#"{"timestamp":"t1","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1,"cached_input_tokens":0,"output_tokens":2,"total_tokens":3},"total_token_usage":{"total_tokens":3}}}}"#)
            .expect("event");
        assert_eq!(e.project_path, None);
    }

    /// info 없는 token_count(rate_limits-only 줄)는 실패가 아니라 스킵.
    /// 누적-only 포맷은 첫 줄만 쓰고 두 번째부터는 실패로 센다(§10.2 배지 재료).
    #[test]
    fn codex_tolerant_fallbacks() {
        let s = CodexSource::new();
        s.parse_line(r#"{"timestamp":"t0","type":"session_meta","payload":{"session_id":"s-3","cwd":"/Users/me/p"}}"#);
        let failed_before = CODEX_PARSE_FAILED.load(Ordering::Relaxed);
        assert!(s
            .parse_line(r#"{"timestamp":"t1","type":"event_msg","payload":{"type":"token_count","info":null}}"#)
            .is_none());
        assert_eq!(CODEX_PARSE_FAILED.load(Ordering::Relaxed), failed_before, "info null은 스킵");
        // 누적-only 첫 줄 = 델타로 사용 가능
        let e = s
            .parse_line(r#"{"timestamp":"t2","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":4,"output_tokens":6,"total_tokens":16}}}}"#)
            .expect("누적-only 첫 줄은 그대로 델타");
        assert_eq!(e.xp_tokens(), 4 + 6);
        // 누적-only 두 번째 줄 = 복원 불가 → 실패 카운트
        assert!(s
            .parse_line(r#"{"timestamp":"t3","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":20,"cached_input_tokens":8,"output_tokens":12,"total_tokens":32}}}}"#)
            .is_none());
        assert_eq!(CODEX_PARSE_FAILED.load(Ordering::Relaxed), failed_before + 1);
    }

    /// M2 검증 도구 — 실제 rollout 전량을 CodexSource로 흘려 파싱 결과를
    /// 집계한다. 크래시 0 + 실패율 기록이 게이트(plan MVP-2). 회사 맥북프로
    /// 코퍼스(292개)에서 돌리는 게 본판이고, 다른 머신에선 스모크.
    ///   cargo test --lib dump_codex_ingest -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_codex_ingest() {
        use std::io::BufRead;
        let s = CodexSource::new();
        let Some(root) = s.watch_root().filter(|p| p.exists()) else {
            eprintln!("~/.codex/sessions 없음 — 이 머신엔 Codex 데이터가 없다");
            return;
        };
        let (mut files, mut lines, mut events, mut xp) = (0u32, 0u64, 0u64, 0i64);
        let mut by_project: HashMap<String, i64> = HashMap::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if s.is_log_file(&p) {
                    files += 1;
                    let Ok(f) = std::fs::File::open(&p) else { continue };
                    for line in std::io::BufReader::new(f).lines().map_while(|l| l.ok()) {
                        lines += 1;
                        if let Some(ev) = s.parse_line(&line) {
                            events += 1;
                            xp += ev.xp_tokens();
                            *by_project.entry(ev.project_path.unwrap_or_else(|| "(없음)".into())).or_insert(0) += ev.xp_tokens();
                        }
                    }
                }
            }
        }
        let (ok, failed) = codex_parse_stats();
        eprintln!("\n=== codex ingest: files {files} · lines {lines} · events {events} · xp {xp} ===");
        eprintln!("parse ok {ok} · failed {failed} (실패율 {:.2}%)", 100.0 * failed as f64 / (ok + failed).max(1) as f64);
        let mut v: Vec<_> = by_project.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        for (proj, t) in v.into_iter().take(10) {
            eprintln!("{t:>14} | {proj}");
        }
    }

    /// rollout-*.jsonl만 원장이다 — history.jsonl 등은 제외.
    #[test]
    fn codex_log_file_filter() {
        let s = CodexSource::new();
        assert!(s.is_log_file(Path::new("/x/2026/09/01/rollout-2026-09-01T00-00-00-abc.jsonl")));
        assert!(!s.is_log_file(Path::new("/x/history.jsonl")));
        assert!(!s.is_log_file(Path::new("/x/rollout-abc.txt")));
    }
}
