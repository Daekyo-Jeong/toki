//! Game state model + pure update functions.
//!
//! Inputs come from the IO layer (token totals, elapsed time, etc.).
//! Outputs are deterministic given the same inputs.

use serde::{Deserialize, Serialize};

/* ─── Constants (placeholders, calibrate in Phase 1) ────────────── */

/// Hard cap on Lv. Anything past this is "maxed" until rebalanced.
pub const LV_CAP: i32 = 999;

/// XP unit = `cache_read + cache_creation + output` tokens (spec §6.1).
/// `input_tokens`는 뺀다 — Claude에선 streaming placeholder라 못 쓴다.
/// v4까진 cache_read 하나였지만 그게 총합의 95.9%라 divisor는 그대로 둔다
/// (실측 116,368 이벤트 기준 Lv 68 → 69, 리셋 아님). Big numbers OK in i64.
/// Formula: Lv = floor(sqrt(xp / 5_000_000)) + 1, capped at LV_CAP.
/// √ curve is intentional — mastery doesn't rise linearly, so each level
/// costs progressively more. Divisor sets the pace (5M ≈ one heavy session
/// per early level); raising it stretches the whole curve.
///   5M XP   → Lv 2
///   500M    → Lv 11
///   50B     → Lv 101
///   ~5T     → Lv 999 (cap)
const XP_LV_DIVISOR: f64 = 5_000_000.0;

/* ─── Types ──────────────────────────────────────────────────────── */

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterSave {
    pub lv: i32,
    pub xp: i64,
    pub hp: i32,
    pub hp_max: i32,
    pub gold: i64,
    pub sp: i64,

    // 5 stats (0~100, placeholder all 10 for now)
    pub int_stat: i32,
    pub str_stat: i32,
    pub agi_stat: i32,
    pub wis_stat: i32,
    pub luk_stat: i32,

    pub class: String, // "apprentice" | "coder" | "frontend" | "backend" | ...
    pub born_at: String,
    pub last_save: String,
}

impl CharacterSave {
    pub fn new_at_birth(now_rfc3339: &str) -> Self {
        Self {
            lv: 1,
            xp: 0,
            hp: HP_MAX_FIXED,
            hp_max: HP_MAX_FIXED,
            gold: 0,
            sp: 0,
            int_stat: 10,
            str_stat: 10,
            agi_stat: 10,
            wis_stat: 10,
            luk_stat: 10,
            class: "apprentice".to_string(),
            born_at: now_rfc3339.to_string(),
            last_save: now_rfc3339.to_string(),
        }
    }
}

/// Input for one game tick.
#[derive(Debug, Clone)]
pub struct GameInputs {
    pub now_rfc3339: String,
    pub elapsed_secs: i64,
    pub effective_xp: i64,        // cache_read after baseline
    pub gold_delta: i64,          // cache_creation increase since last tick
    pub sp_delta: i64,            // output_tokens increase since last tick

    /// Total tokens used so far in the current Anthropic 5-hour block.
    /// HP is derived directly from this — monotonic within a block, resets
    /// to 0 when a new block starts.
    pub active_block_tokens: i64,

    /// Effective 5h block ceiling — usage at this value ⇒ HP 0%.
    /// Sourced from ccusage's "max tokens from previous sessions" (i.e.
    /// the user's historical peak), which self-calibrates over time.
    /// 0 means unknown → HP stays full.
    pub token_limit: i64,
}

/// HP is a fixed 0~100 percentage. The user's historical peak 5h block
/// usage is treated as their personal ceiling — burning that much in a
/// block reduces HP to 0.
pub const HP_MAX_FIXED: i32 = 100;

/// Result of one tick.
#[derive(Debug, Clone)]
pub struct TickResult {
    pub state: CharacterSave,
    pub leveled_up_from: Option<i32>,
    /// True if HP transitioned from > 0 to 0 in this tick (battle defeat).
    /// IO layer should mark the active dungeon as `abandoned` and restore HP.
    pub hp_zeroed: bool,
}

/* ─── Pure functions ─────────────────────────────────────────────── */

pub fn xp_to_level(xp: i64) -> i32 {
    if xp <= 0 {
        return 1;
    }
    let v = ((xp as f64) / XP_LV_DIVISOR).sqrt() as i32 + 1;
    v.clamp(1, LV_CAP)
}

pub fn xp_for_level(lv: i32) -> i64 {
    let lv = lv.max(1) as f64;
    ((lv - 1.0).powi(2) * XP_LV_DIVISOR) as i64
}

/// HP_max no longer scales with level — it's a fixed 0~100 percentage now.
/// Kept as a function for backward compatibility with `tick()`.
pub fn hp_max_for(_lv: i32) -> i32 {
    HP_MAX_FIXED
}

