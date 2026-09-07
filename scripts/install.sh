#!/usr/bin/env bash
# Toki 설치 스크립트 (macOS · Apple Silicon)
#
#   curl -fsSL https://github.com/Daekyo-Jeong/toki/releases/latest/download/install.sh | bash
#
# 스크립트를 먼저 읽고 실행하고 싶다면:
#   curl -fsSL https://github.com/Daekyo-Jeong/toki/releases/latest/download/install.sh -o install.sh
#   less install.sh && bash install.sh
#
# **왜 브라우저 다운로드가 아니라 터미널인가**: `com.apple.quarantine` 딱지는
# 브라우저가 붙인다. curl로 받은 파일에는 안 붙어서 "손상되었기 때문에 열 수
# 없습니다"를 아예 안 만난다. dmg를 직접 받아 더블클릭해도 된다 — 공증돼 있어
# 경고가 안 뜬다. 편한 쪽으로 고르면 된다.
#
# **Gatekeeper를 우회하지 않는다.** 이 앱은 Developer ID로 서명되고 Apple 공증을
# 받았다. `xattr`로 딱지를 떼는 스크립트를 보면 그건 무서명 배포다.
set -euo pipefail

# 기본은 공개 레포. 공개 전 검증용으로 TOKI_REPO 로 임시 레포를 가리킬 수 있다.
REPO="${TOKI_REPO:-Daekyo-Jeong/toki}"
BASE="https://github.com/${REPO}/releases/latest/download"
APP_NAME="Toki"

say() { printf '%s\n' "$*"; }
die() { printf '✗ %s\n' "$*" >&2; exit 1; }

# ── 환경 확인 ──────────────────────────────────────────────────────────────
[ "$(uname -s)" = "Darwin" ] || die "macOS 전용이에요."
if [ "$(uname -m)" != "arm64" ]; then
  die "Apple Silicon(arm64) 전용이에요. 인텔 맥은 아직 지원하지 않아요."
fi

# ── 받을 파일 이름은 릴리스가 알려준다 (버전 하드코딩 금지) ────────────────
# 스크립트에 파일명을 박으면 릴리스마다 스크립트를 고쳐야 한다. `macos.txt`가
# 실제 파일명을, `version.txt`가 버전을 들고 있다.
VERSION=$(curl -fsSL "$BASE/version.txt" 2>/dev/null | tr -d '[:space:]' || true)
FILE=$(curl -fsSL "$BASE/macos.txt" 2>/dev/null | tr -d '[:space:]' || true)
[ -n "$FILE" ] || die "릴리스 정보를 못 읽었어요. 네트워크나 릴리스 상태를 확인해 주세요."
say "→ Toki ${VERSION:-?} 설치 ($FILE)"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
DMG="$TMP/toki.dmg"

say "→ 받는 중..."
curl -fL --progress-bar -o "$DMG" "$BASE/$FILE" || die "다운로드 실패: $FILE"

# ── 무결성 ────────────────────────────────────────────────────────────────
# 체크섬이 안 맞으면 **설치하지 않는다.** 받다 만 파일이거나 릴리스 업로드가
# 세대가 섞인 상태다(reFlex 2026-08-31 실사고: 메타만 먼저 올라가 업데이트가
# 통째로 막혔다 — 안전장치가 제대로 작동한 것).
SUMS=$(curl -fsSL "$BASE/SHA256SUMS" 2>/dev/null || true)
if [ -n "$SUMS" ]; then
  EXPECTED=$(printf '%s\n' "$SUMS" | awk -v f="$FILE" '$2 == f { print $1 }')
  if [ -n "$EXPECTED" ]; then
    ACTUAL=$(shasum -a 256 "$DMG" | awk '{print $1}')
    [ "$EXPECTED" = "$ACTUAL" ] || die "체크섬 불일치 — 손상되었거나 변조됐어요. 설치 중단."
    say "  ✓ 체크섬 확인"
  else
    say "  ! SHA256SUMS 에 $FILE 항목이 없어요 — 검증 건너뜀"
  fi
else
  say "  ! SHA256SUMS 를 못 받았어요 — 검증 건너뜀"
fi

# ── 설치 ──────────────────────────────────────────────────────────────────
if pgrep -f "$APP_NAME.app/Contents/MacOS/app" >/dev/null 2>&1; then
  say "→ 실행 중인 Toki 종료..."
  osascript -e "tell application \"$APP_NAME\" to quit" 2>/dev/null || true
  for _ in $(seq 1 25); do
    pgrep -f "$APP_NAME.app/Contents/MacOS/app" >/dev/null 2>&1 || break
    sleep 0.2
  done
  pkill -f "$APP_NAME.app/Contents/MacOS/app" 2>/dev/null || true
  sleep 0.3
fi

say "→ 마운트..."
MOUNT=$(hdiutil attach -nobrowse -readonly "$DMG" | grep -E "/Volumes/" | tail -1 | awk -F'\t' '{print $NF}')
[ -d "$MOUNT/$APP_NAME.app" ] || {
  hdiutil detach -quiet "$MOUNT" 2>/dev/null || true
  die "마운트 결과에서 $APP_NAME.app 을 못 찾았어요."
}

say "→ /Applications 에 설치..."
rm -rf "/Applications/$APP_NAME.app"
cp -R "$MOUNT/$APP_NAME.app" /Applications/
hdiutil detach -quiet "$MOUNT" || true

# 공증 티켓이 실제로 붙어 있는지 본다 — 여기서 걸러야 사용자가 첫 실행에서
# 경고를 안 만난다. 실패해도 설치는 이미 끝났으니 안내만 하고 멈추지 않는다.
if ! spctl -a -vv "/Applications/$APP_NAME.app" >/dev/null 2>&1; then
  say "  ! Gatekeeper 확인에 실패했어요. 처음 열 때 경고가 뜨면 알려주세요."
fi

say ""
say "✓ 설치 완료: /Applications/$APP_NAME.app"
say "  메뉴 막대에 토키 아이콘이 생겨요. 안 보이면 시스템 설정 →"
say "  제어 센터 → '메뉴 막대에서 허용'을 확인해 주세요."
open "/Applications/$APP_NAME.app"
