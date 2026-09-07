//! One-shot v1 → v2 data location migration.
//!   v1: <app_data_dir>/tamagotchi.sqlite   (resolved via Tauri identifier)
//!   v2: ~/.toki/engine.sqlite
//!
//! - Pure copy (preserves all v1 tables — schema swap happens in MVP-6b).
//! - WAL / SHM sidecar files migrated too.
//! - Source file moved to `~/.toki/legacy-backup/` (safety; not deleted).

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub struct Paths {
    pub toki_dir: PathBuf,
    pub db_path: PathBuf,
}

pub fn resolve(app: &tauri::AppHandle) -> Result<Paths> {
    let home = dirs::home_dir().context("no home dir")?;
    let toki_dir = home.join(".toki");
    fs::create_dir_all(&toki_dir).context("create ~/.toki")?;
    let db_path = toki_dir.join("engine.sqlite");

    if !db_path.exists() {
        // Try v1 migration.
        if let Ok(v1_dir) = tauri::Manager::path(app).app_data_dir() {
            let v1_db = v1_dir.join("tamagotchi.sqlite");
            if v1_db.exists() {
                println!("[migrate] v1 db found at {} → migrating", v1_db.display());
                copy_with_sidecars(&v1_db, &db_path).context("copy v1 db")?;
                let backup_dir = toki_dir.join("legacy-backup");
                fs::create_dir_all(&backup_dir).ok();
                let backup_path = backup_dir.join("tamagotchi.sqlite");
                // Move original aside (safety preserved, but old code path won't be used again).
                if move_with_sidecars(&v1_db, &backup_path).is_ok() {
                    println!("[migrate] v1 db moved to {}", backup_path.display());
                }
            }
        }
    }

    Ok(Paths { toki_dir, db_path })
}

fn copy_with_sidecars(src: &Path, dst: &Path) -> Result<()> {
    fs::copy(src, dst)?;
    for suffix in ["-wal", "-shm"] {
        let s = with_suffix(src, suffix);
        if s.exists() {
            let d = with_suffix(dst, suffix);
            let _ = fs::copy(&s, &d);
        }
    }
    Ok(())
}

fn move_with_sidecars(src: &Path, dst: &Path) -> Result<()> {
    fs::rename(src, dst)?;
    for suffix in ["-wal", "-shm"] {
        let s = with_suffix(src, suffix);
        if s.exists() {
            let d = with_suffix(dst, suffix);
            let _ = fs::rename(&s, &d);
        }
    }
    Ok(())
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}
