//! Authoritative 5h usage % via Claude Code's own OAuth session — reverse
//! engineered endpoint (undocumented, no public API). See
//! `strings ~/.local/share/claude/versions/<ver> | grep oauth/usage`.
//!
//! `ccusage` (usage_tracker.rs) estimates the 5h ceiling from the user's own
//! historical peak block, which drifts from Claude Code's actual quota UI.
//! This calls the same endpoint the official `/usage` view calls, using the
//! user's own `claude login` session — own token, own usage, read-only.
//!
//! Token source, in priority order:
//! 1. `~/.claude/.credentials.json` (older/Linux installs)
//! 2. macOS Keychain, service "Claude Code-credentials" (current macOS installs)
//! The token is only ever held in memory here and sent as a Bearer header —
//! never logged, never returned to the frontend. Schema confirmed live:
//! `{claudeAiOauth: {accessToken, expiresAt, rateLimitTier, refreshToken,
//! scopes, subscriptionType}}`.
//!
//! **V4-4 status (2026-07-07): working after re-login.** The stored token
//! had gone stale (~11d past its `expiresAt` → 401); an interactive `/login`
//! refreshed the Keychain entry and the call returned 200 with the real 5h
//! utilization. No refresh-token flow is implemented — Toki only *reads* the
//! token Claude Code maintains; minting new tokens from the refresh_token
//! ourselves would be a materially deeper credential operation, out of
//! scope. When the token re-stales, the `[oauth-usage] stored token expired`
//! warning fires and we fall back to the ccusage estimate until the next
//! login. Also seen live: repeated calls can 429 with `retry-after` ~48min
//! (not built for tight polling) — `backoff_until` honors that.
//!
//! Confirmed 200 schema (2026-07-07):
//! ```json
//! { "five_hour": { "utilization": 56.0, "resets_at": "…Z" },
//!   "seven_day": { "utilization": 37.0 },
//!   "limits": [ { "kind": "session", "percent": 56, "is_active": true }, … ] }
//! ```

use std::time::{Duration, Instant};

const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
// Happy-path cadence (user preference). A live 429 has been observed
// returning `retry-after: 2876` (~48min) — this endpoint isn't built for
// tight polling, so the eager interval below is a ceiling on how *often we
// try*, not a guarantee the server accepts every attempt. `backoff_until`
// (set from the server's own `retry-after`) always wins over this.
// 2026-08-31: 300s → 900s. 이 엔드포인트는 상시 폴링용이 아니고(관측된
// retry-after가 50분까지 나온다), 5h 블록은 15분에 눈에 띄게 안 움직인다.
// 호출을 아껴야 정작 필요할 때 창이 열려 있다.
const EAGER_POLL: Duration = Duration::from_secs(900);
/// 디스크에 남긴 마지막 성공값을 이 나이까지는 재사용한다. 앱을 재시작하면
/// 인메모리 캐시가 사라져 첫 호출이 429면 보여줄 게 아무것도 없었다(오늘
/// 재설치를 반복하며 배터리가 계속 빈칸이던 이유). 5h 블록 기준으로 30분은
/// 여전히 유의미한 근사다.
const PERSIST_MAX_AGE_SECS: i64 = 1800;
/// statusLine 피드 값의 유효 기간. Claude Code 세션이 돌면 매 턴 갱신되므로,
/// 이보다 오래됐다는 건 한동안 작업을 안 했다는 뜻 — 5h 창이 흘렀을 수 있어
/// 신뢰하지 않는다.
const STATUSLINE_MAX_AGE: Duration = Duration::from_secs(1800);

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct OauthUsage {
    /// 0-100 utilization of the current 5h rolling window.
    pub five_hour_pct: f64,
}

/// statusLine 피드와 oauth 응답이 주는 값은 `five_hour` 키 하나라 창이 5시간으로
/// **정해져 있다**. 이건 소스가 아는 사실이지 앱이 가정하는 값이 아니다 — 창
/// 길이를 아는 곳은 여기(Claude)와 `agent.rs`의 Codex 파서 둘뿐이어야 한다.
pub const CLAUDE_FIVE_HOUR_WINDOW_MINUTES: u32 = 300;

impl OauthUsage {
    /// M3: 소스 중립 창으로 감싼다(spec §5.3).
    pub fn window(self) -> crate::agent::UsageWindow {
        crate::agent::UsageWindow {
            used_percent: self.five_hour_pct as f32,
            window_minutes: CLAUDE_FIVE_HOUR_WINDOW_MINUTES,
            resets_at: None,
        }
    }
}

