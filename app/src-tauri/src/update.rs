//! v5 M8 — 자체 업데이트 (spec §9.4).
//!
//! **왜 `tauri-plugin-updater`가 아닌가**: 그쪽은 별도 서명 키쌍이 필요하고,
//! **그 키를 잃으면 기존 사용자가 영영 갱신을 못 받는다.** 여기서는 체크섬으로
//! 무결성을 잡으므로 잃을 키가 없고, 설치(`install.sh`)와 갱신이 **같은 경로**를
//! 읽어 검증할 것이 하나다. 사내 도구 reFlex가 이 구조로 굴러왔다.
//!
//! 흐름: 최신 버전 확인 → 받기 → SHA256 검증 → `/Applications` 교체 → 재실행.
//! **검증에 실패하면 교체하지 않는다** — 앱은 그대로 떠 있고 화면에 이유가 뜬다.

use serde::Serialize;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Runtime};

/// 배포 레포. 기본은 공개 레포이고, **빌드 때 `TOKI_RELEASE_REPO`로 바꿀 수 있다** —
/// 공개 전에 임시 레포로 업데이트 경로를 검증하려는 것이다. 테스트 주소가
/// 코드에 박히면 진짜 릴리스에 섞여 나갈 위험이 있어 환경변수로 뺐다.
fn release_repo() -> &'static str {
    option_env!("TOKI_RELEASE_REPO").unwrap_or("Daekyo-Jeong/toki")
}

/// GitHub Releases의 `latest`는 안정 URL이라 버전을 몰라도 받는다.
/// 릴리스에 올리는 것: `version.txt` · `macos.txt` · `SHA256SUMS` · dmg · install.sh.
fn release_base() -> String {
    format!("https://github.com/{}/releases/latest/download", release_repo())
}
const APP_NAME: &str = "Toki";
/// 네트워크가 죽어 있어도 앱이 멈추면 안 된다.
const NET_TIMEOUT: Duration = Duration::from_secs(15);

fn url(file: &str) -> String {
    format!("{}/{file}", release_base())
}

/// "1.2.3" → (1,2,3). 접두 `v`는 떼고 본다.
fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches('v');
    let mut it = s.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next().unwrap_or("0").parse().ok()?;
    let c = it.next().unwrap_or("0").parse().ok()?;
    Some((a, b, c))
}

fn is_newer(remote: &str, current: &str) -> bool {
    match (parse_semver(remote), parse_semver(current)) {
        (Some(r), Some(c)) => r > c,
        // 파싱이 안 되면 **업데이트가 있다고 하지 않는다** — 잘못된 알림이
        // 조용한 침묵보다 나쁘다(사용자가 누르면 교체가 일어난다).
        _ => false,
    }
}

fn fetch_text(file: &str) -> Option<String> {
    let resp = ureq::get(&url(file)).timeout(NET_TIMEOUT).call().ok()?;
    Some(resp.into_string().ok()?.trim().to_string())
}

#[derive(Serialize, Clone)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: Option<String>,
    pub available: bool,
}

/// 최신 버전 확인. **네트워크가 나가는 유일한 지점**이고, 나가는 건 버전 조회뿐이다
/// (spec §9.2 — 수집 서버 없음과 충돌하지 않는다).
///
/// `async` 필수: 동기 커맨드는 메인 스레드에서 돌아 15초 타임아웃이 UI를 통째로
/// 얼린다. 워커로 격리한다.
#[tauri::command]
pub async fn update_check() -> UpdateInfo {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let latest = tauri::async_runtime::spawn_blocking(|| fetch_text("version.txt"))
        .await
        .ok()
        .flatten();
    let available = latest.as_deref().map(|l| is_newer(l, &current)).unwrap_or(false);
    UpdateInfo { current, latest, available }
}

