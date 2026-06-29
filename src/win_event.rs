use std::cell::Cell;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT};

use crate::state;
use crate::windows_enum;

thread_local! {
    static HOOK: Cell<Option<HWINEVENTHOOK>> = const { Cell::new(None) };
}

/// Track foreground changes so the switcher can sort by recency.
pub fn install() -> windows::core::Result<()> {
    unsafe {
        let h = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        if h.is_invalid() {
            return Err(windows::core::Error::from_thread());
        }
        HOOK.with(|c| c.set(Some(h)));
        Ok(())
    }
}

pub fn uninstall() {
    HOOK.with(|c| {
        if let Some(h) = c.get() {
            unsafe {
                let _ = UnhookWinEvent(h);
            }
            c.set(None);
        }
    });
}

extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    if event == EVENT_SYSTEM_FOREGROUND && !hwnd.0.is_null() {
        if windows_enum::is_switchable_window(hwnd) {
            state::with(|st| st.touch_recent(hwnd));
        }
    }
}
