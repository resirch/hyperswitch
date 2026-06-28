use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, SetActiveWindow, SetFocus, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, SetForegroundWindow,
    ShowWindow, SW_RESTORE,
};

/// Reliably bring `target` to the foreground, working around Windows'
/// foreground-lock restrictions by attaching to the current foreground thread
/// and nudging the ALT key.
pub fn focus_window(target: HWND) {
    if target.0.is_null() {
        return;
    }

    unsafe {
        // Restore if minimized.
        if IsIconic(target).as_bool() {
            let _ = ShowWindow(target, SW_RESTORE);
        }

        let foreground = GetForegroundWindow();
        let our_thread = GetCurrentThreadId();
        let fg_thread = if foreground.0.is_null() {
            0
        } else {
            GetWindowThreadProcessId(foreground, None)
        };
        let target_thread = GetWindowThreadProcessId(target, None);

        // Synthetic ALT tap relaxes the foreground lock for this call.
        keybd_event(VK_MENU.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);

        let attached_fg = fg_thread != 0 && fg_thread != our_thread;
        let attached_tg = target_thread != 0 && target_thread != our_thread && target_thread != fg_thread;

        if attached_fg {
            let _ = AttachThreadInput(our_thread, fg_thread, true);
        }
        if attached_tg {
            let _ = AttachThreadInput(our_thread, target_thread, true);
        }

        let _ = BringWindowToTop(target);
        let _ = SetForegroundWindow(target);
        let _ = SetActiveWindow(target);
        let _ = SetFocus(Some(target));

        if attached_tg {
            let _ = AttachThreadInput(our_thread, target_thread, false);
        }
        if attached_fg {
            let _ = AttachThreadInput(our_thread, fg_thread, false);
        }
    }
}
