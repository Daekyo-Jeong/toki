#!/usr/bin/env bash
# 버전을 **세 곳에 한 번에** 올린다.
#
#   bash scripts/bump-version.sh 1.0.0
#
# 세 곳인 이유와 각각의 역할:
#   app/src-tauri/tauri.conf.json  번들 버전 (Info.plist에 박힌다)
#   app/package.json               프런트 패키지 버전
#   app/src-tauri/Cargo.toml       **앱이 자기 버전이라고 믿는 값** (CARGO_PKG_VERSION)
#
# 마지막 것을 빠뜨리면 새 버전이 자기를 구버전으로 알고 업데이트 알림이
# 영원히 뜬다(2026-09-04 실측). 손으로 고치지 말고 이 스크립트를 쓴다.
set -euo pipefail
cd "$(dirname "$0")/.."
V="${1:-}"
[ -n "$V" ] || { echo "사용법: bash scripts/bump-version.sh <버전>  (예: 1.0.0)" >&2; exit 2; }
case "$V" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) echo "✗ semver 형식이 아니에요: $V" >&2; exit 2 ;;
esac

python3 - "$V" <<'PY'
import json, re, sys
v = sys.argv[1]
for p in ("app/package.json", "app/src-tauri/tauri.conf.json"):
    d = json.load(open(p, encoding="utf-8"))
    d["version"] = v
    json.dump(d, open(p, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
    open(p, "a", encoding="utf-8").write("\n")
# Cargo.toml은 [package] 블록의 첫 version만 바꾼다(의존성 버전은 건드리지 않게).
p = "app/src-tauri/Cargo.toml"
s = open(p, encoding="utf-8").read()
s = re.sub(r'(?m)^version = "[^"]+"', f'version = "{v}"', s, count=1)
open(p, "w", encoding="utf-8").write(s)
PY

echo "✓ $V 로 맞췄습니다:"
grep -m1 '"version"' app/package.json
grep -m1 '"version"' app/src-tauri/tauri.conf.json
awk -F'"' '/^version = "/{print "  Cargo.toml      "$2; exit}' app/src-tauri/Cargo.toml
