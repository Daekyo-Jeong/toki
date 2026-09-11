#!/usr/bin/env bash
# Toki 릴리스 패키징 — 서명(+선택 공증) 빌드와 배포 세트를 만든다.
#
#   bash scripts/release.sh                 # 서명만 (빠름, 개발 확인용)
#   bash scripts/release.sh --notarize      # 서명 + 공증 + staple (정식 배포)
#   bash scripts/release.sh --no-build      # 이미 빌드된 산출물로 세트만 다시 구성
#   bash scripts/release.sh --notarize --win # + Windows NSIS 설치본도 한 세트에 (M9)
#
# **Windows 는 mac 에서 크로스빌드한다** (cargo-xwin + nsis, reFlex 2026-08-20 방식).
# 크로스빌드는 컴파일러를 옮긴 것이지 런타임을 옮긴 게 아니다 — exe 는 반드시
# Windows 실기에서 설치·트레이·투명 데스크·알림까지 확인한 뒤 릴리스에 올린다.
# 사전 요구: brew install nsis llvm · cargo install cargo-xwin · rustup target add x86_64-pc-windows-msvc
#
# 산출물: app/build/latest/ (설치 스크립트가 참조) + app/build/v<버전>/ (보관)
#
# **arm64 전용이다** (2026-09-04 결정). universal은 빌드 시간과 용량이 두 배인데
# 인텔 맥은 지원 대상이 아니다 — spec §9.1.
#
# 서명은 `tauri.conf.json`의 `signingIdentity`가 항상 수행한다. 공증만 선택인
# 이유는 Apple 서버 큐라 수 분~수십 분 걸리기 때문. **공개 배포본은 반드시
# `--notarize`로 만든다** — 서명만으로는 처음 여는 맥에서 경고가 뜬다.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
APP_DIR="$ROOT/app"
BUILD="$APP_DIR/build"
TARGET="aarch64-apple-darwin"
APP_NAME="Toki"

DO_BUILD=1
DO_NOTARIZE=0
DO_WIN=0
WIN_TARGET="x86_64-pc-windows-msvc"
for a in "$@"; do
  case "$a" in
    --no-build) DO_BUILD=0 ;;
    --notarize) DO_NOTARIZE=1 ;;
    --win) DO_WIN=1 ;;
    *) echo "모르는 옵션: $a" >&2; exit 2 ;;
  esac
done

# ── 버전은 **세 곳**에 있고 셋이 같아야 한다 ──────────────────────────────
# tauri.conf.json = 번들 버전(Info.plist), package.json = 프런트,
# **Cargo.toml = 앱이 자기 버전이라고 믿는 값**(`CARGO_PKG_VERSION`).
# 셋째를 빠뜨리면 새 버전이 자기를 구버전으로 알고 **업데이트 알림이 영원히
# 뜬다** — 2026-09-04에 실제로 그랬다(교체는 됐는데 알림이 안 사라짐).
VERSION=$(python3 -c "import json;print(json.load(open('$APP_DIR/src-tauri/tauri.conf.json'))['version'])")
PKG_VERSION=$(python3 -c "import json;print(json.load(open('$APP_DIR/package.json'))['version'])")
CARGO_VERSION=$(awk -F'"' '/^version = "/{print $2; exit}' "$APP_DIR/src-tauri/Cargo.toml")
if [ "$VERSION" != "$PKG_VERSION" ] || [ "$VERSION" != "$CARGO_VERSION" ]; then
  echo "✗ 버전 불일치 — 셋이 같아야 합니다." >&2
  echo "    tauri.conf.json = $VERSION" >&2
  echo "    package.json    = $PKG_VERSION" >&2
  echo "    Cargo.toml      = $CARGO_VERSION  ← 앱이 자기 버전으로 읽는 값" >&2
  echo "  `bash scripts/bump-version.sh <새버전>` 으로 한 번에 맞추세요." >&2
  exit 1
fi
echo "→ Toki $VERSION ($TARGET)"

