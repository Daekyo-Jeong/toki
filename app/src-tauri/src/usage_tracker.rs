//! Active 5-hour block tracker via ccusage.
//!
//! Anthropic's quota system uses fixed 5-hour blocks aligned to the hour
//! of the first message — not a sliding window. The ccusage CLI
//! (https://github.com/ryoppippi/ccusage) parses the same JSONL files
//! Toki watches and reports the active block's tokenCounts authoritatively.
//!
//! Strategy:
//! - Cache 30s in-process (ccusage subprocess ~1s cold, would dominate tick).
//! - Try `ccusage` first (user globally installed), then `npx -y ccusage@latest`.
//! - On any failure, fall back to a sliding-window SQL approximation so
//!   HP keeps moving even without node available.
//!
//! Returns total tokens (input + output + cache_creation + cache_read)
//! matching what Claude Code's quota UI displays.

use anyhow::Result;
use serde::Deserialize;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::agent::UsageWindow;
use crate::db::Db;
use crate::oauth_usage::OauthUsageTracker;

/// Shorter TTL keeps Toki's HP closer to Claude Code's quota UI at the
/// cost of ~10% CPU time on the ccusage subprocess (~1s each, called 6×/min).
const CACHE_TTL: Duration = Duration::from_secs(10);
const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Authoritative — Claude Code's own `/api/oauth/usage` (V4-4).
    OauthUsage,
    /// M3: Codex rollout JSONL에 내장된 `rate_limits.primary`. 추가 호출 0.
    CodexRollout,
    Ccusage,
    NpxCcusage,
    SqliteFallback,
}

/// 지금 값을 아는 에이전트 하나. 배터리는 하나만 따르지만(spec §5.3), 혼용
/// 사용자는 **나머지도 보고 싶다** — 배터리가 왜 그 칸수인지 설명해준다
/// (2026-09-04: "내 claude는 9%인데 왜 2칸이지?").
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentUsage {
    pub agent: &'static str,
    pub used_percent: f32,
    pub window_minutes: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Snapshot {
    pub active_block_tokens: i64,
    pub source: Source,
    /// Burn rate (tokens/min) within the active block. None when source
    /// is SqliteFallback (we don't compute it there).
    pub burn_rate_tpm: Option<f64>,
    /// Projected total tokens by end of current block. None if no projection.
    pub projection_total: Option<i64>,
    /// User's historical peak 5h block usage — the personal ceiling that
    /// HP% is calculated against. 0 if unknown (first run, no history).
    pub token_limit: i64,
    /// Minutes until the current block expires (and HP recovers as a new
    /// block starts at 0 tokens). None when source is SqliteFallback.
    pub block_remaining_minutes: Option<i64>,
    /// RFC3339 timestamp of block end. None when source is SqliteFallback.
    pub block_end_time: Option<String>,
    /// M3: 이 값이 어느 길이의 창에서 왔나(분). 소스가 창을 직접 준 경우만
    /// Some — 디버깅·폴백 판단용이고 **UI엔 표시하지 않는다**(spec §5.3).
    pub window_minutes: Option<u32>,
    /// 값을 아는 에이전트 전부. 배터리는 하나만 따르지만 툴팁은 다 보여준다.
    #[serde(default)]
    pub agents: Vec<AgentUsage>,
}

pub struct UsageTracker {
    db: Arc<Db>,
    cached: Option<(Snapshot, Instant)>,
    oauth: OauthUsageTracker,
}

impl UsageTracker {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db, cached: None, oauth: OauthUsageTracker::new() }
    }

    pub fn get(&mut self) -> Snapshot {
        if let Some((snap, fetched_at)) = &self.cached {
            if fetched_at.elapsed() < CACHE_TTL {
                return snap.clone();
            }
        }
        let snap = self.fetch();
        self.cached = Some((snap.clone(), Instant::now()));
        snap
    }

    fn fetch(&mut self) -> Snapshot {
        // 1순위 — 소스가 직접 준 사용률 창 (spec §5.3). Claude는 statusLine →
        // oauth → 디스크 캐시 사슬, Codex는 rollout에 내장된 rate_limits. 둘 다
        // 있으면 **최근에 활동한 에이전트**를 따른다 — 펫의 허기는 지금 하고
        // 있는 일에서 온다. used_percent를 tokens/limit=100으로 실어
        // game::hp_for_block_usage에 산식 변경 없이 넣는다.
        let recent = self.db.last_active_agent().ok().flatten();
        let claude = self.oauth.get().map(|u| u.window());
        let codex = crate::agent::codex_usage_window();
        // 배터리가 고르는 것과 별개로, 값을 아는 에이전트는 전부 실어 보낸다.
        let mut agents: Vec<AgentUsage> = Vec::new();
        if let Some(w) = claude {
            agents.push(AgentUsage { agent: "claude", used_percent: w.used_percent, window_minutes: w.window_minutes });
        }
        if let Some(w) = codex {
            agents.push(AgentUsage { agent: "codex", used_percent: w.used_percent, window_minutes: w.window_minutes });
        }
        if let Some((w, source)) = pick_window(recent.as_deref(), claude, codex) {
            return Snapshot {
                agents,
                active_block_tokens: w.used_percent.round() as i64,
                source,
                burn_rate_tpm: None,
                projection_total: None,
                token_limit: 100,
                block_remaining_minutes: None,
                block_end_time: None,
                window_minutes: Some(w.window_minutes),
            };
        }
        // 아래는 전부 Claude 전용 **근사**(ccusage / 5h 블록 sqlite) — 소스가
        // 창을 못 준 경우의 빈칸 방지용이지 정확도를 기대하는 값이 아니다.
        if let Some(s) = call_ccusage("ccusage", &[]).and_then(parse_active) {
            return snapshot_from(Source::Ccusage, s);
        }
        if let Some(s) =
            call_ccusage("npx", &["-y", "ccusage@latest"]).and_then(parse_active)
        {
            return snapshot_from(Source::NpxCcusage, s);
        }
        // 최종 폴백 — 우리 DB만으로 계산한 (현재 5h 블록 / 개인 최대 블록).
        // 예전엔 여기서 token_limit=0을 냈고, UI는 limit 0을 null로 처리하므로
        // oauth가 429 + ccusage 미설치인 사용자에겐 배터리가 통째로 빈칸이
        // 됐다(2026-08-31 실사고). 이제 천장을 자체 계산해 항상 값이 나온다.
        let (cur, peak) = self.db.five_hour_block_and_peak().unwrap_or((0, 0));
        // peak을 못 구하면(첫 실행 등) 슬라이딩 합계라도 내보낸다 — 이때만
        // limit 0(=UI 빈칸)이고, 그건 정말로 데이터가 없는 상태다.
        if peak <= 0 {
            return Snapshot {
                active_block_tokens: self.db.last_5h_total_tokens().unwrap_or(0),
                source: Source::SqliteFallback,
                burn_rate_tpm: None,
                projection_total: None,
                token_limit: 0,
                block_remaining_minutes: None,
                block_end_time: None,
                window_minutes: None,
                agents,
            };
        }
        Snapshot {
            active_block_tokens: cur,
            source: Source::SqliteFallback,
            burn_rate_tpm: None,
            projection_total: None,
            token_limit: peak,
            block_remaining_minutes: None,
            block_end_time: None,
            window_minutes: None,
            agents,
        }
    }
}

