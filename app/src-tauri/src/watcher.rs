use crate::agent::{self, AgentSource};
use crate::db::{Db, EventRow};
use anyhow::Result;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

fn process_file(db: &Db, path: &Path, src: &dyn AgentSource) -> Result<usize> {
    let path_str = path.to_string_lossy().to_string();
    let start = db.get_offset(&path_str)?;

    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    if file_len <= start {
        return Ok(0);
    }
    file.seek(SeekFrom::Start(start))?;
    let reader = BufReader::new(file);

    let mut inserted = 0usize;
    let mut bytes_consumed = start;
    for line_res in reader.lines() {
        let line = match line_res {
            Ok(l) => l,
            Err(_) => break,
        };
        bytes_consumed += line.len() as u64 + 1; // +1 for \n
        if let Some(ev) = src.parse_line(&line) {
            let row = EventRow {
                uuid: &ev.uuid,
                agent: ev.agent.as_str(),
                session_id: &ev.session_id,
                timestamp: &ev.timestamp,
                model: ev.model.as_deref(),
                input_tokens: ev.input_tokens,
                output_tokens: ev.output_tokens,
                cache_creation_tokens: ev.cache_creation_tokens,
                cache_read_tokens: ev.cache_read_tokens,
                project_path: ev.project_path.as_deref(),
            };
            if db.insert_event(&row)? {
                inserted += 1;
            }
            feed_thrash_watch(&path_str, &ev);
        }
    }
    db.set_offset(&path_str, bytes_consumed)?;
    Ok(inserted)
}

/// §3.3.5 ①: feed the live retry-streak tracker from the tail. Only lines
/// fresh enough to be "now" count — catch-up scans replay months of history
/// and stale edits must not raise the tray alert.
const THRASH_FRESH_SECS: i64 = 10 * 60;

fn feed_thrash_watch(session_key: &str, ev: &agent::Event) {
    let fresh = chrono::DateTime::parse_from_rfc3339(&ev.timestamp)
        .ok()
        .map(|ts| (chrono::Utc::now() - ts.with_timezone(&chrono::Utc)).num_seconds() < THRASH_FRESH_SECS)
        .unwrap_or(false);
    if !fresh {
        return;
    }
    // 지금은 Claude의 tool_use 블록 모양을 안다. M2에서 Codex의
    // custom_tool_call을 붙일 때 `raw_content`를 공통 모양으로 정규화한다.
    let Some(blocks) = ev.raw_content.as_ref().and_then(|c| c.as_array()) else {
        return;
    };
    let tw = crate::thrash_watch::ThrashWatch::global();
    for b in blocks {
        if b.get("type").and_then(|x| x.as_str()) != Some("tool_use") {
            continue;
        }
        let nm = b.get("name").and_then(|x| x.as_str()).unwrap_or("");
        match nm {
            "Bash" => tw.note_bash(session_key),
            "Edit" | "Write" => {
                let fp = b
                    .get("input")
                    .and_then(|i| i.get("file_path"))
                    .and_then(|x| x.as_str());
                if let Some(fp) = fp {
                    let bn = fp.rsplit('/').next().unwrap_or(fp);
                    if !crate::retro::is_living_doc(bn) {
                        tw.note_edit(session_key, bn);
                    }
                }
            }
            _ => {}
        }
    }
}

fn scan_all(db: &Db, root: &Path, src: &dyn AgentSource) -> Result<usize> {
    let mut total = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if src.is_log_file(&p) {
                match process_file(db, &p, src) {
                    Ok(n) => total += n,
                    Err(e) => eprintln!("[watcher] process {} error: {}", p.display(), e),
                }
            }
        }
    }
    Ok(total)
}

/// 감지된 에이전트마다 watcher 스레드를 하나씩 띄운다. M1엔 Claude 하나뿐이지만
/// 구조는 여럿을 전제한다 — M2에서 `agent::sources()`에 Codex가 추가되면
/// 여기는 손대지 않아도 된다.
pub fn spawn(db: Arc<Db>) {
    for src in agent::sources() {
        let name = src.id().as_str();
        let Some(root) = src.watch_root() else {
            eprintln!("[watcher] {name}: no home dir");
            continue;
        };
        if !root.exists() {
            eprintln!("[watcher] {name}: {} 없음 — 이 에이전트는 건너뛴다", root.display());
            continue;
        }
        let db = Arc::clone(&db);
        std::thread::spawn(move || watch_one(db, root, src));
    }
}

fn watch_one(db: Arc<Db>, root: PathBuf, src: Box<dyn AgentSource>) {
    let src = src.as_ref();
    {
        // Initial catch-up scan
        match scan_all(&db, &root, src) {
            Ok(n) => println!("[watcher] initial scan: {} new events", n),
            Err(e) => eprintln!("[watcher] initial scan error: {}", e),
        }
        log_totals(&db);

        // notify watcher
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher: RecommendedWatcher =
            match notify::recommended_watcher(move |res| {
                let _ = tx.send(res);
            }) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("[watcher] create error: {}", e);
                    return;
                }
            };
        if let Err(e) = watcher.watch(&root, RecursiveMode::Recursive) {
            eprintln!("[watcher] watch error: {}", e);
            return;
        }

        // Polling fallback ticker (macOS fsevents can drop events for nested files)
        let (poll_tx, poll_rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            if poll_tx.send(()).is_err() {
                break;
            }
        });

        loop {
            let mut changed_paths: HashSet<PathBuf> = HashSet::new();
            let mut do_full_scan = false;

            // Block on either notify event or polling tick
            crossbeam_select(&rx, &poll_rx, &mut changed_paths, &mut do_full_scan);

            if do_full_scan {
                match scan_all(&db, &root, src) {
                    Ok(n) if n > 0 => {
                        println!("[watcher] poll scan: {} new events", n);
                        log_totals(&db);
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("[watcher] poll scan error: {}", e),
                }
                continue;
            }

            let mut any = 0usize;
            for p in changed_paths {
                if src.is_log_file(&p) {
                    match process_file(&db, &p, src) {
                        Ok(n) => any += n,
                        Err(e) => eprintln!("[watcher] process error: {}", e),
                    }
                }
            }
            if any > 0 {
                println!("[watcher] notify: {} new events", any);
                log_totals(&db);
            }
        }
    }
}

fn log_totals(db: &Db) {
    match db.totals() {
        Ok(t) => println!(
            "[totals] events={} input={} output={} cache_create={} cache_read={}",
            t.events, t.input, t.output, t.cache_creation, t.cache_read
        ),
        Err(e) => eprintln!("[totals] err: {}", e),
    }
}

/// Minimal hand-rolled select: drain notify events first (non-blocking),
/// then either return if we have any, or wait on polling tick with a timeout.
fn crossbeam_select(
    rx: &std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    poll_rx: &std::sync::mpsc::Receiver<()>,
    changed: &mut HashSet<PathBuf>,
    do_full_scan: &mut bool,
) {
    // Try one blocking recv first (either source) using short timeout loop
    loop {
        // Drain notify
        while let Ok(res) = rx.try_recv() {
            if let Ok(ev) = res {
                for p in ev.paths {
                    changed.insert(p);
                }
            }
        }
        if !changed.is_empty() {
            return;
        }
        // Check poll
        if poll_rx.try_recv().is_ok() {
            *do_full_scan = true;
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}