# ── 공증 자격증명 ──────────────────────────────────────────────────────────
# `.notary-creds`(gitignore됨)에서 읽는다. 셸 히스토리에 암호를 남기지 않으려는 것.
#   APPLE_ID=you@example.com
#   APPLE_PASSWORD=앱-전용-암호        # appleid.apple.com 에서 발급
#   APPLE_TEAM_ID=V6347DY5X7
if [ "$DO_NOTARIZE" -eq 1 ]; then
  [ -f "$ROOT/.notary-creds" ] && { set -a; . "$ROOT/.notary-creds"; set +a; }
  if [ -z "${APPLE_ID:-}" ] || [ -z "${APPLE_PASSWORD:-}" ] || [ -z "${APPLE_TEAM_ID:-}" ]; then
    echo "✗ --notarize 인데 자격증명이 없어요." >&2
    echo "  .notary-creds 에 APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID 를 넣으세요." >&2
    exit 1
  fi
  echo "→ 서명 + 공증 + staple (Apple 큐 때문에 수 분~수십 분)"
else
  # 서명만 — tauri가 공증을 시도하지 않도록 비운다.
  unset APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID APPLE_API_KEY APPLE_API_ISSUER 2>/dev/null || true
  echo "→ 서명만 (공증 생략). 배포본은 --notarize 로 다시 만드세요."
fi

if [ "$DO_BUILD" -eq 1 ]; then
  ( cd "$APP_DIR" && npm run tauri build -- --target "$TARGET" --bundles app,dmg )
fi

# ── Windows (M9) ──────────────────────────────────────────────────────────
# `--bundles nsis` 명시 필수 — msi(WiX)는 Windows 에서만 만들 수 있다.
# llvm 의 clang-cl/lld-link 가 PATH 에 있어야 cargo-xwin 이 링크한다.
if [ "$DO_WIN" -eq 1 ] && [ "$DO_BUILD" -eq 1 ]; then
  command -v cargo-xwin >/dev/null || { echo "✗ cargo-xwin 없음 — cargo install cargo-xwin" >&2; exit 1; }
  command -v makensis  >/dev/null || { echo "✗ makensis 없음 — brew install nsis" >&2; exit 1; }
  export PATH="/opt/homebrew/opt/llvm/bin:$PATH"
  ( cd "$APP_DIR" && npm run tauri build -- --runner cargo-xwin --target "$WIN_TARGET" --bundles nsis )
fi

OUT="$APP_DIR/src-tauri/target/$TARGET/release/bundle"
APP_PATH="$OUT/macos/$APP_NAME.app"
DMG_PATH=$(ls "$OUT/dmg/"*.dmg 2>/dev/null | head -1 || true)
[ -d "$APP_PATH" ] || { echo "✗ $APP_PATH 없음" >&2; exit 1; }
[ -n "$DMG_PATH" ] || { echo "✗ dmg 없음 ($OUT/dmg)" >&2; exit 1; }

# ── 검증 — 여기서 걸러야 사용자 맥에서 안 걸린다 ───────────────────────────
echo "→ 서명 확인"
# `codesign -dv`는 stderr로 쓴다. 그리고 **grep -q로 바로 파이프하면 안 된다** —
# 첫 매치에서 grep이 파이프를 닫아 codesign이 SIGPIPE로 죽고, `pipefail`이 그걸
# 실패로 읽어 멀쩡한 서명을 "adhoc"이라 보고한다(2026-09-04에 실제로 물렸다).
SIGINFO=$(codesign -dv --verbose=2 "$APP_PATH" 2>&1 || true)
printf '%s\n' "$SIGINFO" | grep -E "Authority|TeamIdentifier|Signature" || true
case "$SIGINFO" in
  *"TeamIdentifier=V6347DY5X7"*) ;;
  *) echo "✗ Developer ID 서명이 아닙니다 (adhoc?). signingIdentity 설정을 확인하세요." >&2; exit 1 ;;
esac
echo "→ 아키텍처: $(lipo -archs "$APP_PATH/Contents/MacOS/app")"

if [ "$DO_NOTARIZE" -eq 1 ]; then
  # **dmg도 따로 공증해야 한다.** tauri는 .app만 공증하고 dmg는 그 뒤에 만들어
  # 서명만 한다 — 그대로 두면 브라우저로 dmg를 받은 사람에게 경고가 뜬다
  # (curl 설치는 quarantine이 안 붙어 무관하지만, dmg 직접 다운로드도 제공하기로
  # 했다 — spec §9.4). 안에 든 앱은 이미 공증돼 있어 이 심사는 빠르다.
  echo "→ dmg 공증"
  xcrun notarytool submit "$DMG_PATH" \
    --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" \
    --wait 2>&1 | grep -Ei "id:|status:|message:" || true
  xcrun stapler staple "$DMG_PATH" 2>&1 | tail -1

  echo "→ Gatekeeper 판정"
  spctl -a -vvv -t install "$APP_PATH" 2>&1 | head -3
  STAPLE=$(xcrun stapler validate "$DMG_PATH" 2>&1 | tail -1)
  echo "  dmg staple: $STAPLE"
  case "$STAPLE" in
    *"validate action worked"*|*"The validate action worked"*) ;;
    *) echo "✗ dmg에 공증 티켓이 안 붙었습니다 — 브라우저로 받으면 경고가 뜹니다." >&2; exit 1 ;;
  esac