/// Progress toward next level (0.0 ~ 1.0). Returns 1.0 at LV_CAP.
pub fn level_progress(xp: i64, lv: i32) -> f64 {
    if lv >= LV_CAP {
        return 1.0;
    }
    let here = xp_for_level(lv) as f64;
    let next = xp_for_level(lv + 1) as f64;
    let span = (next - here).max(1.0);
    let into = (xp as f64 - here).max(0.0);
    (into / span).clamp(0.0, 1.0)
}

/// Derive HP percentage (0~100) from current block usage vs the user's
/// historical peak ceiling. Monotonic within a block — the user only
/// loses HP as they spend tokens, never on speculative burn-rate spikes.
/// token_limit == 0 ⇒ unknown ceiling ⇒ HP stays full.
pub fn hp_for_block_usage(active_block_tokens: i64, token_limit: i64) -> i32 {
    if token_limit <= 0 {
        return HP_MAX_FIXED;
    }
    let pct = 100 - ((active_block_tokens.max(0) as f64 / token_limit as f64) * 100.0) as i32;
    pct.clamp(0, HP_MAX_FIXED)
}

/// One game tick. Pure: same inputs → same output.
pub fn tick(prev: &CharacterSave, input: &GameInputs) -> TickResult {
    let mut next = prev.clone();

    next.xp = input.effective_xp.max(0);
    next.gold = next.gold.saturating_add(input.gold_delta.max(0));
    next.sp = next.sp.saturating_add(input.sp_delta.max(0));

    // lv always tracks the honest xp→level mapping. xp is monotonic
    // (cache_read only grows), so this only ever moves lv *down* when the
    // curve itself changes (e.g. XP_LV_DIVISOR retune) — self-healing
    // recalibration, no one-off migration needed. Up-moves still emit a
    // level-up event; down-moves (recalibration) are silent.
    let new_lv = xp_to_level(next.xp);
    let leveled = if new_lv > prev.lv {
        Some(prev.lv)
    } else {
        None
    };
    next.lv = new_lv;
    // HP_max stays fixed at 100% — leveling earns stats/class/skills, not
    // a larger HP pool. Personal token ceiling does the HP scaling.
    next.hp_max = HP_MAX_FIXED;

    // HP = current block usage vs personal ceiling. Monotonic decrease
    // within a block, resets to 100 when a new block starts (tokens = 0).
    next.hp = hp_for_block_usage(input.active_block_tokens, input.token_limit);
    let hp_zeroed = prev.hp > 0 && next.hp == 0;

    next.last_save = input.now_rfc3339.clone();

    TickResult {
        state: next,
        leveled_up_from: leveled,
        hp_zeroed,
    }
}

