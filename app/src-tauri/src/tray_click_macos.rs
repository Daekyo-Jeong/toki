//! macOS 27 메뉴 막대 아이콘 클릭 보정.
//!
//! **증상**: macOS 27 에서 트레이 아이콘을 왼쪽 클릭하면 데스크 토글 대신 메뉴(Show/Quit)가 뜬다.
//!
//! **원인 (reFlex 에서 2026-11-16 앱 로컬 이벤트 모니터로 실측)**: tray-icon(0.23)은 메뉴를
//! NSStatusItem 에 계속 붙여 두고, 아이콘 위에 덧씌운 뷰로 클릭을 받는다. macOS 26 까지는 그 뷰가
//! 먼저 가로채 문제가 없었는데, macOS 27 은 메뉴가 붙은 아이콘의 왼쪽 클릭을 **시스템이 직접 처리해
//! 메뉴를 띄우고** 앱에는 알리지 않는다. 오른쪽 클릭은 앱에 "RightMouseDown(type 3), 버튼 번호 0,
//! NSStatusBarWindow" 로 들어온다(버튼 번호 0 이라 왼쪽처럼 보이지만 오른쪽 클릭이다).
//! `NSApp.currentEvent` 는 처리 중 이벤트가 아니라 직전 이벤트를 돌려줘서 판별에 쓰면 안 된다.
//!
//! **해법**: macOS 27 이상에서만 메뉴를 아이콘에서 뗀다. 그러면 왼쪽 클릭은 다시 tray-icon 뷰로
//! 들어와 기존 tray 이벤트(lib.rs `toggle_desk`)가 처리한다. 오른쪽 클릭은 로컬 모니터가 가로채
//! 메뉴를 잠깐 붙여 `performClick` 으로 띄우고 다시 뗀다. 26 이하는 아무것도 바꾸지 않는다.
//!
//! reFlex 와 달리 버튼 action 보조 경로는 두지 않는다 — Toki 의 왼쪽 클릭은 **토글**이라 두 경로가
//! 다 오면 켰다 바로 꺼진다.

use objc2::msg_send;
use objc2::runtime::AnyObject;

use crate::desk::debug_log;

/// 메뉴를 아이콘에서 떼어 두고, 오른쪽 클릭 때만 붙여서 띄운다 (macOS 27+).
/// `status_item`: NSStatusItem 포인터. 메인 스레드에서 한 번 부른다.
pub fn install_menu_on_right_click_only(status_item: *mut AnyObject) {
    use block2::RcBlock;
    use std::ptr::NonNull;
    let major = macos_major();
    if status_item.is_null() || major < 27 {
        return;
    }
    unsafe {
        let menu: *mut AnyObject = msg_send![status_item, menu];
        if menu.is_null() {
            return;
        }
        // 메뉴·아이콘은 앱 수명 동안 유지되므로 포인터를 붙잡아 둔다
        let _: *mut AnyObject = msg_send![menu, retain];
        let _: *mut AnyObject = msg_send![status_item, retain];
        let _: () = msg_send![status_item, setMenu: std::ptr::null_mut::<AnyObject>()];
        let (item_addr, menu_addr) = (status_item as usize, menu as usize);

        let Some(cls) = objc2::runtime::AnyClass::get(c"NSEvent") else {
            let _: () = msg_send![status_item, setMenu: menu];
            return;
        };
        let mask: u64 = 1 << 3; // RightMouseDown
        let block = RcBlock::new(move |ev: NonNull<AnyObject>| -> *mut AnyObject {
            let e = ev.as_ptr();
            if !in_status_bar_window(e) {
                return e;
            }
            debug_log("[tray] right click → menu");
            let item = item_addr as *mut AnyObject;
            let menu = menu_addr as *mut AnyObject;
            let button: *mut AnyObject = msg_send![item, button];
            let _: () = msg_send![item, setMenu: menu];
            if !button.is_null() {
                // 메뉴가 붙은 버튼의 performClick 은 메뉴를 띄우고, 닫힐 때까지 돌아오지 않는다
                let _: () = msg_send![button, performClick: std::ptr::null_mut::<AnyObject>()];
            }
            let _: () = msg_send![item, setMenu: std::ptr::null_mut::<AnyObject>()];
            std::ptr::null_mut() // tray-icon 에는 넘기지 않는다
        });
        let monitor: *mut AnyObject =
            msg_send![cls, addLocalMonitorForEventsMatchingMask: mask, handler: &*block];
        if monitor.is_null() {
            // 실패하면 메뉴를 되돌려 최소한 메뉴는 쓸 수 있게 둔다
            let _: () = msg_send![status_item, setMenu: menu];
            debug_log("[tray] 오른쪽 클릭 메뉴 설치 실패 — 메뉴를 아이콘에 다시 붙임");
        } else {
            std::mem::forget(block);
            debug_log(&format!(
                "[tray] macOS {major} — 메뉴는 오른쪽 클릭에만, 왼쪽 클릭은 데스크 토글"
            ));
        }
    }
}

fn macos_major() -> i64 {
    #[repr(C)]
    struct OsVersion {
        major: isize,
        minor: isize,
        patch: isize,
    }
    unsafe impl objc2::Encode for OsVersion {
        const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
            "_NSOperatingSystemVersion",
            &[isize::ENCODING, isize::ENCODING, isize::ENCODING],
        );
    }
    unsafe {
        let Some(cls) = objc2::runtime::AnyClass::get(c"NSProcessInfo") else {
            return 0;
        };
        let info: *mut AnyObject = msg_send![cls, processInfo];
        if info.is_null() {
            return 0;
        }
        let v: OsVersion = msg_send![info, operatingSystemVersion];
        let _ = (v.minor, v.patch);
        v.major as i64
    }
}

unsafe fn in_status_bar_window(e: *mut AnyObject) -> bool {
    let win: *mut AnyObject = msg_send![e, window];
    if win.is_null() {
        return false;
    }
    let c: *const objc2::runtime::AnyClass = msg_send![win, class];
    (*c).name().to_bytes() == b"NSStatusBarWindow"
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_macos_major_version() {
        // 구조체 반환 ABI 가 어긋나면 0 이나 쓰레기 값이 나온다 — 보정이 조용히 꺼지거나 26 이하에 켜진다.
        let out = std::process::Command::new("sw_vers").arg("-productVersion").output().unwrap();
        let want: i64 = String::from_utf8_lossy(&out.stdout).split('.').next().unwrap().trim().parse().unwrap();
        assert_eq!(super::macos_major(), want);
    }
}
