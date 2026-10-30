use std::cell::Cell;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, EVENT_OBJECT_DESTROY, EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT,
};

use crate::state;
use crate::windows_enum;

thread_local! {
    static HOOKS: Cell<Vec<HWINEVENTHOOK>> = const { Cell::new(Vec::new()) };
}

/// Track foreground changes and window closes so the cached list stays current.
pub fn install() -> windows::core::Result<()> {
    unsafe {
        let hooks = [
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(win_event_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            ),
            SetWinEventHook(
                EVENT_OBJECT_DESTROY,
                EVENT_OBJECT_DESTROY,
                None,
                Some(win_event_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            ),
        ];
        let mut installed = Vec::with_capacity(hooks.len());
        for h in hooks {
            if h.is_invalid() {
                for prev in installed {
                    let _ = UnhookWinEvent(prev);
                }
                return Err(windows::core::Error::from_thread());
            }
            installed.push(h);
        }
        HOOKS.with(|c| c.set(installed));
        Ok(())
    }
}

pub fn uninstall() {
    HOOKS.with(|c| {
        for h in c.take() {
            unsafe {
                let _ = UnhookWinEvent(h);
            }
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
    if hwnd.0.is_null() {
        return;
    }

    match event {
        EVENT_SYSTEM_FOREGROUND => {
            if !windows_enum::is_switchable_window(hwnd) {
                return;
            }
            let _ = state::try_with(|st| {
                st.touch_recent(hwnd);
                st.refresh_all_windows();
                if st.visible {
                    if st.selected >= st.windows.len() && !st.windows.is_empty() {
                        st.selected = st.windows.len() - 1;
                    }
                    unsafe {
                        let _ = PostMessageW(
                            Some(st.overlay),
                            state::WM_HS_UPDATE,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    }
                }
            });
        }
        EVENT_OBJECT_DESTROY => {
            // Drop closed windows from the cache without a full re-enumerate
            // (refresh during destroy events caused re-entrant crashes).
            let _ = state::try_with(|st| {
                if let Some(pos) = st.all_windows.iter().position(|w| w.hwnd.0 == hwnd.0) {
                    let removed = st.all_windows.remove(pos);
                    windows_enum::free_window_icons(std::slice::from_ref(&removed));
                }
                st.recent_hwnds.retain(|h| h.0 != hwnd.0);
                if st.visible {
                    st.apply_display_filter();
                    if st.windows.is_empty() {
                        st.visible = false;
                        unsafe {
                            let _ = PostMessageW(
                                Some(st.overlay),
                                state::WM_HS_HIDE,
                                WPARAM(0),
                                LPARAM(0),
                            );
                        }
                    } else {
                        if st.selected >= st.windows.len() {
                            st.selected = st.windows.len() - 1;
                        }
                        unsafe {
                            let _ = PostMessageW(
                                Some(st.overlay),
                                state::WM_HS_UPDATE,
                                WPARAM(0),
                                LPARAM(0),
                            );
                        }
                    }
                }
            });
        }
        _ => {}
    }
}
