use crate::db::Db;
use crate::usage_tracker::{Source, UsageTracker};
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use toki_core::game::{self, CharacterSave, GameInputs};

pub type SharedSnapshot = Arc<std::sync::Mutex<Option<crate::usage_tracker::Snapshot>>>;

pub fn spawn(app: AppHandle, db: Arc<Db>, shared_snapshot: SharedSnapshot) {
    std::thread::spawn(move || {
        let mut last_tray_key = String::new();
        let mut tracker = UsageTracker::new(db.clone());
        let mut last_logged_source: Option<Source> = None;
        loop {
            if let Err(e) = run_tick(
                &app,
                &db,
                &mut last_tray_key,
                &mut tracker,
                &mut last_logged_source,
                &shared_snapshot,
            ) {
                eprintln!("[tick] error: {}", e);
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

fn run_tick(
    app: &AppHandle,
    db: &Db,
    last_tray_key: &mut String,
    tracker: &mut UsageTracker,
    last_logged_source: &mut Option<Source>,
    shared_snapshot: &SharedSnapshot,
) -> anyhow::Result<()> {
    // Drain new hook events into dungeon rows + record battle attribution.
    // No HP side effects — HP is now derived from the 5h usage window.
    drain_hooks(app, db)?;

    let totals = db.totals()?;
    let settings = db.load_settings()?;
    let now = Utc::now();
    let now_rfc = now.to_rfc3339();

    // Effective XP after baseline (track_since_install)
    let baseline = if settings.track_since_install {
        settings.install_baseline_cache_read
    } else {
        0
    };
    // v5 M1: XP 화폐가 cache_read → cache_read+cache_creation+output로 바뀌었다
    // (spec §6.1). baseline 컬럼 이름은 `install_baseline_cache_read` 그대로지만
    // 담는 값은 이제 같은 산식의 스냅샷이다 — 아래 두 기록 지점도 함께 옮겼다.
    let effective_xp = (totals.xp() - baseline).max(0);

    // Token deltas since last tick (cache_creation → gold, output → SP)
    let prev_cursors = db.load_cursors()?;
    let gold_delta = (totals.cache_creation - prev_cursors.cache_creation).max(0);
    let sp_delta = (totals.output - prev_cursors.output).max(0);

    // Load or birth save
    let prev = match db.load_save()? {
        Some(s) => s,
        None => {
            let init = CharacterSave::new_at_birth(&now_rfc);
            db.save_save(&init)?;
            update_tray_icon(app, &init, last_tray_key);
            let _ = app.emit("character-save", &init);
            // also persist cursors so deltas start fresh
            db.save_cursors(&crate::db::TokenCursors {
                cache_read: totals.cache_read,
                cache_creation: totals.cache_creation,
                output: totals.output,
            })?;
            return Ok(());
        }
    };

    let elapsed_secs = chrono::DateTime::parse_from_rfc3339(&prev.last_save)
        .map(|d| (now - d.with_timezone(&Utc)).num_seconds())
        .unwrap_or(0)
        .max(0);

    let usage = tracker.get();
    // Log source on first observation + any subsequent transition.
    if Some(usage.source) != *last_logged_source {
        println!("[usage] source={:?} tokens={}", usage.source, usage.active_block_tokens);
        *last_logged_source = Some(usage.source);
    }
    // Publish to the shared snapshot so UI commands can read it without
    // re-running the subprocess.
    if let Ok(mut guard) = shared_snapshot.lock() {
        *guard = Some(usage.clone());
    }

    let input = GameInputs {
        now_rfc3339: now_rfc.clone(),
        elapsed_secs,
        effective_xp,
        gold_delta,
        sp_delta,
        active_block_tokens: usage.active_block_tokens,
        token_limit: usage.token_limit,
    };
    let result = game::tick(&prev, &input);
    db.save_save(&result.state)?;

    // HP-driven dungeon abandonment removed — the 5h window's sliding
    // behavior made HP→0 a noisy signal that didn't match user activity.
    // Dungeons now only close via SessionEnd hooks (or future explicit
    // user action). game::tick still computes hp_zeroed; we ignore it.
    db.save_cursors(&crate::db::TokenCursors {
        cache_read: totals.cache_read,
        cache_creation: totals.cache_creation,
        output: totals.output,
    })?;

    if let Some(from_lv) = result.leveled_up_from {
        println!(
            "[level-up] {} → {} (xp={})",
            from_lv, result.state.lv, result.state.xp
        );
        let _ = app.emit(
            "character-leveled",
            serde_json::json!({"from": from_lv, "to": result.state.lv}),
        );
    }

    update_tray_icon(app, &result.state, last_tray_key);
    let _ = app.emit("character-save", &result.state);
    // 6b backwards-compat: emit legacy shape so untouched UI keeps working.
    let legacy = synthesize_legacy(&result.state);
    let _ = app.emit("character-state", &legacy);
    Ok(())
}

fn synthesize_legacy(save: &CharacterSave) -> serde_json::Value {
    let hp_pct = if save.hp_max > 0 {
        (save.hp as f32 / save.hp_max as f32 * 100.0) as i32
    } else {
        100
    };
    let stage = lv_to_sprite_stage(save.lv);
    serde_json::json!({
        "stage": stage,
        "hunger": (100 - hp_pct).clamp(0, 100),
        "happiness": save.agi_stat.clamp(0, 100),
        "intelligence": save.int_stat.clamp(0, 100),
        "stamina": save.str_stat.clamp(0, 100),
        "curiosity": save.luk_stat.clamp(0, 100),
        "total_xp": save.xp,
        "born_at": save.born_at,
        "last_interaction_at": save.last_save,
        "last_tick_at": save.last_save,
        "last_cache_read_seen": 0,
    })
}

/// Drain new rows from hook_events_raw and translate to dungeon state +
/// battle attribution rows. HP is no longer mutated here — it's derived
/// from the 5h usage window in `game::tick`.
///
///  - SessionStart → open dungeon
///  - SessionEnd   → close dungeon (cleared)
///  - Stop         → no-op (per-turn boundary, not session end)
///  - PostToolUse  → dungeon_events battle row (tool_use_id UNIQUE for replay)
fn drain_hooks(app: &AppHandle, db: &Db) -> anyhow::Result<()> {
    let after = db.load_last_drained_hook_id()?;
    let rows = db.drain_hook_events(after)?;
    if rows.is_empty() {
        return Ok(());
    }

    let mut max_id = after;

    for row in &rows {
        max_id = max_id.max(row.id);
        // Best-effort parse; ignore malformed payloads.
        let payload: serde_json::Value = match serde_json::from_str(&row.payload_json) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(sid) = payload.get("session_id").and_then(|v| v.as_str()) else {
            continue;
        };
        if sid.is_empty() {
            continue;
        }

        match row.event_type.as_str() {
            "SessionStart" => {
                let cwd = payload.get("cwd").and_then(|v| v.as_str());
                let inserted = db.upsert_dungeon_active(
                    sid,
                    "spontaneous",
                    cwd,
                    &row.received_at,
                )?;
                if inserted {
                    println!("[dungeon] open session={} cwd={:?}", sid, cwd);
                    let _ = app.emit("dungeon-opened", sid);
                }
            }
            // Stop fires on every assistant turn boundary, NOT at session end.
            // Treat as no-op — SessionEnd is the real session-terminal event.
            "Stop" => {}
            "SessionEnd" => {
                let closed = db.close_dungeon(sid, &row.received_at)?;
                if closed {
                    let reason = payload
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    println!("[dungeon] clear session={} reason={}", sid, reason);
                    let _ = app.emit("dungeon-closed", sid);
                }
            }
            "PostToolUse" => {
                let tool_name = payload
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unknown)");
                let tool_use_id = payload.get("tool_use_id").and_then(|v| v.as_str());

                // MVP-10: tools beyond the basic file/shell set are
                // learnable RPG skills. Basics (Bash/Edit/Read/Write/Glob/
                // Grep/LS) are platy "basic attacks" — kept out so the
                // skillbook stays curated. Everything else (mcp__*,
                // WebFetch, ToolSearch, Agent, Task*, etc.) counts.
                // Recorded independently of dungeon state.
                if crate::is_skill_tool(tool_name) {
                    match db.record_skill_use(tool_name, &row.received_at) {
                        Ok(true) => {
                            println!("[skill] unlocked {}", tool_name);
                            let _ = app.emit(
                                "skill-unlocked",
                                serde_json::json!({ "skill": tool_name }),
                            );
                        }
                        Ok(false) => {
                            let _ = app.emit(
                                "skill-used",
                                serde_json::json!({ "skill": tool_name }),
                            );
                        }
                        Err(e) => eprintln!("[skill] record error: {}", e),
                    }
                }

                // dungeon_events: ensure an active dungeon exists for this
                // session. Two failure modes we used to silently swallow:
                //   1) SessionStart hook was missed by Claude Code → no row
                //   2) HP previously hit 0 and the dungeon got abandoned
                // The user's mental model is "I used a tool → mob should
                // show up", so lazy-open / reactivate here. The UI relies
                // on `battle-resolved` emit at the end of this branch.
                let dungeon = match db.dungeon_by_id(sid)? {
                    Some(d) if d.status == "active" => d,
                    Some(d) => {
                        let _ = db.reactivate_dungeon(sid)?;
                        println!("[dungeon] reactivate session={} (was {})", sid, d.status);
                        let _ = app.emit("dungeon-opened", sid);
                        db.dungeon_by_id(sid)?.unwrap_or(d)
                    }
                    None => {
                        let cwd = payload.get("cwd").and_then(|v| v.as_str());
                        let _ = db.upsert_dungeon_active(
                            sid,
                            "lazy",
                            cwd,
                            &row.received_at,
                        )?;
                        println!("[dungeon] lazy-open session={} cwd={:?}", sid, cwd);
                        let _ = app.emit("dungeon-opened", sid);
                        match db.dungeon_by_id(sid)? {
                            Some(d) => d,
                            None => continue,
                        }
                    }
                };
                let resp = payload.get("tool_response");
                let duration_ms = resp
                    .and_then(|r| r.get("duration_ms"))
                    .and_then(|v| v.as_i64());
                let interrupted = resp
                    .and_then(|r| r.get("interrupted"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let event_payload = serde_json::json!({
                    "tool": tool_name,
                    "duration_ms": duration_ms,
                    "interrupted": interrupted,
                });
                let inserted = db.insert_dungeon_event(
                    &dungeon.id,
                    &row.received_at,
                    "battle",
                    &event_payload.to_string(),
                    tool_use_id,
                )?;
                if inserted {
                    let _ = app.emit(
                        "battle-resolved",
                        serde_json::json!({
                            "session_id": sid,
                            "tool": tool_name,
                            "duration_ms": duration_ms,
                            "interrupted": interrupted,
                        }),
                    );
                }
            }
            _ => {}
        }
    }

    db.save_last_drained_hook_id(max_id)?;
    Ok(())
}

fn lv_to_sprite_stage(lv: i32) -> &'static str {
    match lv {
        0..=4 => "egg",
        5..=14 => "baby",
        15..=29 => "teen",
        30..=59 => "adult",
        _ => "elder",
    }
}

fn update_tray_icon(app: &AppHandle, _save: &CharacterSave, last_key: &mut String) {
    // Default = fixed Toki brand glyph. §3.3.5 ①: while a live retry streak
    // is hot (same file re-edited ≥3× with runs in between, watcher-fed) the
    // tray swaps to D_TRAY_ALERT — 말 없이 상태로만 표현, 개입 0. The alert
    // decays in thrash_watch (TTL) and the tray follows on the next tick.
    let alert = crate::thrash_watch::ThrashWatch::global().current_alert();
    let key = if alert.is_some() { "alert" } else { "default" };
    if last_key == key {
        return;
    }
    let Some(tray) = app.tray_by_id("main-tray") else {
        return;
    };
    let res = match &alert {
        Some(_) => {
            let (buf, w, h) = crate::tray_sprite::alert_rgba();
            tray.set_icon(Some(tauri::image::Image::new_owned(buf, w, h)))
        }
        None => tray.set_icon(Some(crate::tray_icon())),
    };
    if let Err(e) = res {
        eprintln!("[tray] set_icon error: {}", e);
        return;
    }
    let _ = tray.set_icon_as_template(true);
    // Frontend can mirror the pet's expression off the same state; payload is
    // (file, streak) or null when the alert clears.
    let _ = app.emit("thrash-alert", &alert);
    *last_key = key.into();
}
