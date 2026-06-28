use std::cell::Cell;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::state::{self, AppState};
use crate::windows_enum::{close_window, cursor_monitor, enumerate_windows, window_monitor};

thread_local! {
    static HOOK: Cell<HHOOK> = const { Cell::new(HHOOK(std::ptr::null_mut())) };
    static MOUSE_HOOK: Cell<HHOOK> = const { Cell::new(HHOOK(std::ptr::null_mut())) };
}

/// Install the low-level keyboard and mouse hooks on the current thread.
pub fn install() -> windows::core::Result<()> {
    unsafe {
        let h = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0)?;
        HOOK.with(|c| c.set(h));
        let m = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0)?;
        MOUSE_HOOK.with(|c| c.set(m));
        Ok(())
    }
}

/// Remove the hooks (best effort).
pub fn uninstall() {
    for cell in [&HOOK, &MOUSE_HOOK] {
        cell.with(|c| {
            let h = c.get();
            if !h.0.is_null() {
                unsafe {
                    let _ = UnhookWindowsHookEx(h);
                }
                c.set(HHOOK(std::ptr::null_mut()));
            }
        });
    }
}

extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 {
            let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let vk = kb.vkCode;
            let msg = wparam.0 as u32;
            let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
            let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

            if (is_down || is_up) && handle_key(vk, is_down, is_up) {
                return LRESULT(1);
            }
        }
        CallNextHookEx(None, code, wparam, lparam)
    }
}

extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 {
            let ms = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            if handle_mouse(wparam.0 as u32, ms.pt) {
                return LRESULT(1);
            }
        }
        CallNextHookEx(None, code, wparam, lparam)
    }
}

/// Mouse handling while the overlay is visible. Returns true to swallow the
/// event. Clicking outside the icon row cancels the switch; left-clicking an
/// icon selects and commits it; middle-clicking an icon closes that window.
fn handle_mouse(msg: u32, pt: POINT) -> bool {
    state::with(|st| {
        if !st.visible {
            return false;
        }
        let overlay = st.overlay;
        let hit = crate::overlay::item_index_at(
            overlay,
            st.config.icon_size,
            st.windows.len(),
            pt,
        );

        match msg {
            WM_LBUTTONDOWN => {
                if let Some(idx) = hit {
                    st.selected = idx;
                    st.visible = false;
                    post(overlay, state::WM_HS_COMMIT);
                } else {
                    st.visible = false;
                    post(overlay, state::WM_HS_HIDE);
                }
                true
            }
            WM_RBUTTONDOWN => {
                // Any right click cancels.
                st.visible = false;
                post(overlay, state::WM_HS_HIDE);
                true
            }
            WM_MOUSEMOVE => {
                // Hover-to-select. Never swallow movement so the cursor keeps
                // moving normally; commit still happens on modifier release.
                if let Some(idx) = hit {
                    if st.selected != idx {
                        st.selected = idx;
                        post(overlay, state::WM_HS_UPDATE);
                    }
                }
                false
            }
            WM_MBUTTONDOWN => {
                if let Some(idx) = hit {
                    let target = st.windows[idx].hwnd;
                    close_window(target);
                    st.windows.remove(idx);
                    st.cached_order.retain(|h| h.0 != target.0);
                    if st.windows.is_empty() {
                        st.visible = false;
                        post(overlay, state::WM_HS_HIDE);
                    } else {
                        if st.selected > idx {
                            st.selected -= 1;
                        }
                        if st.selected >= st.windows.len() {
                            st.selected = st.windows.len() - 1;
                        }
                        // Re-show to resize for the new count and repaint.
                        post(overlay, state::WM_HS_SHOW);
                    }
                } else {
                    st.visible = false;
                    post(overlay, state::WM_HS_HIDE);
                }
                true
            }
            _ => false,
        }
    })
    .unwrap_or(false)
}