/// 교체 스크립트. **앱 밖에서 돌아야 한다** — 자기 자신을 덮어쓰는 프로세스는
/// 중간에 사라진다. 앱이 종료되길 기다렸다가 교체하고 다시 띄운다.
fn spawn_updater() -> Result<(), String> {
    let base = release_base();
    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
BASE="{base}"
APP="{APP_NAME}"
log() {{ echo "[toki-update] $*"; }}
# 실패하면 **기존 앱을 다시 띄운다.** 호출한 앱은 이미 종료 수순이라,
# 그냥 죽으면 사용자가 빈손이 된다.
fail() {{ log "$*"; open "/Applications/$APP.app" 2>/dev/null || true; exit 1; }}

FILE=$(curl -fsSL "$BASE/macos.txt" | tr -d '[:space:]') || fail "macos.txt 실패"
[ -n "$FILE" ] || fail "macos.txt 비어 있음"
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
log "downloading $FILE"
curl -fsSL -o "$TMP/t.dmg" "$BASE/$FILE" || fail "dmg 다운로드 실패: $FILE"

# 체크섬 — 이게 무결성의 유일한 방어선이다. 없거나 어긋나면 **교체하지 않는다.**
SUMS=$(curl -fsSL "$BASE/SHA256SUMS") || fail "SHA256SUMS 실패"
EXPECTED=$(echo "$SUMS" | awk -v f="$FILE" '$2 == f {{ print $1 }}')
[ -n "$EXPECTED" ] || fail "SHA256SUMS 에 $FILE 없음"
ACTUAL=$(shasum -a 256 "$TMP/t.dmg" | awk '{{ print $1 }}')
[ "$EXPECTED" = "$ACTUAL" ] || fail "체크섬 불일치 — 교체 중단"
log "checksum ok"

# 앱이 완전히 사라질 때까지 기다린다(종료-재실행 레이스 방지).
for _ in $(seq 1 50); do
  pgrep -f "$APP.app/Contents/MacOS/app" >/dev/null || break
  sleep 0.2
done
pkill -9 -f "$APP.app/Contents/MacOS/app" 2>/dev/null || true
sleep 0.3

MOUNT=$(hdiutil attach -nobrowse -readonly "$TMP/t.dmg" | grep -E "/Volumes/" | tail -1 | awk -F'\t' '{{print $NF}}')
[ -d "$MOUNT/$APP.app" ] || {{ hdiutil detach -quiet "$MOUNT" 2>/dev/null || true; fail "마운트에서 앱 못 찾음"; }}
log "replacing /Applications/$APP.app"
rm -rf "/Applications/$APP.app"
cp -R "$MOUNT/$APP.app" /Applications/
hdiutil detach -quiet "$MOUNT" || true
# quarantine 제거는 **하지 않는다** — 공증된 빌드라 필요 없고, 우회를 습관으로
# 만들면 안 된다(spec §9.4).
log "relaunching"
open "/Applications/$APP.app"
log "done"
"#
    );

    let path = std::env::temp_dir().join("toki-self-update.sh");
    std::fs::write(&path, script).map_err(|e| format!("스크립트 작성 실패: {e}"))?;
    let p = path.to_string_lossy().to_string();
    // 로그를 남긴다 — 교체가 실패하면 앱이 없으니 화면으로 물어볼 수가 없다.
    std::process::Command::new("/bin/bash")
        .arg("-c")
        .arg(format!("nohup bash '{p}' >/tmp/toki-update.log 2>&1 &"))
        .spawn()
        .map_err(|e| format!("업데이터 실행 실패: {e}"))?;
    Ok(())
}

/// 업데이트 실행. **사용자가 누른 경우에만** 불린다 — 자동 설치 금지(spec §9.4).
#[tauri::command]
pub async fn update_run<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(spawn_updater)
        .await
        .map_err(|e| format!("업데이트 태스크 실패: {e}"))??;
    // 스크립트가 앱 종료를 기다리고 있다 — 잠깐 뒤 빠져 준다.
    let app2 = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(400));
        app2.exit(0);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compare() {
        assert!(is_newer("1.0.1", "1.0.0"));
        assert!(is_newer("1.1.0", "1.0.9"));
        assert!(is_newer("2.0.0", "1.9.9"));
        assert!(is_newer("v1.0.1", "1.0.0")); // 접두 v 허용
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("0.9.9", "1.0.0")); // 다운그레이드 제안 금지
    }

    /// 못 읽는 값에 "업데이트 있음"이라고 하지 않는다 — 누르면 교체가 일어난다.
    #[test]
    fn unparsable_never_offers_update() {
        assert!(!is_newer("", "1.0.0"));
        assert!(!is_newer("최신", "1.0.0"));
        assert!(!is_newer("1.0.0", "이상한버전"));
    }
}