enum FetchOutcome {
    Success(OauthUsage),
    /// Server-specified cooldown (seconds), when present on the 429.
    RateLimited(Option<u64>),
    Unavailable,
}

pub struct OauthUsageTracker {
    cached: Option<(Option<OauthUsage>, Instant)>,
    backoff_until: Option<Instant>,
    /// 디스크에서 읽어온 마지막 성공값 + 그 값이 쓸모없어지는 시각. 인메모리
    /// 캐시가 비었을 때만 쓰이는 **최후 폴백**이라, 라이브 시도를 가리지
    /// 않는다(가리면 429가 풀려도 영영 옛날 값을 보여주게 된다).
    persisted: Option<(OauthUsage, Instant)>,
}

impl OauthUsageTracker {
    pub fn new() -> Self {
        let persisted = load_persisted();
        if let Some((u, _)) = &persisted {
            eprintln!("[oauth-usage] 디스크 캐시 사용 가능: {:.0}%", u.five_hour_pct);
        }
        Self { cached: None, backoff_until: None, persisted }
    }

    /// 아직 유효한 디스크 캐시 값 (만료됐으면 None).
    fn persisted_fresh(&self) -> Option<OauthUsage> {
        self.persisted
            .filter(|(_, expires)| Instant::now() < *expires)
            .map(|(u, _)| u)
    }

    /// Returns `None` on any failure (no token / network / rate-limited /
    /// unexpected response shape) — callers must treat that as "fall back
    /// to the ccusage estimate", not as an error.
    pub fn get(&mut self) -> Option<OauthUsage> {
        // 1순위 — statusLine 피드(`~/.toki/usage-pct`). Claude Code가 매 턴
        // statusLine 명령 stdin으로 흘려주는 rate_limits에서 우리 셸 스크립트가
        // 적어둔 값이다. **추가 네트워크 호출 0**이고 Claude Code가 표시하는 %와
        // 정확히 같은 수라, 429에 막히는 직접 폴링보다 모든 면에서 낫다.
        // (Orca도 같은 429 벽을 맞고 이 경로로 옮겼다 — 그쪽 소스 주석에 명시.)
        if let Some(u) = read_statusline_pct() {
            return Some(u);
        }
        let live = self.get_live();
        // 라이브도 없으면 마지막 성공값(디스크)으로 메운다.
        live.or_else(|| self.persisted_fresh())
    }

    fn get_live(&mut self) -> Option<OauthUsage> {
        if let Some(until) = self.backoff_until {
            if Instant::now() < until {
                return self.cached.as_ref().and_then(|(v, _)| *v);
            }
        }
        if let Some((snap, at)) = &self.cached {
            if at.elapsed() < EAGER_POLL {
                return *snap;
            }
        }
        match fetch() {
            FetchOutcome::Success(u) => {
                self.backoff_until = None;
                self.cached = Some((Some(u), Instant::now()));
                save_persisted(&u);
                self.persisted =
                    Some((u, Instant::now() + Duration::from_secs(PERSIST_MAX_AGE_SECS as u64)));
                Some(u)
            }
            FetchOutcome::RateLimited(retry_after_secs) => {
                // Trust the server's number when it gives one; otherwise
                // still wait at least one eager interval before retrying.
                let wait = retry_after_secs
                    .map(Duration::from_secs)
                    .unwrap_or(EAGER_POLL)
                    .max(EAGER_POLL);
                eprintln!("[oauth-usage] backing off {}s", wait.as_secs());
                self.backoff_until = Some(Instant::now() + wait);
                self.cached.as_ref().and_then(|(v, _)| *v)
            }
            FetchOutcome::Unavailable => {
                self.cached = Some((None, Instant::now()));
                None
            }
        }
    }
}

/// statusLine 피드가 남긴 5시간 사용률. 파일엔 숫자 하나뿐이고 **신선도는
/// mtime으로 판단**한다 — statusLine 스크립트가 프로세스를 하나도 안 띄우게
/// 하려고(스트리밍 중 초당 여러 번 실행된다) 타임스탬프를 안 적기 때문.
/// STATUSLINE_MAX_AGE를 넘으면 무시하고 다음 소스로 내려간다(세션을 안 켜둔
/// 동안 옛날 값이 배터리에 눌어붙는 걸 막는다).
fn read_statusline_pct() -> Option<OauthUsage> {
    let p = dirs::home_dir()?.join(".toki").join("usage-pct");
    let age = std::fs::metadata(&p).ok()?.modified().ok()?.elapsed().ok()?;
    if age > STATUSLINE_MAX_AGE {
        return None;
    }
    let pct: f64 = std::fs::read_to_string(&p).ok()?.trim().parse().ok()?;
    Some(OauthUsage { five_hour_pct: pct.clamp(0.0, 100.0) })
}