/// Core state machine. Returns true if the key should be swallowed.
fn handle_key(vk: u32, is_down: bool, is_up: bool) -> bool {
    state::with(|st| {
        // Modifier tracking must reflect this event before any decision.
        update_modifier(st, vk, is_down);

        let overlay = st.overlay;
        let cycle = st.config.cycle_vk().0 as u32;

        // Cycle / activate key.
        if is_down && vk == cycle {
            if !st.visible {
                if st.all_hold_mods_down() {
                    let fresh = enumerate_windows();
                    let ordered = st.reconcile(fresh);
                    st.windows = if st.config.current_monitor_only {
                        let cm = cursor_monitor();
                        ordered
                            .into_iter()
                            .filter(|w| window_monitor(w.hwnd).0 == cm.0)
                            .collect()
                    } else {
                        ordered
                    };
                    // Always surface the currently focused window first, without
                    // disturbing the stable order of the rest (cached_order is
                    // left untouched).
                    let fg = unsafe { GetForegroundWindow() };
                    if let Some(pos) = st.windows.iter().position(|w| w.hwnd.0 == fg.0) {
                        if pos != 0 {
                            let item = st.windows.remove(pos);
                            st.windows.insert(0, item);
                        }
                    }
                    if !st.windows.is_empty() {
                        st.selected = if st.windows.len() > 1 { 1 } else { 0 };
                        st.visible = true;
                        post(overlay, state::WM_HS_SHOW);
                        return true;
                    }
                }
                return false;
            } else {
                if st.reverse_held() {
                    st.select_prev();
                } else {
                    st.select_next();
                }
                post(overlay, state::WM_HS_UPDATE);
                return true;
            }
        }

        // Navigation / cancel while visible.
        if st.visible && is_down {
            let key = VIRTUAL_KEY(vk as u16);
            let mut swallow = true;
            match key {
                VK_TAB => {
                    if st.shift() {
                        st.select_prev();
                    } else {
                        st.select_next();
                    }
                    post(overlay, state::WM_HS_UPDATE);
                }
                VK_RIGHT | VK_DOWN => {
                    st.select_next();
                    post(overlay, state::WM_HS_UPDATE);
                }
                VK_LEFT | VK_UP => {
                    st.select_prev();
                    post(overlay, state::WM_HS_UPDATE);
                }
                VK_ESCAPE => {
                    st.visible = false;
                    post(overlay, state::WM_HS_HIDE);
                }
                _ => swallow = false,
            }
            if swallow {
                return true;
            }
        }

        // Another shortcut while the overlay is open (e.g. hyper+Space for an app
        // launcher): cancel without committing and let the key through.
        if st.visible && is_down && should_cancel_on_other_key(st, vk, cycle) {
            st.visible = false;
            post(overlay, state::WM_HS_HIDE);
            return false;
        }

        // Commit on hold-modifier release: the core fix.
        if is_up && st.visible && is_modifier_vk(vk) && !st.any_hold_mod_held() {
            st.visible = false;
            post(overlay, state::WM_HS_COMMIT);
        }

        false
    })
    .unwrap_or(false)
}

fn post(hwnd: HWND, msg: u32) {
    unsafe {
        let _ = PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0));
    }
}

fn update_modifier(st: &mut AppState, vk: u32, is_down: bool) {
    let key = VIRTUAL_KEY(vk as u16);
    match key {
        VK_LCONTROL => st.lctrl = is_down,
        VK_RCONTROL => st.rctrl = is_down,
        VK_CONTROL => {
            st.lctrl = is_down;
            st.rctrl = is_down;
        }
        VK_LMENU => st.lalt = is_down,
        VK_RMENU => st.ralt = is_down,
        VK_MENU => {
            st.lalt = is_down;
            st.ralt = is_down;
        }
        VK_LWIN => st.lwin = is_down,
        VK_RWIN => st.rwin = is_down,
        VK_LSHIFT => st.lshift = is_down,
        VK_RSHIFT => st.rshift = is_down,
        VK_SHIFT => {
            st.lshift = is_down;
            st.rshift = is_down;
        }
        _ => {}
    }
}

fn is_modifier_vk(vk: u32) -> bool {
    matches!(
        VIRTUAL_KEY(vk as u16),
        VK_LCONTROL
            | VK_RCONTROL
            | VK_CONTROL
            | VK_LMENU
            | VK_RMENU
            | VK_MENU
            | VK_LWIN
            | VK_RWIN
            | VK_LSHIFT
            | VK_RSHIFT
            | VK_SHIFT
    )
}

/// True when a key down should dismiss the overlay without committing so another
/// hyperkey chord can run (cycle/navigation/reverse-modifier keys are excluded).
fn should_cancel_on_other_key(st: &AppState, vk: u32, cycle: u32) -> bool {
    if vk == cycle || is_modifier_vk(vk) || is_reverse_modifier_vk(st, vk) {
        return false;
    }
    !matches!(
        VIRTUAL_KEY(vk as u16),
        VK_TAB | VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_ESCAPE
    )
}

fn is_reverse_modifier_vk(st: &AppState, vk: u32) -> bool {
    let key = VIRTUAL_KEY(vk as u16);
    match st
        .config
        .reverse_modifier
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "ctrl" | "control" => matches!(key, VK_CONTROL | VK_LCONTROL | VK_RCONTROL),
        "alt" | "menu" => matches!(key, VK_MENU | VK_LMENU | VK_RMENU),
        "win" | "windows" | "super" => matches!(key, VK_LWIN | VK_RWIN),
        "shift" => matches!(key, VK_SHIFT | VK_LSHIFT | VK_RSHIFT),
        _ => false,
    }
}
