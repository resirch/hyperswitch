use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows::Win32::UI::WindowsAndMessaging::{
    ClipCursor, SetWindowPos, ShowCursor, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_SHOWWINDOW,
};

/// Release cursor clipping/capture so the mouse works over fullscreen games
/// while the switcher overlay is visible.
pub unsafe fn unlock_cursor() {
    let _ = ClipCursor(None);
    let _ = ReleaseCapture();
    // Games often hide the cursor; force it visible while the switcher is open.
    let mut count = ShowCursor(true);
    while count < 0 {
        count = ShowCursor(true);
    }
}

/// Keep the overlay above borderless and many exclusive-fullscreen windows.
pub unsafe fn raise_overlay(hwnd: HWND) {
    let _ = SetWindowPos(
        hwnd,
        Some(HWND_TOPMOST),
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
    // Re-assert topmost after any competing fullscreen HWND may have claimed Z-order.
    let _ = SetWindowPos(
        hwnd,
        Some(HWND_TOPMOST),
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    );
}