fn snapshot_from(source: Source, parsed: ParsedActive) -> Snapshot {
    Snapshot {
        active_block_tokens: parsed.total_tokens,
        source,
        burn_rate_tpm: parsed.burn_rate_tpm,
        projection_total: parsed.projection_total,
        token_limit: parsed.token_limit,
        block_remaining_minutes: parsed.remaining_minutes,
        block_end_time: parsed.end_time,
        window_minutes: None,
        // ccusage/로컬 근사 경로 — 에이전트별 실측 창이 없다.
        agents: Vec::new(),
    }
}

/// 최근 활동 에이전트의 창을 우선하고, 없으면 다른 쪽으로 폴백. 순수 함수.
/// `recent`가 미상이거나 데이터가 없으면 Claude 우선 — v4 동작을 보존한다.
fn pick_window(
    recent: Option<&str>,
    claude: Option<UsageWindow>,
    codex: Option<UsageWindow>,
) -> Option<(UsageWindow, Source)> {
    let c = claude.map(|w| (w, Source::OauthUsage));
    let x = codex.map(|w| (w, Source::CodexRollout));
    match recent {
        Some("codex") => x.or(c),
        _ => c.or(x),
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;

    fn w(pct: f32, min: u32) -> UsageWindow {
        UsageWindow { used_percent: pct, window_minutes: min, resets_at: None }
    }

    #[test]
    fn recent_codex_prefers_codex_window() {
        let (win, src) = pick_window(Some("codex"), Some(w(10.0, 300)), Some(w(42.0, 10080))).unwrap();
        assert_eq!(src, Source::CodexRollout);
        assert_eq!(win.window_minutes, 10080);
    }

    #[test]
    fn recent_claude_or_unknown_prefers_claude() {
        for recent in [Some("claude"), None, Some("???")] {
            let (_, src) = pick_window(recent, Some(w(10.0, 300)), Some(w(42.0, 10080))).unwrap();
            assert_eq!(src, Source::OauthUsage, "recent={recent:?}");
        }
    }

    #[test]
    fn falls_back_to_whichever_exists() {
        assert_eq!(pick_window(Some("codex"), Some(w(10.0, 300)), None).unwrap().1, Source::OauthUsage);
        assert_eq!(pick_window(Some("claude"), None, Some(w(42.0, 10080))).unwrap().1, Source::CodexRollout);
        assert!(pick_window(Some("codex"), None, None).is_none());
    }
}

fn call_ccusage(bin: &str, prefix_args: &[&str]) -> Option<Vec<u8>> {
    let mut cmd = Command::new(bin);
    crate::platform::quiet(&mut cmd);
    for a in prefix_args {
        cmd.arg(a);
    }
    if bin != "ccusage" && prefix_args.is_empty() {
        // direct ccusage path
    }
    // Final ccusage args. When bin == "npx", "ccusage" comes via prefix_args.
    if bin == "ccusage" {
        cmd.arg("blocks");
    } else {
        // npx -y ccusage@latest blocks ...
        cmd.arg("blocks");
    }
    // --token-limit max: ccusage auto-derives the ceiling from the user's
    // historical peak 5h block. Embedded in the tokenLimitStatus field.
    cmd.args(["--json", "--active", "--offline", "--token-limit", "max"]);

    // Spawn and apply a timeout manually. stdout은 **파일로** 받는다 — 파이프로
    // 열고 종료 후에 읽는 구조는 자식이 버퍼를 채우면 교착이다(planner.rs 주석
    // 참고. 2026-09-02 codex 경로에서 실제로 물렸다). 여기 출력(ccusage --active)은
    // 작아서 아직 안 물렸지만 같은 결함이라 같이 고친다.
    let out_file = crate::planner::TempCapture::new("ccusage").ok()?;
    let mut child = match cmd
        .stdout(out_file.stdio().ok()?)
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return None,
    };

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                return Some(out_file.read().into_bytes());
            }
            Ok(None) => {
                if start.elapsed() > SUBPROCESS_TIMEOUT {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
}

#[derive(Deserialize)]
struct BlocksOut {
    blocks: Vec<Block>,
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "isActive", default)]
    is_active: bool,
    #[serde(rename = "totalTokens", default)]
    total_tokens: i64,
    #[serde(rename = "endTime")]
    end_time: Option<String>,
    #[serde(rename = "burnRate")]
    burn_rate: Option<BurnRate>,
    projection: Option<Projection>,
    #[serde(rename = "tokenLimitStatus")]
    token_limit_status: Option<TokenLimitStatus>,
}