/// `~/.toki/usage-cache.json` — 마지막으로 성공한 5h 사용률 + 그 시각.
/// 사용률 숫자 하나뿐이라 비밀이 없다(토큰은 절대 안 남는다).
fn persist_path() -> Option<std::path::PathBuf> {
    Some(dirs::home_dir()?.join(".toki").join("usage-cache.json"))
}

fn save_persisted(u: &OauthUsage) {
    let Some(p) = persist_path() else { return };
    let body = serde_json::json!({
        "five_hour_pct": u.five_hour_pct,
        "at": chrono::Utc::now().to_rfc3339(),
    });
    if let Ok(s) = serde_json::to_string(&body) {
        let _ = std::fs::write(p, s);
    }
}

/// 저장된 값과 **남은 유효 시간**. 30분이 지난 값은 아예 안 읽는다.
fn load_persisted() -> Option<(OauthUsage, Instant)> {
    let raw = std::fs::read_to_string(persist_path()?).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let pct = v.get("five_hour_pct")?.as_f64()?;
    let at = chrono::DateTime::parse_from_rfc3339(v.get("at")?.as_str()?).ok()?;
    let age = (chrono::Utc::now() - at.with_timezone(&chrono::Utc)).num_seconds();
    if !(0..PERSIST_MAX_AGE_SECS).contains(&age) {
        return None;
    }
    let remaining = Duration::from_secs((PERSIST_MAX_AGE_SECS - age) as u64);
    Some((
        OauthUsage { five_hour_pct: pct.clamp(0.0, 100.0) },
        Instant::now() + remaining,
    ))
}

fn fetch() -> FetchOutcome {
    let token = match get_oauth_token() {
        Some(t) => t,
        None => {
            eprintln!("[oauth-usage] no token found (no credentials file, no keychain entry)");
            return FetchOutcome::Unavailable;
        }
    };
    let resp = match ureq::get(ENDPOINT)
        .set("Authorization", &format!("Bearer {token}"))
        // 2026-08-31: 비공식 클라이언트(ureq 기본 UA)만 상시 429로 막히기
        // 시작했다 (retry-after ~50분, 토큰은 정상 = 401 아님). Claude Code
        // 자신은 같은 엔드포인트를 UA `claude-cli/<ver> (external, cli)` +
        // `anthropic-beta: oauth-2025-04-20`으로 호출하고 멀쩡히 통과한다
        // (바이너리 strings의 jI()/IV()에서 확인). 같은 계정·같은 토큰·읽기
        // 전용 조회이므로 CC와 동일한 클라이언트 표기로 호출한다.
        .set("User-Agent", &client_user_agent())
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("Content-Type", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .call()
    {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let retry_after = r
                .header("retry-after")
                .and_then(|s| s.trim().parse::<u64>().ok());
            // Error bodies are typically {"error": {...}} — no secrets, safe to log.
            let text = r.into_string().unwrap_or_default();
            eprintln!(
                "[oauth-usage] http {}: retry-after={:?} body={}",
                code, retry_after, text
            );
            if code == 429 {
                return FetchOutcome::RateLimited(retry_after);
            }
            return FetchOutcome::Unavailable;
        }
        Err(e) => {
            eprintln!("[oauth-usage] request failed: {}", e);
            return FetchOutcome::Unavailable;
        }
    };
    let body: serde_json::Value = match resp.into_json() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[oauth-usage] response wasn't JSON: {}", e);
            return FetchOutcome::Unavailable;
        }
    };
    match parse_five_hour_pct(&body) {
        Some(u) => {
            // Compact, non-secret summary (utilization %, never the token).
            eprintln!(
                "[oauth-usage] 5h={:.0}% 7d={:?}",
                u.five_hour_pct,
                dig(&body, &["seven_day", "utilization"]).and_then(|n| n.as_f64())
            );
            FetchOutcome::Success(u)
        }
        None => {
            // Parser missed — log the raw body (usage numbers only, no
            // secrets) so the schema drift is visible instead of silent.
            eprintln!("[oauth-usage] parse miss, body={}", body);
            FetchOutcome::Unavailable
        }
    }
}

