//! Localhost HTTP server for Claude Code hooks.
//!
//! Endpoint: POST /hook?event=<name>
//! Body: arbitrary JSON (Claude Code hook payload)
//!
//! For MVP-7 we just persist raw events. MVP-8 consumes them into dungeons.

use crate::db::Db;
use chrono::Utc;
use std::io::Read;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use tauri::{AppHandle, Emitter};
use tiny_http::{Method, Response, Server};

const PORT_RANGE_START: u16 = 6996;
const PORT_RANGE_END: u16 = 7010;

/// Find a free port in [start, end] and return both the bound listener + port.
fn pick_port() -> Option<(TcpListener, u16)> {
    for p in PORT_RANGE_START..=PORT_RANGE_END {
        let addr: SocketAddr = ([127, 0, 0, 1], p).into();
        if let Ok(l) = TcpListener::bind(addr) {
            return Some((l, p));
        }
    }
    None
}

/// Persist the chosen port so the installer/uninstaller can find it.
fn write_port_file(toki_dir: &PathBuf, port: u16) {
    let _ = std::fs::write(toki_dir.join("port"), port.to_string());
}

pub fn spawn(app: AppHandle, db: Arc<Db>, toki_dir: PathBuf) -> Option<u16> {
    let (listener, port) = pick_port()?;
    write_port_file(&toki_dir, port);
    println!("[hook] listening on 127.0.0.1:{}", port);

    let server = Server::from_listener(listener, None).ok()?;
    thread::spawn(move || run(server, app, db));
    Some(port)
}

fn run(server: Server, app: AppHandle, db: Arc<Db>) {
    for mut req in server.incoming_requests() {
        if req.method() != &Method::Post {
            let _ = req.respond(Response::from_string("method not allowed").with_status_code(405));
            continue;
        }
        if !req.url().starts_with("/hook") {
            let _ = req.respond(Response::from_string("not found").with_status_code(404));
            continue;
        }

        // Extract ?event=<name>
        let event = url_query(req.url(), "event").unwrap_or_else(|| "unknown".to_string());
        // M4: Codex 훅은 `agent=codex`를 달고 온다 → 이벤트명에 접두를 붙여 같은
        // 테이블에 섞여도 출처를 안다. drain_hooks는 아는 이름만 매칭하므로
        // `codex/…`는 저장만 되고 무시된다(정규화는 후속).
        let event = match url_query(req.url(), "agent").as_deref() {
            Some("codex") => format!("codex/{event}"),
            _ => event,
        };

        // Read body (capped)
        let mut body = String::new();
        let _ = req.as_reader().take(1024 * 1024).read_to_string(&mut body);
        // Ensure JSON-ish; if empty, store {}
        if body.trim().is_empty() {
            body = "{}".to_string();
        }
        // Validate it's parseable JSON; if not, wrap as text
        let stored = match serde_json::from_str::<serde_json::Value>(&body) {
            Ok(_) => body,
            Err(_) => serde_json::json!({"raw_text": body}).to_string(),
        };

        let now = Utc::now().to_rfc3339();
        if let Err(e) = db.insert_hook_raw(&now, &event, &stored) {
            eprintln!("[hook] insert err: {}", e);
        } else {
            println!("[hook] {} ({} bytes)", event, stored.len());
            let _ = app.emit(
                "hook-received",
                serde_json::json!({
                    "event": event,
                    "received_at": now,
                }),
            );
        }

        let _ = req.respond(Response::from_string("ok").with_status_code(200));
    }
}

fn url_query(url: &str, key: &str) -> Option<String> {
    let qs = url.split_once('?')?.1;
    for kv in qs.split('&') {
        let (k, v) = kv.split_once('=')?;
        if k == key {
            return Some(urldecode(v));
        }
    }
    None
}

fn urldecode(s: &str) -> String {
    // tiny manual decode for our limited use (just %20 etc.)
    let mut out = String::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'+' {
            out.push(' ');
        } else if b == b'%' {
            let h = bytes.next();
            let l = bytes.next();
            if let (Some(h), Some(l)) = (h, l) {
                if let (Some(h), Some(l)) = (
                    (h as char).to_digit(16),
                    (l as char).to_digit(16),
                ) {
                    out.push(((h * 16 + l) as u8) as char);
                    continue;
                }
            }
        } else {
            out.push(b as char);
        }
    }
    out
}
