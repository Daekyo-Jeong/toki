//! §3.3.5 ① — live retry-streak tracker (앰비언트 1단계: 개입 0, 표정만).
//!
//! Fed by the watcher's JSONL tail (works with or without hooks installed),
//! this mirrors the §3.3.2 retry heuristic incrementally: a retry = the same
//! file re-edited with ≥1 Bash since its previous edit. A run of
//! `STREAK_ALERT` retries on one file flips a global alert the tray renders
//! (D_TRAY_ALERT); the alert decays after `ALERT_TTL` of quiet — no card, no
//! text, no notification. Timing is fully deterministic (코칭 헌법 확장).
//!
//! Living docs are excluded (§3.3 가드 승계) and catch-up scans replay old
//! history, so callers must only feed lines fresh enough to be "now"
//! (watcher checks the line timestamp before calling in).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Same-file retries needed to enter the alert state (spec 가설값 — 실기
/// 오탐 체감 후 조정, checklist §4).
const STREAK_ALERT: u32 = 3;
/// Alert decays this long after the last qualifying retry.
const ALERT_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Default)]
struct Sess {
    bash_seq: u32,
    last_edit_bash: HashMap<String, u32>,
    streak_file: String,
    streak: u32,
}

struct Alert {
    file: String,
    streak: u32,
    at: Instant,
}

#[derive(Default)]
struct Inner {
    sessions: HashMap<String, Sess>,
    alert: Option<Alert>,
}

pub struct ThrashWatch {
    inner: Mutex<Inner>,
}

static GLOBAL: OnceLock<ThrashWatch> = OnceLock::new();

impl ThrashWatch {
    pub fn global() -> &'static ThrashWatch {
        GLOBAL.get_or_init(|| ThrashWatch { inner: Mutex::new(Inner::default()) })
    }

    pub fn note_bash(&self, session: &str) {
        let mut g = self.inner.lock().unwrap();
        g.sessions.entry(session.to_string()).or_default().bash_seq += 1;
    }

    /// `file` = basename. Living-doc filtering is the caller's job (watcher
    /// reuses `retro::is_living_doc` so there is exactly one definition).
    pub fn note_edit(&self, session: &str, file: &str) {
        let mut g = self.inner.lock().unwrap();
        let s = g.sessions.entry(session.to_string()).or_default();
        let seq = s.bash_seq;
        let retry = s
            .last_edit_bash
            .get(file)
            .is_some_and(|prev| seq > *prev);
        s.last_edit_bash.insert(file.to_string(), seq);
        let mut hot = None;
        if retry {
            if s.streak_file == file {
                s.streak += 1;
            } else {
                s.streak_file = file.to_string();
                s.streak = 1;
            }
            if s.streak >= STREAK_ALERT {
                hot = Some(s.streak);
            }
        }
        if let Some(streak) = hot {
            g.alert = Some(Alert {
                file: file.to_string(),
                streak,
                at: Instant::now(),
            });
        }
        // A clean (non-retry) edit does NOT clear the streak: stacking edits
        // without running anything says nothing about the loop resolving.
        // Only the TTL (quiet) or a streak on another file moves the alert.
    }

    /// Current alert as (file, streak), TTL-checked. Expired alerts clear.
    pub fn current_alert(&self) -> Option<(String, u32)> {
        let mut g = self.inner.lock().unwrap();
        if let Some(a) = &g.alert {
            if a.at.elapsed() > ALERT_TTL {
                g.alert = None;
            }
        }
        g.alert.as_ref().map(|a| (a.file.clone(), a.streak))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> ThrashWatch {
        ThrashWatch { inner: Mutex::new(Inner::default()) }
    }

    #[test]
    fn streak_alert_after_three_retries() {
        let t = fresh();
        let s = "sess.jsonl";
        t.note_edit(s, "foo.rs"); // first edit — no retry
        for _ in 0..3 {
            t.note_bash(s);
            t.note_edit(s, "foo.rs"); // retry ×3
        }
        let (file, streak) = t.current_alert().expect("alert after 3 retries");
        assert_eq!(file, "foo.rs");
        assert_eq!(streak, 3);
    }

    #[test]
    fn no_alert_without_bash_between_edits() {
        let t = fresh();
        let s = "sess.jsonl";
        for _ in 0..5 {
            t.note_edit(s, "foo.rs"); // stacking edits, nothing ran
        }
        assert!(t.current_alert().is_none());
    }

    #[test]
    fn switching_files_resets_streak() {
        let t = fresh();
        let s = "sess.jsonl";
        t.note_edit(s, "a.rs");
        t.note_bash(s);
        t.note_edit(s, "a.rs"); // retry a ×1
        t.note_edit(s, "b.rs");
        t.note_bash(s);
        t.note_edit(s, "b.rs"); // retry b ×1 — streak restarts on b
        t.note_bash(s);
        t.note_edit(s, "b.rs"); // retry b ×2
        assert!(t.current_alert().is_none(), "no file reached 3 retries");
    }
}