/* ─── Tests ──────────────────────────────────────────────────────── */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lv_growth() {
        assert_eq!(xp_to_level(0), 1);
        assert_eq!(xp_to_level(100), 1);
        assert_eq!(xp_to_level(5_000_000), 2);
        assert_eq!(xp_to_level(20_000_000), 3);
        assert_eq!(xp_to_level(500_000_000), 11);
        assert_eq!(xp_to_level(50_000_000_000), 101);
        // Cap kicks in around (LV_CAP-1)² × 5M ≈ 4.98T.
        assert_eq!(xp_to_level(5_000_000_000_000), LV_CAP);
    }

    #[test]
    fn hp_max_is_fixed_at_100() {
        assert_eq!(hp_max_for(1), 100);
        assert_eq!(hp_max_for(10), 100);
        assert_eq!(hp_max_for(99), 100);
    }

    fn base_input() -> GameInputs {
        GameInputs {
            now_rfc3339: "2026-05-18T00:00:00Z".into(),
            elapsed_secs: 0,
            effective_xp: 0,
            gold_delta: 0,
            sp_delta: 0,
            active_block_tokens: 0,
            token_limit: 200_000_000, // 200M personal ceiling
        }
    }

    #[test]
    fn empty_block_gives_full_hp() {
        let s = CharacterSave::new_at_birth("2026-05-18T00:00:00Z");
        let mut input = base_input();
        input.effective_xp = 5_000_000; // Lv 2 under ÷5M; HP unaffected by lv
        let r = tick(&s, &input);
        assert_eq!(r.state.lv, 2);
        assert_eq!(r.leveled_up_from, Some(1));
        assert_eq!(r.state.hp, 100);
        assert!(!r.hp_zeroed);
    }

    #[test]
    fn divisor_retune_recalibrates_lv_down_silently() {
        // A save left at a stale-high lv (earned under the old ÷1M curve)
        // recalibrates *down* to the honest ÷5M level on the next tick, with
        // no spurious level-up event. 12.16B xp → Lv 50 under ÷5M.
        let mut s = CharacterSave::new_at_birth("t");
        s.lv = 111; // stale, as if carried over from the old curve
        let mut input = base_input();
        input.effective_xp = 12_162_198_604;
        let r = tick(&s, &input);
        assert_eq!(r.state.lv, 50);
        assert_eq!(r.leveled_up_from, None); // down-move is silent
    }

    /// v5 M1 회귀 가드 — XP 화폐를 cache_read에서 합산으로 바꿔도 divisor를
    /// 안 건드린 근거. 2026-09-01 실측 DB(117,045 이벤트)의 실제 값이다.
    ///
    /// 화폐 정의나 divisor를 손대면 여기가 먼저 깨진다. 깨지면 "리셋이 아니라
    /// +1"이라는 전제가 무너진 것이니, 고치기 전에 spec §6.1부터 다시 본다.
    #[test]
    fn v5_currency_shifts_level_by_one_not_a_reset() {
        // 실측: cache_read 22.70B · cache_creation 0.88B · output 0.08B
        const MEASURED_CACHE_READ: i64 = 22_704_209_140;
        const MEASURED_XP_TOTAL: i64 = 23_792_247_607;

        let old_lv = xp_to_level(MEASURED_CACHE_READ); // v4 화폐
        let new_lv = xp_to_level(MEASURED_XP_TOTAL); // v5 화폐
        assert_eq!(old_lv, 68, "v4 기준 레벨이 바뀌었다면 divisor가 흔들린 것");
        assert_eq!(new_lv, 69, "화폐 교체는 +1이어야 한다 — 리셋이면 계약 위반");

        // cache_read가 총합의 대부분이라는 게 divisor를 유지하는 근거다.
        let share = MEASURED_CACHE_READ as f64 / MEASURED_XP_TOTAL as f64;
        assert!(share > 0.94, "cache_read 비중이 {share:.3}으로 떨어졌다 — 재보정 필요");
    }

    #[test]
    fn half_block_gives_half_hp() {
        let s = CharacterSave::new_at_birth("t");
        let mut input = base_input();
        input.active_block_tokens = 100_000_000; // 50% of 200M ceiling
        let r = tick(&s, &input);
        assert_eq!(r.state.hp, 50);
        assert!(!r.hp_zeroed);
    }

    #[test]
    fn at_ceiling_hp_zero() {
        let s = CharacterSave::new_at_birth("t");
        let mut input = base_input();
        input.active_block_tokens = 200_000_000;
        let r = tick(&s, &input);
        assert_eq!(r.state.hp, 0);
        assert!(r.hp_zeroed);
    }

    #[test]
    fn over_ceiling_clamps_to_zero() {
        let s = CharacterSave::new_at_birth("t");
        let mut input = base_input();
        input.active_block_tokens = 999_999_999_999;
        let r = tick(&s, &input);
        assert_eq!(r.state.hp, 0);
    }

    #[test]
    fn unknown_ceiling_keeps_full_hp() {
        let s = CharacterSave::new_at_birth("t");
        let mut input = base_input();
        input.active_block_tokens = 50_000_000;
        input.token_limit = 0; // ccusage history empty
        let r = tick(&s, &input);
        assert_eq!(r.state.hp, 100); // don't punish unknown
    }

    #[test]
    fn level_does_not_affect_hp() {
        // Same usage, different levels → same HP. HP is pure percentage now.
        let mut input = base_input();
        input.active_block_tokens = 100_000_000;

        let s1 = CharacterSave::new_at_birth("t");
        assert_eq!(tick(&s1, &input).state.hp, 50);

        let mut s50 = CharacterSave::new_at_birth("t");
        s50.lv = 50;
        assert_eq!(tick(&s50, &input).state.hp, 50);
    }

    #[test]
    fn hp_zeroed_only_on_transition() {
        let mut s = CharacterSave::new_at_birth("t");
        s.hp = 0;
        let mut input = base_input();
        input.active_block_tokens = 999_999_999_999;
        assert!(!tick(&s, &input).hp_zeroed);
    }

    #[test]
    fn hp_recovers_when_block_resets() {
        let mut s = CharacterSave::new_at_birth("t");
        s.hp = 10;
        let mut input = base_input();
        input.active_block_tokens = 20_000_000; // 10% of 200M ceiling
        let r = tick(&s, &input);
        assert_eq!(r.state.hp, 90);
    }

    #[test]
    fn hp_ignores_burn_rate_spikes() {
        // Burn rate could be wild during a momentary spike, but HP only
        // reflects what's actually been spent.
        let s = CharacterSave::new_at_birth("t");
        let mut input = base_input();
        input.active_block_tokens = 40_000_000; // 20% of 200M, spent
        let r = tick(&s, &input);
        assert_eq!(r.state.hp, 80);
    }

    #[test]
    fn level_progress_bounds() {
        assert!((level_progress(0, 1) - 0.0).abs() < 1e-9);
        assert!((level_progress(xp_for_level(2), 1) - 1.0).abs() < 1e-9);
        // At Lv cap, always 1.0
        assert_eq!(level_progress(0, LV_CAP), 1.0);
    }
}