fi

# ── 배포 세트 ─────────────────────────────────────────────────────────────
DMG_NAME="Toki-$VERSION-arm64.dmg"
# **latest/ 는 매번 비운다.** 안 비우면 이전 버전 dmg가 남고, 아래 안내대로
# `latest/*` 를 통째로 올리면 **구버전이 릴리스에 같이 올라간다**
# (2026-09-07 1.0.0 릴리스 때 0.9.0 dmg가 남아 있었다 — 올리기 직전에 발견).
# SHA256SUMS 에는 새 dmg만 적히므로 받는 쪽이 어느 걸 집었냐에 따라 검증이
# 갈린다. 세대 혼입은 나눠 올릴 때만 나는 게 아니다.
rm -rf "$BUILD/latest"
mkdir -p "$BUILD/latest" "$BUILD/v$VERSION"
cp "$DMG_PATH" "$BUILD/latest/$DMG_NAME"
cp "$DMG_PATH" "$BUILD/v$VERSION/$DMG_NAME"
printf '%s\n' "$VERSION"  > "$BUILD/latest/version.txt"
printf '%s\n' "$DMG_NAME" > "$BUILD/latest/macos.txt"
if [ "$DO_WIN" -eq 1 ]; then
  WIN_OUT="$APP_DIR/src-tauri/target/$WIN_TARGET/release/bundle/nsis"
  WIN_SRC=$(ls "$WIN_OUT/"*-setup.exe 2>/dev/null | head -1 || true)
  [ -n "$WIN_SRC" ] || { echo "✗ Windows setup.exe 없음 ($WIN_OUT)" >&2; exit 1; }
  # 파일명은 dmg 와 같은 규칙: 제품-버전-아키텍처.
  WIN_NAME="Toki-$VERSION-x64-setup.exe"
  cp "$WIN_SRC" "$BUILD/latest/$WIN_NAME"
  cp "$WIN_SRC" "$BUILD/v$VERSION/$WIN_NAME"
  printf '%s\n' "$WIN_NAME" > "$BUILD/latest/windows.txt"
  cp "$ROOT/scripts/install.ps1" "$BUILD/latest/install.ps1"
  echo "→ Windows: $WIN_NAME (실기 검증 전이면 릴리스에 올리지 말 것)"
fi
# **SHA256SUMS 는 세트가 다 모인 뒤 한 번에** — 한 플랫폼만 새로 넣고 합계를
# 안 갱신하면 그쪽 업데이터가 체크섬 불일치로 멈춘다(reFlex 2026-08-31 실사고).
( cd "$BUILD/latest" && shasum -a 256 *.dmg *.exe 2>/dev/null > SHA256SUMS )
cp "$BUILD/latest/version.txt" "$BUILD/latest/macos.txt" "$BUILD/latest/SHA256SUMS" "$BUILD/v$VERSION/"
[ -f "$BUILD/latest/windows.txt" ] && cp "$BUILD/latest/windows.txt" "$BUILD/v$VERSION/"
# 설치 스크립트도 릴리스에 같이 올린다 — curl 한 줄이 이 파일을 받는다.
cp "$ROOT/scripts/install.sh" "$BUILD/latest/install.sh"

echo
echo "✓ $BUILD/latest"
ls -la "$BUILD/latest"
echo
echo "다음: GitHub Release 에 latest/ 의 파일을 **한 번에** 올린다."
echo
echo "  gh release create v$VERSION $BUILD/latest/* --title \"Toki $VERSION\" --notes-file <릴리스노트>"
echo
echo "⚠️ 나눠 올리지 말 것 — dmg 는 최신인데 SHA256SUMS 가 이전 세대면 업데이터가"
echo "   체크섬 불일치로 교체를 중단해 업데이트가 통째로 막힌다(reFlex 2026-08-31 실사고)."
echo "   굳이 나눠야 하면 설치본 먼저, 메타(SHA256SUMS·version.txt·macos.txt)는 맨 마지막."
