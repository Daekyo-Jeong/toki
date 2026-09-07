use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

use toki_core::game::CharacterSave;


pub struct Db {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone)]
pub struct EventRow<'a> {
    pub uuid: &'a str,
    /// 어느 에이전트에서 왔나 — `"claude"` | `"codex"`. 기존 행은 마이그레이션
    /// 기본값으로 전부 `claude`가 된다. spec §5.1.
    pub agent: &'a str,
    pub session_id: &'a str,
    pub timestamp: &'a str,
    pub model: Option<&'a str>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub project_path: Option<&'a str>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS processed_events (
                id INTEGER PRIMARY KEY,
                uuid TEXT UNIQUE NOT NULL,
                session_id TEXT,
                timestamp TEXT,
                model TEXT,
                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0,
                cache_creation_tokens INTEGER DEFAULT 0,
                cache_read_tokens INTEGER DEFAULT 0,
                project_path TEXT,
                processed_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS file_offsets (
                file_path TEXT PRIMARY KEY,
                last_offset INTEGER NOT NULL,
                last_modified TEXT
            );

            CREATE TABLE IF NOT EXISTS app_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                notifications_level TEXT NOT NULL DEFAULT 'impt',
                autostart_enabled INTEGER NOT NULL DEFAULT 0,
                track_since_install INTEGER NOT NULL DEFAULT 0,
                install_baseline_cache_read INTEGER NOT NULL DEFAULT 0
            );
            INSERT OR IGNORE INTO app_settings (id) VALUES (1);

            CREATE TABLE IF NOT EXISTS diary_entries (
                id INTEGER PRIMARY KEY,
                event_type TEXT NOT NULL,
                description TEXT NOT NULL,
                model TEXT,
                project_path TEXT,
                occurred_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_diary_occurred ON diary_entries(occurred_at DESC);

            CREATE TABLE IF NOT EXISTS character_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                stage TEXT NOT NULL,
                hunger INTEGER NOT NULL,
                happiness INTEGER NOT NULL,
                intelligence INTEGER NOT NULL,
                stamina INTEGER NOT NULL,
                curiosity INTEGER NOT NULL,
                total_xp INTEGER NOT NULL,
                born_at TEXT NOT NULL,
                last_interaction_at TEXT NOT NULL,
                last_tick_at TEXT NOT NULL,
                last_cache_read_seen INTEGER NOT NULL DEFAULT 0
            );

            -- v2 tables (MVP-6b)
            CREATE TABLE IF NOT EXISTS character_save (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                lv INTEGER NOT NULL,
                xp INTEGER NOT NULL,
                hp INTEGER NOT NULL,
                hp_max INTEGER NOT NULL,
                gold INTEGER NOT NULL DEFAULT 0,
                sp INTEGER NOT NULL DEFAULT 0,
                int_stat INTEGER NOT NULL DEFAULT 10,
                str_stat INTEGER NOT NULL DEFAULT 10,
                agi_stat INTEGER NOT NULL DEFAULT 10,
                wis_stat INTEGER NOT NULL DEFAULT 10,
                luk_stat INTEGER NOT NULL DEFAULT 10,
                class TEXT NOT NULL DEFAULT 'apprentice',
                born_at TEXT NOT NULL,
                last_save TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS skills (
                id INTEGER PRIMARY KEY,
                skill_name TEXT UNIQUE NOT NULL,
                unlocked_at TEXT NOT NULL,
                use_count INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS dungeons (
                id TEXT PRIMARY KEY,
                source TEXT NOT NULL,        -- 'spontaneous' | 'planned'
                project_path TEXT,
                status TEXT NOT NULL,        -- 'active' | 'cleared' | 'abandoned'
                started_at TEXT NOT NULL,
                ended_at TEXT,
                plan_json TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_dungeons_status ON dungeons(status);

            CREATE TABLE IF NOT EXISTS dungeon_events (
                id INTEGER PRIMARY KEY,
                dungeon_id TEXT NOT NULL,
                occurred_at TEXT NOT NULL,
                event_type TEXT NOT NULL,
                payload_json TEXT,
                tool_use_id TEXT UNIQUE,
                FOREIGN KEY (dungeon_id) REFERENCES dungeons(id)
            );
            CREATE INDEX IF NOT EXISTS idx_devents_dungeon ON dungeon_events(dungeon_id, occurred_at);

            -- MVP-7: raw hook events (pre-game-logic). Drained/processed in MVP-8.
            CREATE TABLE IF NOT EXISTS hook_events_raw (
                id INTEGER PRIMARY KEY,
                received_at TEXT NOT NULL,
                event_type TEXT NOT NULL,
                payload_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_hook_raw_received ON hook_events_raw(received_at DESC);
            "#,
        )?;

        // v2 migrations on app_settings — idempotent ALTER.
        for sql in [
            "ALTER TABLE app_settings ADD COLUMN last_cache_read_seen INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE app_settings ADD COLUMN last_cache_creation_seen INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE app_settings ADD COLUMN last_output_seen INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE app_settings ADD COLUMN migration_dialog_shown INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE app_settings ADD COLUMN last_drained_hook_id INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE app_settings ADD COLUMN last_hook_at TEXT",
            "ALTER TABLE dungeon_events ADD COLUMN tool_use_id TEXT",
            // MVP-12 pivot: retrospective summary for completed dungeons.
            // Generated on demand via `claude -p` from hook_events_raw data.
            "ALTER TABLE dungeons ADD COLUMN retrospective TEXT",
            "ALTER TABLE dungeons ADD COLUMN retrospective_at TEXT",
            // Pin-mode: when on, the popover stays at (pinned_x, pinned_y)
            // and tray click only toggles visibility (no reposition).
            "ALTER TABLE app_settings ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE app_settings ADD COLUMN pinned_x INTEGER",
            "ALTER TABLE app_settings ADD COLUMN pinned_y INTEGER",
            // v4: Cassette Futurism appearance settings (V4-3/V4-7).
            "ALTER TABLE app_settings ADD COLUMN active_character TEXT NOT NULL DEFAULT 'dong'",
            // v5 M6: 기본 펫을 깡총이로. 앱 이름이 Toki인데 토끼가 기본이 아니었다.
            // 컬럼 기본값은 옛 설치를 위해 그대로 두고(위 ALTER는 되돌릴 수 없다),
            // 아래 UPDATE가 **아직 아무도 안 고른 값**만 한 번 올린다.
            "ALTER TABLE app_settings ADD COLUMN pet_default_v5 INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE app_settings ADD COLUMN case_color TEXT NOT NULL DEFAULT 'beige'",
            "ALTER TABLE app_settings ADD COLUMN invert_screen INTEGER NOT NULL DEFAULT 0",
            // V4-6: coaching LLM backend. Default = local Ollama (zero quota).
            "ALTER TABLE app_settings ADD COLUMN coach_backend TEXT NOT NULL DEFAULT 'ollama'",
            "ALTER TABLE app_settings ADD COLUMN coach_ollama_model TEXT NOT NULL DEFAULT 'exaone3.5:7.8b'",
            // V4-10: window always-on-top, user-toggleable (tauri.conf.json's
            // hardcoded `alwaysOnTop: true` is just the pre-boot state now).
            "ALTER TABLE app_settings ADD COLUMN always_on_top INTEGER NOT NULL DEFAULT 1",
            // v5 M1: 이벤트가 어느 에이전트에서 왔나. 기존 행은 전부 Claude다
            // (v5 전엔 Claude만 읽었다). spec §5.1.
            "ALTER TABLE processed_events ADD COLUMN agent TEXT NOT NULL DEFAULT 'claude'",
            // v5 M3: "최근 활동 에이전트"를 10초마다 묻는다 — timestamp 인덱스가
            // 없으면 12만 행 풀스캔이다.
            "CREATE INDEX IF NOT EXISTS idx_processed_events_ts ON processed_events(timestamp)",
            // v5 M5: 코칭 기본 백엔드가 Ollama → 에이전트 CLI(`auto`)로 바뀌었다
            // (spec §4.1). 기존 행의 'ollama'는 사용자가 고른 게 아니라 옛 컬럼
            // 기본값이라 한 번만 'auto'로 올린다. 아래 UPDATE의 게이트가 이 플래그.
            "ALTER TABLE app_settings ADD COLUMN coach_backend_v5 INTEGER NOT NULL DEFAULT 0",
            // v5 M6: 온보딩 시퀀스를 끝냈나. 기존 설치는 아래에서 1로 올린다.
            "ALTER TABLE app_settings ADD COLUMN onboarded INTEGER NOT NULL DEFAULT 0",
            // 그 승격을 **평생 한 번만** 돌리는 게이트. 매 기동마다 돌면
            // `onboarded`를 0으로 되돌려도 다음 기동에 즉시 1로 덮여, 사용자가
            // 온보딩을 다시 볼 방법이 영영 없어진다(검수도 못 한다).
            "ALTER TABLE app_settings ADD COLUMN onboard_migrated INTEGER NOT NULL DEFAULT 0",
        ] {
            let _ = conn.execute(sql, []);
        }
        // 위 플래그가 0인 설치에서만 한 번. 이후 사용자가 다시 Ollama를 고르면
        // 플래그가 이미 1이라 그 선택을 덮지 않는다.
        let _ = conn.execute(
            "UPDATE app_settings SET coach_backend = 'auto'
             WHERE coach_backend_v5 = 0 AND coach_backend = 'ollama'",
            [],
        );
        let _ = conn.execute("UPDATE app_settings SET coach_backend_v5 = 1", []);
        // v5 M6: 이미 이벤트가 쌓인 설치는 신규 사용자가 아니다 — 온보딩을 건너뛴다.
        // 게이트 덕에 이 승격은 설치당 한 번뿐이라, 이후 `onboarded=0`으로
        // 되돌리면 온보딩을 다시 볼 수 있다.
        let _ = conn.execute(
            "UPDATE app_settings SET onboarded = 1
             WHERE onboard_migrated = 0 AND EXISTS (SELECT 1 FROM processed_events LIMIT 1)",
            [],
        );
        let _ = conn.execute("UPDATE app_settings SET onboard_migrated = 1", []);
        let _ = conn.execute(
            "UPDATE app_settings SET active_character = 'bunny'
             WHERE pet_default_v5 = 0 AND active_character = 'dong'",
            [],
        );
        let _ = conn.execute("UPDATE app_settings SET pet_default_v5 = 1", []);
        // Unique index for replay-safe dedup. Can fail if there are pre-existing
        // duplicates — drop them first (none should exist in practice since
        // tool_use_id was just added).
        let _ = conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_devents_tool_use_id
             ON dungeon_events(tool_use_id) WHERE tool_use_id IS NOT NULL",
            [],
        );
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn get_offset(&self, file_path: &str) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let row: Option<i64> = conn
            .query_row(
                "SELECT last_offset FROM file_offsets WHERE file_path = ?1",
                params![file_path],
                |r| r.get(0),
            )
            .ok();
        Ok(row.unwrap_or(0) as u64)
    }

    pub fn set_offset(&self, file_path: &str, offset: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO file_offsets (file_path, last_offset, last_modified)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(file_path) DO UPDATE SET
               last_offset = excluded.last_offset,
               last_modified = excluded.last_modified",
            params![file_path, offset as i64],
        )?;
        Ok(())
    }

    /// Returns true if inserted, false if duplicate.
    pub fn insert_event(&self, e: &EventRow<'_>) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "INSERT OR IGNORE INTO processed_events
             (uuid, agent, session_id, timestamp, model,
              input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, project_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                e.uuid,
                e.agent,
                e.session_id,
                e.timestamp,
                e.model,
                e.input_tokens,
                e.output_tokens,
                e.cache_creation_tokens,
                e.cache_read_tokens,
                e.project_path,
            ],
        )?;
        Ok(changed > 0)
    }

    /// M2: 에이전트별 누적 소비 — XP 화폐(cache_read+cache_creation+output,
    /// spec §6.1) 기준. XP 자체는 한 펫에 합산되고 이건 표시용 분해다(정보
    /// 화면 "에이전트" 페이지).
    pub fn xp_by_agent(&self) -> Result<Vec<TokenBucket>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT agent,
                    COALESCE(SUM(cache_read_tokens + cache_creation_tokens + output_tokens), 0) AS t
             FROM processed_events GROUP BY agent ORDER BY t DESC",
        )?;
        let v = stmt
            .query_map([], |r| {
                Ok(TokenBucket { label: r.get::<_, String>(0)?, tokens: r.get::<_, i64>(1)? })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    pub fn totals(&self) -> Result<Totals> {
        let conn = self.conn.lock().unwrap();
        let t = conn.query_row(
            "SELECT
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(cache_creation_tokens), 0),
                COALESCE(SUM(cache_read_tokens), 0),
                COUNT(*)
             FROM processed_events",
            [],
            |r| {
                Ok(Totals {
                    input: r.get(0)?,
                    output: r.get(1)?,
                    cache_creation: r.get(2)?,
                    cache_read: r.get(3)?,
                    events: r.get(4)?,
                })
            },
        )?;
        Ok(t)
    }

    /// M3: 가장 최근 이벤트의 에이전트 — 배터리가 어느 창을 따를지 정한다
    /// (spec §5.3). `ORDER BY id`가 아니라 **timestamp**다: 초기 스캔은 파일
    /// 순서라 id가 시간 순서와 다르다(M2 설치 때 옛 codex 행이 최신 id를 받았다).
    pub fn last_active_agent(&self) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT agent FROM processed_events ORDER BY timestamp DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn usage_summary(&self) -> Result<UsageSummary> {
        let conn = self.conn.lock().unwrap();

        let q_total = |where_clause: &str| -> Result<i64> {
            let sql = format!(
                "SELECT COALESCE(SUM(cache_read_tokens), 0) FROM processed_events {}",
                where_clause
            );
            Ok(conn.query_row(&sql, [], |r| r.get(0))?)
        };
        let last_5h = q_total("WHERE datetime(timestamp) >= datetime('now', '-5 hours')")?;
        let today = q_total("WHERE datetime(timestamp) >= datetime('now', 'start of day')")?;
        let last_7d = q_total("WHERE datetime(timestamp) >= datetime('now', '-7 days')")?;

        let mut stmt = conn.prepare(
            "SELECT COALESCE(model, '(unknown)') AS m, SUM(cache_read_tokens) AS t
             FROM processed_events
             WHERE datetime(timestamp) >= datetime('now', '-7 days')
               AND model IS NOT NULL
             GROUP BY m
             ORDER BY t DESC",
        )?;
        let by_model = stmt
            .query_map([], |r| {
                Ok(TokenBucket {
                    label: r.get::<_, String>(0)?,
                    tokens: r.get::<_, i64>(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut stmt = conn.prepare(
            "SELECT COALESCE(project_path, '(none)') AS p, SUM(cache_read_tokens) AS t
             FROM processed_events
             WHERE datetime(timestamp) >= datetime('now', '-7 days')
               AND project_path IS NOT NULL AND project_path != ''
             GROUP BY p
             ORDER BY t DESC
             LIMIT 5",
        )?;
        let by_project = stmt
            .query_map([], |r| {
                Ok(TokenBucket {
                    label: r.get::<_, String>(0)?,
                    tokens: r.get::<_, i64>(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(UsageSummary {
            last_5h,
            today,
            last_7d,
            by_model,
            by_project,
        })
    }

    /// Lightweight: sum of cache_read tokens over the rolling 5h window.
    /// Called every tick — keep it cheap.
    pub fn last_5h_cache_read(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let v: i64 = conn.query_row(
            "SELECT COALESCE(SUM(cache_read_tokens), 0)
             FROM processed_events
             WHERE datetime(timestamp) >= datetime('now', '-5 hours')",
            [],
            |r| r.get(0),
        )?;
        Ok(v)
    }

    /// Fallback for active-block tokens when ccusage isn't available.
    /// Sums ALL token columns (input + output + cache_creation + cache_read)
    /// over the rolling 5h window. Not block-accurate but close enough to
    /// keep HP moving when offline.
    pub fn last_5h_total_tokens(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let v: i64 = conn.query_row(
            "SELECT COALESCE(SUM(input_tokens + output_tokens
                                 + cache_creation_tokens + cache_read_tokens), 0)
             FROM processed_events
             WHERE agent = 'claude'
               AND datetime(timestamp) >= datetime('now', '-5 hours')",
            [],
            |r| r.get(0),
        )?;
        Ok(v)
    }

    /// (현재 5h 블록 토큰, 개인 최대 5h 블록 토큰). ccusage의
    /// `--token-limit max`와 같은 의미의 천장을 **우리 DB만으로** 계산한다.
    ///
    /// 2026-08-31: 배터리가 빈칸으로 남는 사고의 근본 원인 — 폴백 사슬의
    /// 마지막 단(sqlite)이 `token_limit=0`을 내보내서 UI가 null 처리했다.
    /// 즉 ccusage 미설치 + oauth 429면 표시할 값이 아예 없었다. 블록은 5h
    /// (18000s) 고정 버킷으로 나눈다 — ccusage는 첫 메시지 시각에 정렬하지만,
    /// 여기 쓰임새는 '개인 천장' 한 개라 정렬 차이가 비율에 거의 영향 없다.
    ///
    /// **M3 (2026-09-02): 이 함수와 `last_5h_total_tokens`는 Claude 전용 근사다.**
    /// 5h 블록은 Claude의 쿼터 모양이지 Codex(주간)의 것이 아니므로 `agent='claude'`
    /// 로 거른다 — M2 이후 codex 행이 섞여 천장을 부풀리고 있었다. 소스 어댑터
    /// (`oauth_usage.rs`·`agent.rs`) 밖에서 창 길이를 아는 유일한 자리이며,
    /// 소스가 창을 직접 못 준 경우의 **마지막 폴백**으로만 쓰인다(spec §5.3).
    pub fn five_hour_block_and_peak(&self) -> Result<(i64, i64)> {
        let conn = self.conn.lock().unwrap();
        let (cur, peak): (i64, i64) = conn.query_row(
            "WITH b AS (
               SELECT CAST(strftime('%s', timestamp) / 18000 AS INTEGER) AS blk,
                      SUM(input_tokens + output_tokens
                          + cache_creation_tokens + cache_read_tokens) AS toks
               FROM processed_events
               WHERE agent = 'claude'
               GROUP BY blk
             )
             SELECT COALESCE((SELECT toks FROM b
                              WHERE blk = CAST(strftime('%s','now') / 18000 AS INTEGER)), 0),
                    COALESCE((SELECT MAX(toks) FROM b), 0)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((cur, peak))
    }

    pub fn today_cache_read(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let v: i64 = conn.query_row(
            "SELECT COALESCE(SUM(cache_read_tokens), 0)
             FROM processed_events
             WHERE processed_at >= datetime('now', 'start of day')",
            [],
            |r| r.get(0),
        )?;
        Ok(v)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TokenBucket {
    pub label: String,
    pub tokens: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UsageSummary {
    pub last_5h: i64,
    pub today: i64,
    pub last_7d: i64,
    pub by_model: Vec<TokenBucket>,
    pub by_project: Vec<TokenBucket>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TokenCursors {
    pub cache_read: i64,
    pub cache_creation: i64,
    pub output: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Totals {
    pub input: i64,
    pub output: i64,
    pub cache_creation: i64,
    pub cache_read: i64,
    pub events: i64,
}

impl Totals {
    /// XP 화폐 (spec §6.1). **`input`은 빼고 더한다** — Claude에선 streaming
    /// placeholder라 못 쓴다.
    ///
    /// v4까진 `cache_read` 하나였다. 바꿔도 크기가 거의 안 변한다 — 실측
    /// 116,368 이벤트에서 `cache_read`가 이 합의 95.9%(22.70B / 23.67B)라
    /// 같은 divisor로 Lv 68 → 69, 즉 리셋이 아니라 +1이다. 그래서
    /// `XP_LV_DIVISOR`는 5,000,000 그대로 둔다.
    pub fn xp(&self) -> i64 {
        self.cache_read + self.cache_creation + self.output
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppSettings {
    pub notifications_level: String, // "off" | "impt" | "all"
    pub autostart_enabled: bool,
    pub track_since_install: bool,
    pub install_baseline_cache_read: i64,
    pub active_character: String, // PET_VARIANTS id, e.g. "bunny"
    pub case_color: String,       // "beige" | "platinum" | "ecru" | "slate"
    pub invert_screen: bool,
    pub coach_backend: String,      // "auto" | "claude" | "codex" | "ollama"
    pub coach_ollama_model: String, // ollama tag, e.g. "exaone3.5:7.8b"
    pub always_on_top: bool,        // desk floats above every window
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            notifications_level: "impt".into(),
            autostart_enabled: false,
            track_since_install: false,
            install_baseline_cache_read: 0,
            active_character: "bunny".into(),
            case_color: "beige".into(),
            invert_screen: false,
            coach_backend: "auto".into(),
            coach_ollama_model: "exaone3.5:7.8b".into(),
            always_on_top: true,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HookRawRow {
    pub id: i64,
    pub received_at: String,
    pub event_type: String,
    pub payload_json: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillRow {
    pub skill_name: String,
    pub unlocked_at: String,
    pub use_count: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DungeonRow {
    pub id: String,
    pub source: String,
    pub project_path: Option<String>,
    pub status: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    #[serde(default)]
    pub event_count: i64,
}

/// Globally recent hook event with session annotation. Used by the
/// Dungeon Log view to render a timeline of recent tool activity.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecentHookEvent {
    pub id: i64,
    pub ts: String,
    pub event_type: String, // "PostToolUse" / "SessionStart" / "SessionEnd" / "Stop" ...
    pub session_id: Option<String>,
    pub tool: Option<String>,
    pub file_path: Option<String>,
    pub command: Option<String>,
    pub duration_ms: Option<i64>,
    pub interrupted: bool,
}

/// A compact view of one PostToolUse event for retrospective generation.
/// Pulled out of `hook_events_raw.payload_json` on demand.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RawPostToolUse {
    pub ts: String,
    pub tool: String,
    pub file_path: Option<String>,
    pub command: Option<String>,
    pub pattern: Option<String>,
    pub url: Option<String>,
    pub duration_ms: Option<i64>,
    pub interrupted: bool,
}

fn row_to_dungeon(r: &rusqlite::Row<'_>) -> rusqlite::Result<DungeonRow> {
    Ok(DungeonRow {
        id: r.get(0)?,
        source: r.get(1)?,
        project_path: r.get(2)?,
        status: r.get(3)?,
        started_at: r.get(4)?,
        ended_at: r.get(5)?,
        event_count: 0,
    })
}

fn row_to_dungeon_with_count(r: &rusqlite::Row<'_>) -> rusqlite::Result<DungeonRow> {
    Ok(DungeonRow {
        id: r.get(0)?,
        source: r.get(1)?,
        project_path: r.get(2)?,
        status: r.get(3)?,
        started_at: r.get(4)?,
        ended_at: r.get(5)?,
        event_count: r.get(6)?,
    })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DiaryEntry {
    pub id: i64,
    pub event_type: String,
    pub description: String,
    pub model: Option<String>,
    pub project_path: Option<String>,
    pub occurred_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CharacterState {
    pub stage: String,
    pub hunger: i32,
    pub happiness: i32,
    pub intelligence: i32,
    pub stamina: i32,
    pub curiosity: i32,
    pub total_xp: i64,
    pub born_at: String,
    pub last_interaction_at: String,
    pub last_tick_at: String,
    pub last_cache_read_seen: i64,
}

impl Db {
    pub fn load_character(&self) -> Result<Option<CharacterState>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT stage, hunger, happiness, intelligence, stamina, curiosity,
                        total_xp, born_at, last_interaction_at, last_tick_at, last_cache_read_seen
                 FROM character_state WHERE id = 1",
                [],
                |r| {
                    Ok(CharacterState {
                        stage: r.get(0)?,
                        hunger: r.get(1)?,
                        happiness: r.get(2)?,
                        intelligence: r.get(3)?,
                        stamina: r.get(4)?,
                        curiosity: r.get(5)?,
                        total_xp: r.get(6)?,
                        born_at: r.get(7)?,
                        last_interaction_at: r.get(8)?,
                        last_tick_at: r.get(9)?,
                        last_cache_read_seen: r.get(10)?,
                    })
                },
            )
            .ok();
        Ok(row)
    }

    pub fn insert_diary(
        &self,
        event_type: &str,
        description: &str,
        model: Option<&str>,
        project_path: Option<&str>,
        occurred_at: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO diary_entries (event_type, description, model, project_path, occurred_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![event_type, description, model, project_path, occurred_at],
        )?;
        Ok(())
    }

    pub fn recent_diary(&self, limit: i64) -> Result<Vec<DiaryEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, event_type, description, model, project_path, occurred_at
             FROM diary_entries
             ORDER BY occurred_at DESC, id DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok(DiaryEntry {
                id: r.get(0)?,
                event_type: r.get(1)?,
                description: r.get(2)?,
                model: r.get(3)?,
                project_path: r.get(4)?,
                occurred_at: r.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Latest event's model + project path (used to label a meal).
    pub fn latest_event_meta(&self) -> Result<Option<(Option<String>, Option<String>)>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT model, project_path FROM processed_events
                 ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .ok();
        Ok(row)
    }

    pub fn load_settings(&self) -> Result<AppSettings> {
        let conn = self.conn.lock().unwrap();
        let row = conn.query_row(
            "SELECT notifications_level, autostart_enabled, track_since_install,
                    install_baseline_cache_read, active_character, case_color, invert_screen,
                    coach_backend, coach_ollama_model, always_on_top
             FROM app_settings WHERE id = 1",
            [],
            |r| {
                Ok(AppSettings {
                    notifications_level: r.get(0)?,
                    autostart_enabled: r.get::<_, i64>(1)? != 0,
                    track_since_install: r.get::<_, i64>(2)? != 0,
                    install_baseline_cache_read: r.get(3)?,
                    active_character: r.get(4)?,
                    case_color: r.get(5)?,
                    invert_screen: r.get::<_, i64>(6)? != 0,
                    coach_backend: r.get(7)?,
                    coach_ollama_model: r.get(8)?,
                    always_on_top: r.get::<_, i64>(9)? != 0,
                })
            },
        );
        Ok(row.unwrap_or_default())
    }

    pub fn save_settings(&self, s: &AppSettings) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE app_settings SET
               notifications_level = ?1,
               autostart_enabled = ?2,
               track_since_install = ?3,
               install_baseline_cache_read = ?4,
               active_character = ?5,
               case_color = ?6,
               invert_screen = ?7,
               coach_backend = ?8,
               coach_ollama_model = ?9,
               always_on_top = ?10
             WHERE id = 1",
            params![
                s.notifications_level,
                s.autostart_enabled as i64,
                s.track_since_install as i64,
                s.install_baseline_cache_read,
                s.active_character,
                s.case_color,
                s.invert_screen as i64,
                s.coach_backend,
                s.coach_ollama_model,
                s.always_on_top as i64,
            ],
        )?;
        Ok(())
    }

    /// Wipe character state + diary, keep events (re-scan won't double-insert).
    pub fn reset_character(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM character_state", [])?;
        conn.execute("DELETE FROM diary_entries", [])?;
        conn.execute("DELETE FROM character_save", [])?;
        conn.execute("DELETE FROM dungeon_events", [])?;
        conn.execute("DELETE FROM dungeons", [])?;
        Ok(())
    }

    /* ─── v2: character_save ────────────────────────────────────── */

    pub fn load_save(&self) -> Result<Option<CharacterSave>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT lv, xp, hp, hp_max, gold, sp,
                        int_stat, str_stat, agi_stat, wis_stat, luk_stat,
                        class, born_at, last_save
                 FROM character_save WHERE id = 1",
                [],
                |r| {
                    Ok(CharacterSave {
                        lv: r.get(0)?,
                        xp: r.get(1)?,
                        hp: r.get(2)?,
                        hp_max: r.get(3)?,
                        gold: r.get(4)?,
                        sp: r.get(5)?,
                        int_stat: r.get(6)?,
                        str_stat: r.get(7)?,
                        agi_stat: r.get(8)?,
                        wis_stat: r.get(9)?,
                        luk_stat: r.get(10)?,
                        class: r.get(11)?,
                        born_at: r.get(12)?,
                        last_save: r.get(13)?,
                    })
                },
            )
            .ok();
        Ok(row)
    }

    pub fn save_save(&self, s: &CharacterSave) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO character_save
             (id, lv, xp, hp, hp_max, gold, sp,
              int_stat, str_stat, agi_stat, wis_stat, luk_stat,
              class, born_at, last_save)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id) DO UPDATE SET
               lv = excluded.lv,
               xp = excluded.xp,
               hp = excluded.hp,
               hp_max = excluded.hp_max,
               gold = excluded.gold,
               sp = excluded.sp,
               int_stat = excluded.int_stat,
               str_stat = excluded.str_stat,
               agi_stat = excluded.agi_stat,
               wis_stat = excluded.wis_stat,
               luk_stat = excluded.luk_stat,
               class = excluded.class,
               last_save = excluded.last_save",
            params![
                s.lv, s.xp, s.hp, s.hp_max, s.gold, s.sp,
                s.int_stat, s.str_stat, s.agi_stat, s.wis_stat, s.luk_stat,
                s.class, s.born_at, s.last_save,
            ],
        )?;
        Ok(())
    }

    /* ─── v2: cursors (moved from character_state to app_settings) ── */

    pub fn load_cursors(&self) -> Result<TokenCursors> {
        let conn = self.conn.lock().unwrap();
        let row = conn.query_row(
            "SELECT last_cache_read_seen, last_cache_creation_seen, last_output_seen
             FROM app_settings WHERE id = 1",
            [],
            |r| {
                Ok(TokenCursors {
                    cache_read: r.get(0)?,
                    cache_creation: r.get(1)?,
                    output: r.get(2)?,
                })
            },
        );
        Ok(row.unwrap_or_default())
    }

    pub fn save_cursors(&self, c: &TokenCursors) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE app_settings SET
               last_cache_read_seen = ?1,
               last_cache_creation_seen = ?2,
               last_output_seen = ?3
             WHERE id = 1",
            params![c.cache_read, c.cache_creation, c.output],
        )?;
        Ok(())
    }

    /* ─── v2: migration dialog flag ─────────────────────────────── */

    /// v5 M6 — 온보딩 시퀀스를 이미 끝냈나.
    pub fn onboarded(&self) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT onboarded FROM app_settings WHERE id = 1", [], |r| {
            r.get::<_, i64>(0)
        })? == 1)
    }

    pub fn mark_onboarded(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE app_settings SET onboarded = 1 WHERE id = 1", [])?;
        Ok(())
    }

    pub fn migration_dialog_shown(&self) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let v: i64 = conn
            .query_row(
                "SELECT migration_dialog_shown FROM app_settings WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok(v != 0)
    }

    pub fn mark_migration_dialog_shown(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE app_settings SET migration_dialog_shown = 1 WHERE id = 1",
            [],
        )?;
        Ok(())
    }

    pub fn insert_hook_raw(
        &self,
        received_at: &str,
        event_type: &str,
        payload_json: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO hook_events_raw (received_at, event_type, payload_json)
             VALUES (?1, ?2, ?3)",
            params![received_at, event_type, payload_json],
        )?;
        Ok(())
    }

    pub fn count_hook_raw(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM hook_events_raw",
            [],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    /* ─── MVP-8: drainer + dungeons ─────────────────────────────── */

    pub fn load_last_drained_hook_id(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let v: i64 = conn
            .query_row(
                "SELECT last_drained_hook_id FROM app_settings WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok(v)
    }

    pub fn save_last_drained_hook_id(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE app_settings SET last_drained_hook_id = ?1 WHERE id = 1",
            params![id],
        )?;
        Ok(())
    }

    /// Fetch raw hook rows with id > after_id, ordered by id ascending.
    pub fn drain_hook_events(&self, after_id: i64) -> Result<Vec<HookRawRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, received_at, event_type, payload_json
             FROM hook_events_raw
             WHERE id > ?1
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![after_id], |r| {
            Ok(HookRawRow {
                id: r.get(0)?,
                received_at: r.get(1)?,
                event_type: r.get(2)?,
                payload_json: r.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// INSERT OR IGNORE — dungeon id is the Claude session_id, so SessionStart
    /// fired by `source: "compact"` on the same session is a no-op.
    pub fn upsert_dungeon_active(
        &self,
        id: &str,
        source: &str,
        project_path: Option<&str>,
        started_at: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "INSERT OR IGNORE INTO dungeons (id, source, project_path, status, started_at)
             VALUES (?1, ?2, ?3, 'active', ?4)",
            params![id, source, project_path, started_at],
        )?;
        Ok(changed > 0)
    }

    /// Mark dungeon cleared only if it's currently active.
    pub fn close_dungeon(&self, id: &str, ended_at: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE dungeons
             SET status = 'cleared', ended_at = ?2
             WHERE id = ?1 AND status = 'active'",
            params![id, ended_at],
        )?;
        Ok(changed > 0)
    }

    pub fn active_dungeon(&self) -> Result<Option<DungeonRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, source, project_path, status, started_at, ended_at
                 FROM dungeons WHERE status = 'active'
                 ORDER BY started_at DESC LIMIT 1",
                [],
                row_to_dungeon,
            )
            .ok();
        Ok(row)
    }

    pub fn dungeon_by_id(&self, id: &str) -> Result<Option<DungeonRow>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, source, project_path, status, started_at, ended_at
                 FROM dungeons WHERE id = ?1",
                params![id],
                row_to_dungeon,
            )
            .ok();
        Ok(row)
    }

    /// Resurrect a non-active dungeon back to `active` (clearing ended_at).
    /// No-op if the row is already active or absent. Returns true if a row
    /// was actually flipped.
    pub fn reactivate_dungeon(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE dungeons
             SET status = 'active', ended_at = NULL
             WHERE id = ?1 AND status != 'active'",
            params![id],
        )?;
        Ok(changed > 0)
    }

    /// Mark active dungeon (by id) as abandoned with the given timestamp.
    /// Returns true if a row was actually flipped.
    pub fn abandon_dungeon(&self, id: &str, ended_at: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE dungeons
             SET status = 'abandoned', ended_at = ?2
             WHERE id = ?1 AND status = 'active'",
            params![id, ended_at],
        )?;
        Ok(changed > 0)
    }

    /// INSERT OR IGNORE on `tool_use_id`. Returns true if a new row was
    /// inserted (caller can use this gate for one-shot side effects like
    /// HP damage on interrupted tool uses).
    pub fn insert_dungeon_event(
        &self,
        dungeon_id: &str,
        occurred_at: &str,
        event_type: &str,
        payload_json: &str,
        tool_use_id: Option<&str>,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "INSERT OR IGNORE INTO dungeon_events
             (dungeon_id, occurred_at, event_type, payload_json, tool_use_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![dungeon_id, occurred_at, event_type, payload_json, tool_use_id],
        )?;
        Ok(changed > 0)
    }

    pub fn count_dungeon_events(&self, dungeon_id: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM dungeon_events WHERE dungeon_id = ?1",
            params![dungeon_id],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    pub fn load_last_hook_at(&self) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let row: Option<String> = conn
            .query_row(
                "SELECT last_hook_at FROM app_settings WHERE id = 1",
                [],
                |r| r.get::<_, Option<String>>(0),
            )
            .unwrap_or(None);
        Ok(row)
    }

    pub fn save_last_hook_at(&self, ts: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE app_settings SET last_hook_at = ?1 WHERE id = 1",
            params![ts],
        )?;
        Ok(())
    }

    /* ─── MVP-10: skills (mirrored MCP tool usage) ─────────────── */

    /// INSERT OR IGNORE + UPDATE use_count. Returns true if this was the
    /// first time the skill was unlocked (i.e. INSERT actually inserted).
    pub fn record_skill_use(&self, skill_name: &str, occurred_at: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO skills (skill_name, unlocked_at, use_count)
             VALUES (?1, ?2, 0)",
            params![skill_name, occurred_at],
        )?;
        conn.execute(
            "UPDATE skills SET use_count = use_count + 1 WHERE skill_name = ?1",
            params![skill_name],
        )?;
        Ok(inserted > 0)
    }

    /// One-shot backfill: scan `hook_events_raw` for every distinct
    /// PostToolUse `tool_name`, run it through `accept` (caller-decides
    /// whether it's a "skill" or basic), and record cumulative counts
    /// with first-seen timestamp. Returns (skills_inserted, total_uses).
    pub fn backfill_skills<F: Fn(&str) -> bool>(&self, accept: F) -> Result<(i64, i64)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT
                json_extract(payload_json, '$.tool_name') AS tool_name,
                MIN(received_at) AS first_seen,
                COUNT(*) AS uses
             FROM hook_events_raw
             WHERE event_type = 'PostToolUse'
               AND tool_name IS NOT NULL
               AND tool_name != ''
             GROUP BY tool_name",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        let mut new_skills = 0i64;
        let mut total_uses = 0i64;
        for row in rows {
            let (name_opt, first_opt, uses) = row?;
            let Some(name) = name_opt else { continue };
            let first = first_opt.unwrap_or_else(|| String::from(""));
            if !accept(&name) {
                continue;
            }
            let changed = conn.execute(
                "INSERT OR IGNORE INTO skills (skill_name, unlocked_at, use_count)
                 VALUES (?1, ?2, 0)",
                params![name, first],
            )?;
            new_skills += changed as i64;
            // Overwrite use_count with the historical count. Idempotent.
            conn.execute(
                "UPDATE skills SET use_count = ?2 WHERE skill_name = ?1",
                params![name, uses],
            )?;
            total_uses += uses;
        }
        Ok((new_skills, total_uses))
    }

    pub fn list_skills(&self) -> Result<Vec<SkillRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT skill_name, unlocked_at, use_count
             FROM skills
             ORDER BY use_count DESC, unlocked_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SkillRow {
                skill_name: r.get(0)?,
                unlocked_at: r.get(1)?,
                use_count: r.get(2)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// List dungeons for the Dungeons view, newest-first.
    ///
    /// Filters out "bare home-directory" noise: a `claude` session opened
    /// in `$HOME` with zero recorded tool activity. These pile up from
    /// quick throwaway chats and used to flood the limit, burying real
    /// project sessions. We exclude them *before* LIMIT (filtering after
    /// fetch wouldn't help — the noise would already fill the window).
    /// Active sessions are always kept regardless.
    pub fn list_dungeons(&self, limit: i64) -> Result<Vec<DungeonRow>> {
        let home = dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT d.id, d.source, d.project_path, d.status, d.started_at, d.ended_at,
                    COALESCE((SELECT COUNT(*) FROM dungeon_events e WHERE e.dungeon_id = d.id), 0) AS ec
             FROM dungeons d
             WHERE d.status = 'active'
                OR NOT (
                     d.project_path = ?2
                     AND (SELECT COUNT(*) FROM dungeon_events e WHERE e.dungeon_id = d.id) = 0
                   )
             ORDER BY d.started_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit, home], row_to_dungeon_with_count)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Pull raw PostToolUse payloads for a given session id (= dungeon id).
    /// We use hook_events_raw rather than dungeon_events because the latter
    /// only stores the tool name + duration — for a retrospective we want
    /// file paths, bash commands, etc., which live in the raw payload.
    pub fn raw_post_tool_uses(
        &self,
        session_id: &str,
        limit: i64,
    ) -> Result<Vec<RawPostToolUse>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT received_at, payload_json
             FROM hook_events_raw
             WHERE event_type = 'PostToolUse'
               AND payload_json LIKE '%\"session_id\":\"' || ?1 || '\"%'
             ORDER BY id ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![session_id, limit], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (ts, payload) = row?;
            // Best-effort parse; skip malformed.
            let v: serde_json::Value = match serde_json::from_str(&payload) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let tool = v
                .get("tool_name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let input = v.get("tool_input");
            let file_path = input
                .and_then(|i| i.get("file_path"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            let command = input
                .and_then(|i| i.get("command"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            let pattern = input
                .and_then(|i| i.get("pattern"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            let url = input
                .and_then(|i| i.get("url"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            let duration_ms = v
                .get("tool_response")
                .and_then(|r| r.get("duration_ms"))
                .and_then(|x| x.as_i64());
            let interrupted = v
                .get("tool_response")
                .and_then(|r| r.get("interrupted"))
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            out.push(RawPostToolUse {
                ts,
                tool,
                file_path,
                command,
                pattern,
                url,
                duration_ms,
                interrupted,
            });
        }
        Ok(out)
    }

    /// Recent hook events for the global Log view. Returns newest-first
    /// rows with session_id + tool + target extracted from the payload.
    pub fn recent_hook_events(&self, limit: i64) -> Result<Vec<RecentHookEvent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, received_at, event_type, payload_json
             FROM hook_events_raw
             ORDER BY id DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, ts, event_type, payload) = row?;
            let v: serde_json::Value = match serde_json::from_str(&payload) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let session_id = v
                .get("session_id")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            let tool = v
                .get("tool_name")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            let input = v.get("tool_input");
            let file_path = input
                .and_then(|i| i.get("file_path"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            let command = input
                .and_then(|i| i.get("command"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            let duration_ms = v
                .get("tool_response")
                .and_then(|r| r.get("duration_ms"))
                .and_then(|x| x.as_i64());
            let interrupted = v
                .get("tool_response")
                .and_then(|r| r.get("interrupted"))
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            out.push(RecentHookEvent {
                id,
                ts,
                event_type,
                session_id,
                tool,
                file_path,
                command,
                duration_ms,
                interrupted,
            });
        }
        Ok(out)
    }

    /// Save the LLM-generated retrospective onto a dungeon row.
    pub fn save_retrospective(&self, dungeon_id: &str, body: &str, at: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE dungeons SET retrospective = ?2, retrospective_at = ?3 WHERE id = ?1",
            params![dungeon_id, body, at],
        )?;
        Ok(())
    }

    /// `(retrospective, retrospective_at)` if present.
    pub fn load_retrospective(
        &self,
        dungeon_id: &str,
    ) -> Result<Option<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT retrospective, retrospective_at FROM dungeons WHERE id = ?1",
                params![dungeon_id],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .ok();
        Ok(match row {
            Some((Some(body), Some(at))) => Some((body, at)),
            _ => None,
        })
    }

    pub fn distinct_projects(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT project_path FROM processed_events
             WHERE project_path IS NOT NULL AND project_path != ''",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn save_character(&self, s: &CharacterState) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO character_state
             (id, stage, hunger, happiness, intelligence, stamina, curiosity,
              total_xp, born_at, last_interaction_at, last_tick_at, last_cache_read_seen)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
               stage = excluded.stage,
               hunger = excluded.hunger,
               happiness = excluded.happiness,
               intelligence = excluded.intelligence,
               stamina = excluded.stamina,
               curiosity = excluded.curiosity,
               total_xp = excluded.total_xp,
               last_interaction_at = excluded.last_interaction_at,
               last_tick_at = excluded.last_tick_at,
               last_cache_read_seen = excluded.last_cache_read_seen",
            params![
                s.stage,
                s.hunger,
                s.happiness,
                s.intelligence,
                s.stamina,
                s.curiosity,
                s.total_xp,
                s.born_at,
                s.last_interaction_at,
                s.last_tick_at,
                s.last_cache_read_seen,
            ],
        )?;
        Ok(())
    }
}