#[derive(Deserialize)]
struct TokenLimitStatus {
    #[serde(default)]
    limit: i64,
}

struct ParsedActive {
    total_tokens: i64,
    burn_rate_tpm: Option<f64>,
    projection_total: Option<i64>,
    token_limit: i64,
    remaining_minutes: Option<i64>,
    end_time: Option<String>,
}

#[derive(Deserialize)]
struct BurnRate {
    #[serde(rename = "tokensPerMinute")]
    tokens_per_minute: f64,
}

#[derive(Deserialize)]
struct Projection {
    #[serde(rename = "totalTokens", default)]
    total_tokens: i64,
    #[serde(rename = "remainingMinutes")]
    remaining_minutes: Option<i64>,
}

fn parse_active(json: Vec<u8>) -> Option<ParsedActive> {
    let parsed: BlocksOut = serde_json::from_slice(&json).ok()?;
    let active = parsed.blocks.into_iter().find(|b| b.is_active)?;
    let projection = active.projection;
    Some(ParsedActive {
        total_tokens: active.total_tokens,
        burn_rate_tpm: active.burn_rate.map(|b| b.tokens_per_minute),
        projection_total: projection.as_ref().map(|p| p.total_tokens),
        token_limit: active.token_limit_status.map(|t| t.limit).unwrap_or(0),
        remaining_minutes: projection.and_then(|p| p.remaining_minutes),
        end_time: active.end_time,
    })
}

#[allow(dead_code)]
fn _trait_for_anyhow(_: &dyn Fn() -> Result<()>) {}

#[cfg(test)]
mod fallback_check {
    /// 실 DB로 최종 폴백(개인 천장 기반 5h 블록 비율)을 확인:
    ///   cargo test --lib sqlite_fallback_gives_pct -- --ignored --nocapture
    #[test]
    #[ignore]
    fn sqlite_fallback_gives_pct() {
        let path = dirs::home_dir().unwrap().join(".toki").join("engine.sqlite");
        let db = crate::db::Db::open(&path).expect("db open");
        let (cur, peak) = db.five_hour_block_and_peak().expect("query");
        eprintln!(
            "현재 블록 {} · 개인 최대 {} · 배터리 {:.1}%",
            cur,
            peak,
            if peak > 0 { cur as f64 / peak as f64 * 100.0 } else { 0.0 }
        );
        assert!(peak > 0, "개인 최대 블록이 0이면 폴백이 여전히 빈칸을 낸다");
    }
}
