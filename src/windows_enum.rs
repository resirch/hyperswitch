use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, TRUE, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{
    MonitorFromPoint, MonitorFromWindow, HMONITOR, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::{
    SHGetFileInfoW, SHGetImageList, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SYSICONINDEX,
    SHIL_EXTRALARGE, SHIL_JUMBO,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// One enumerated, alt-tab-able top-level window.
#[derive(Clone)]
pub struct WindowInfo {
    pub hwnd: HWND,
    pub title: String,
    pub icon: HICON,
}

/// Enumerate all alt-tab-able windows in top-to-bottom Z-order.
pub fn enumerate_windows() -> Vec<WindowInfo> {
    let mut list: Vec<WindowInfo> = Vec::new();
    let ptr = &mut list as *mut Vec<WindowInfo> as isize;
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(ptr));
    }
    list
}

extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        if is_alt_tab_window(hwnd) {
            let list = &mut *(lparam.0 as *mut Vec<WindowInfo>);
            let title = get_window_title(hwnd);
            if !title.is_empty() {
                let icon = get_window_icon(hwnd);
                list.push(WindowInfo { hwnd, title, icon });
            }
        }
    }
    TRUE
}

/// Standard "is this in the Alt+Tab list" test plus DWM-cloak filtering.
unsafe fn is_alt_tab_window(hwnd: HWND) -> bool {
    if !IsWindowVisible(hwnd).as_bool() {
        return false;
    }

    // Resolve to the root owner, then to its last visible popup. If that is
    // not this window, it isn't the alt-tab representative.
    let mut walk = GetAncestor(hwnd, GA_ROOTOWNER);
    loop {
        let try_hwnd = last_active_popup(walk);
        if try_hwnd == walk {
            break;
        }
        if IsWindowVisible(try_hwnd).as_bool() {
            break;
        }
        walk = try_hwnd;
    }
    if walk != hwnd {
        return false;
    }

    let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
        return false;
    }

    if is_cloaked(hwnd) {
        return false;
    }

    true
}

unsafe fn last_active_popup(hwnd: HWND) -> HWND {
    let popup = GetLastActivePopup(hwnd);
    if popup.0.is_null() {
        hwnd
    } else {
        popup
    }
}

unsafe fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    let res = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut _,
        std::mem::size_of::<u32>() as u32,
    );
    res.is_ok() && cloaked != 0
}

/// Read a window's title via GetWindowTextW.
pub fn get_window_title(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let copied = GetWindowTextW(hwnd, &mut buf);
        if copied <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..copied as usize])
    }
}

/// Best-effort icon retrieval. Prefers a high-resolution source (jumbo/extra
/// large system image-list icon for the owning exe) so the icon stays sharp
/// when drawn scaled down. Falls back to the window's own icon, the class icon,
/// and finally the standard large exe icon.
pub fn get_window_icon(hwnd: HWND) -> HICON {
    unsafe {
        // 1. High-res icon from the system image list (256px jumbo / 48px
        //    extra-large), downscaled cleanly by DrawIconEx.
        if let Some(icon) = jumbo_exe_icon(hwnd) {
            return icon;
        }

        // 2. The window's own icon (big, then small variants).
        for icon_type in [ICON_BIG, ICON_SMALL2, ICON_SMALL] {
            let mut result: usize = 0;
            let _ = SendMessageTimeoutW(
                hwnd,
                WM_GETICON,
                WPARAM(icon_type as usize),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                120,
                Some(&mut result as *mut usize),
            );
            if result != 0 {
                return HICON(result as *mut _);
            }
        }

        // 3. The window class icon.
        for idx in [GCLP_HICON, GCLP_HICONSM] {
            let handle = GetClassLongPtrW(hwnd, idx);
            if handle != 0 {
                return HICON(handle as *mut _);
            }
        }

        // 4. The standard large process executable icon.
        if let Some(icon) = exe_icon(hwnd) {
            return icon;
        }

        HICON::default()
    }
}

const ICON_SMALL: i32 = 0;
const ICON_BIG: i32 = 1;
const ICON_SMALL2: i32 = 2;

/// Resolve the owning process's full image path into `buf`, returning its
/// length in UTF-16 code units (0 on failure).
unsafe fn process_image_path(hwnd: HWND, buf: &mut [u16]) -> u32 {
    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 {
        return 0;
    }
    let process = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
        Ok(p) => p,
        Err(_) => return 0,
    };
    let mut size = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(
        process,
        PROCESS_NAME_FORMAT(0),
        PWSTR(buf.as_mut_ptr()),
        &mut size,
    );
    let _ = CloseHandle(process);
    if ok.is_err() {
        0
    } else {
        size
    }
}

/// Fetch the largest available icon for the window's exe from the shell system
/// image list (jumbo 256px, then extra-large 48px).
unsafe fn jumbo_exe_icon(hwnd: HWND) -> Option<HICON> {
    let mut buf = [0u16; 1024];
    if process_image_path(hwnd, &mut buf) == 0 {
        return None;
    }

    let mut info = SHFILEINFOW::default();
    let res = SHGetFileInfoW(
        PWSTR(buf.as_mut_ptr()),
        FILE_FLAGS_AND_ATTRIBUTES(0),
        Some(&mut info),
        std::mem::size_of::<SHFILEINFOW>() as u32,
        SHGFI_SYSICONINDEX,
    );
    if res == 0 {
        return None;
    }
    let index = info.iIcon;

    for shil in [SHIL_JUMBO, SHIL_EXTRALARGE] {
        if let Ok(list) = SHGetImageList::<IImageList>(shil as i32) {
            if let Ok(icon) = list.GetIcon(index, ILD_TRANSPARENT.0) {
                if !icon.0.is_null() {
                    return Some(icon);
                }
            }
        }
    }
    None
}

unsafe fn exe_icon(hwnd: HWND) -> Option<HICON> {
    let mut buf = [0u16; 1024];
    if process_image_path(hwnd, &mut buf) == 0 {
        return None;
    }

    let mut info = SHFILEINFOW::default();
    let res = SHGetFileInfoW(
        PWSTR(buf.as_mut_ptr()),
        FILE_FLAGS_AND_ATTRIBUTES(0),
        Some(&mut info),
        std::mem::size_of::<SHFILEINFOW>() as u32,
        SHGFI_ICON | SHGFI_LARGEICON,
    );
    if res != 0 && !info.hIcon.0.is_null() {
        Some(info.hIcon)
    } else {
        None
    }
}

/// Ask a window to close gracefully (posts WM_CLOSE so the app can run its own
/// shutdown / save prompts).
pub fn close_window(hwnd: HWND) {
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

/// The monitor currently containing the mouse cursor.
pub fn cursor_monitor() -> HMONITOR {
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
    }
}

/// The monitor that a window is (mostly) on.
pub fn window_monitor(hwnd: HWND) -> HMONITOR {
    unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) }
}