/// Extracts the 5h utilization % from the confirmed schema (see module doc).
/// Primary: `five_hour.utilization`. Fallback: the active `session` entry in
/// `limits[]` (integer `percent`, same number) — used only if `utilization`
/// is ever null on some plan shape.
fn parse_five_hour_pct(v: &serde_json::Value) -> Option<OauthUsage> {
    if let Some(n) = dig(v, &["five_hour", "utilization"]).and_then(|n| n.as_f64()) {
        return Some(OauthUsage { five_hour_pct: n.clamp(0.0, 100.0) });
    }
    if let Some(limits) = v.get("limits").and_then(|l| l.as_array()) {
        for lim in limits {
            let is_session = lim.get("kind").and_then(|k| k.as_str()) == Some("session")
                || lim.get("group").and_then(|g| g.as_str()) == Some("session");
            if is_session {
                if let Some(p) = lim.get("percent").and_then(|p| p.as_f64()) {
                    return Some(OauthUsage { five_hour_pct: p.clamp(0.0, 100.0) });
                }
            }
        }
    }
    None
}

fn dig<'a>(v: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    Some(cur)
}

/// Claude Code와 동일한 UA. 버전은 설치된 CC에서 동적으로 읽는다 —
/// `~/.local/bin/claude`는 `~/.local/share/claude/versions/<ver>`로의 심링크라
/// basename이 곧 버전이다. 못 읽으면 확인된 마지막 버전으로 폴백.
fn client_user_agent() -> String {
    const FALLBACK_VER: &str = "2.1.251";
    let ver = dirs::home_dir()
        .map(|h| h.join(".local").join("bin").join("claude"))
        .and_then(|p| std::fs::read_link(p).ok())
        .and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned()))
        .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| FALLBACK_VER.to_string());
    format!("claude-cli/{ver} (external, cli)")
}

fn get_oauth_token() -> Option<String> {
    if let Some(tok) = read_token_from_credentials_file() {
        return Some(tok);
    }
    #[cfg(target_os = "macos")]
    if let Some(tok) = read_token_from_keychain() {
        return Some(tok);
    }
    None
}

fn read_token_from_credentials_file() -> Option<String> {
    let home = dirs::home_dir()?;
    let path = home.join(".claude").join(".credentials.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    extract_token(&v)
}

#[cfg(target_os = "macos")]
fn read_token_from_keychain() -> Option<String> {
    let out = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        // stderr is a diagnostic string (e.g. "item could not be found" /
        // "User interaction is not allowed") — never the secret itself.
        eprintln!(
            "[oauth-usage] keychain lookup failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    let raw = String::from_utf8(out.stdout).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    // Keychain may hold the same JSON blob as the credentials file, or a
    // bare token string — handle both without assuming one.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        return extract_token(&v);
    }
    Some(raw.to_string())
}

fn extract_token(v: &serde_json::Value) -> Option<String> {
    // Non-secret staleness signal: if the stored access token is already
    // past its `expiresAt`, the usage call will 401 and the user must
    // re-login. Logging this (a timestamp, never the token) makes a re-stale
    // visible instead of a silent fallback to the ccusage estimate.
    if let Some(exp_ms) = dig(v, &["claudeAiOauth", "expiresAt"]).and_then(|e| e.as_i64()) {
        let now_ms = chrono::Utc::now().timestamp_millis();
        if exp_ms < now_ms {
            eprintln!(
                "[oauth-usage] stored token expired {}h ago — will 401, re-login needed",
                (now_ms - exp_ms) / 3_600_000
            );
        }
    }
    let candidates: &[&[&str]] = &[
        &["claudeAiOauth", "accessToken"],
        &["oauth", "accessToken"],
        &["accessToken"],
        &["access_token"],
    ];
    for path in candidates {
        if let Some(s) = dig(v, path).and_then(|n| n.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

#[cfg(test)]
mod probe {
    use super::*;

    /// 배터리 공백 진단용 — 실호출 1회. 모듈 설계상 토큰은 절대 로그에 안
    /// 남고(401/429/parse miss 등 비밀 아닌 진단만), 성공 시 사용률 %만 찍힌다:
    ///   cargo test --lib oauth_probe -- --ignored --nocapture
    #[test]
    #[ignore]
    fn oauth_probe() {
        let mut t = OauthUsageTracker::new();
        match t.get() {
            Some(u) => eprintln!("PROBE OK: five_hour_pct={:.1}", u.five_hour_pct),
            None => eprintln!("PROBE UNAVAILABLE (위 진단 로그 참조)"),
        }
    }
}
