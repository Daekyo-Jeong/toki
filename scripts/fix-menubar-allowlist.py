#!/usr/bin/env python3
"""macOS 26 Tahoe "메뉴 막대에서 허용" 목록에서 Toki를 되살린다.

**증상**: 트레이 아이콘이 등록은 되는데 메뉴 막대에 안 그려진다.

**원인**: Tahoe는 어느 앱의 메뉴바 항목을 보일지 허용 목록으로 관리하는데,
앱을 다른 앱의 프로세스에서 띄우면(터미널에서 실행 등) 그 부모 앱의
`menuItemLocations`에 자식으로 엮인다. 부모가 꺼져 있으면 자식도 같이 막힌다.
Toki를 터미널에서 직접 실행하면 매번 재발한다 — **하지 마라.**

**이 스크립트**: 저장소를 열어 (1) Toki 자신의 `isAllowed`를 켜고
(2) 다른 앱의 `menuItemLocations`에 끼어든 Toki 항목을 떼어낸다.

실행: 파일이 TCC 보호 대상이라 **전체 디스크 접근 권한이 있는 터미널**에서
사용자가 직접 돌려야 한다.

    python3 scripts/fix-menubar-allowlist.py          # 진단만
    python3 scripts/fix-menubar-allowlist.py --apply  # 수정 + 서비스 재시작
"""
import os
import plistlib
import shutil
import subprocess
import sys
import time

BUNDLE_IDS = {"me.dkdk.toki", "me.dkdk.toki.desktop", "com.dk.tokentamagotchi"}
PLIST = os.path.expanduser(
    "~/Library/Group Containers/group.com.apple.controlcenter/"
    "Library/Preferences/group.com.apple.controlcenter.plist"
)


def bundle_of(entry):
    """{'bundle': {'_0': 'com.example'}} 모양에서 id를 꺼낸다. 아니면 None."""
    if isinstance(entry, dict):
        b = entry.get("bundle")
        if isinstance(b, dict):
            v = b.get("_0")
            if isinstance(v, str):
                return v
        if isinstance(b, str):
            return b
    return None


def main():
    apply = "--apply" in sys.argv
    if not os.path.exists(PLIST):
        print("허용 목록 파일이 없습니다:", PLIST)
        return 1
    try:
        with open(PLIST, "rb") as f:
            root = plistlib.load(f)
    except PermissionError:
        print("읽기 거부됨 — 이 터미널에 '전체 디스크 접근' 권한이 필요합니다.")
        print("시스템 설정 → 개인정보 보호 및 보안 → 전체 디스크 접근")
        return 1

    blob = root.get("trackedApplications")
    if not isinstance(blob, bytes):
        print("trackedApplications 항목이 없습니다 (이 macOS는 해당 없음).")
        return 0

    tracked = plistlib.loads(blob)
    if not isinstance(tracked, list):
        print("예상과 다른 구조입니다 — 손대지 않습니다.")
        return 1

    # 구조: [ {bundle:{_0:id}}, {isAllowed, location, menuItemLocations:[...]}, ... ] 교대
    changed = []
    i = 0
    while i + 1 < len(tracked):
        key, val = tracked[i], tracked[i + 1]
        bid = bundle_of(key)
        if isinstance(val, dict):
            # (1) Toki 자신이 차단돼 있으면 켠다.
            if bid in BUNDLE_IDS and val.get("isAllowed") is False:
                val["isAllowed"] = True
                changed.append(f"{bid}: isAllowed False → True")
            # (2) 남의 항목에 Toki가 자식으로 끼어 있으면 뗀다.
            kids = val.get("menuItemLocations")
            if isinstance(kids, list) and bid not in BUNDLE_IDS:
                keep = [k for k in kids if bundle_of(k) not in BUNDLE_IDS]
                if len(keep) != len(kids):
                    val["menuItemLocations"] = keep
                    changed.append(f"{bid}의 하위 항목에서 toki 제거 ({len(kids)}→{len(keep)})")
        i += 2

    if not changed:
        print("고칠 것이 없습니다. (트레이가 여전히 안 보이면 원인이 다른 곳입니다)")
        return 0

    print("발견:")
    for c in changed:
        print("  -", c)
    if not apply:
        print("\n--apply 를 붙이면 실제로 고칩니다.")
        return 0

    backup = f"{PLIST}.toki-backup.{int(time.time())}"
    shutil.copy2(PLIST, backup)
    print("백업:", backup)

    root["trackedApplications"] = plistlib.dumps(tracked, fmt=plistlib.FMT_BINARY)
    try:
        with open(PLIST, "wb") as f:
            plistlib.dump(root, f, fmt=plistlib.FMT_BINARY)
    except PermissionError:
        print("쓰기 거부됨 — '전체 디스크 접근' 권한이 필요합니다.")
        return 1

    # 캐시를 비우고 서비스를 다시 띄워야 반영된다.
    subprocess.run(["killall", "cfprefsd"], capture_output=True)
    subprocess.run(["killall", "ControlCenter"], capture_output=True)
    print("완료. 메뉴 막대를 확인하세요. (Toki를 한 번 껐다 켜면 확실합니다)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
