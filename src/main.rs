#![windows_subsystem = "windows"]

mod config;
mod focus;
mod hook;
mod icon_draw;
mod overlay;
mod startup;
mod state;
mod windows_enum;
mod win_event;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use config::Config;
use state::AppState;

const TRAY_UID: u32 = 1;
const ID_TRAY_RELOAD: u32 = 1001;
const ID_TRAY_EXIT: u32 = 1002;
const ID_TRAY_SHOW_TITLE: u32 = 1003;
const ID_TRAY_CURRENT_MONITOR: u32 = 1004;
const ID_TRAY_STARTUP: u32 = 1005;

fn main() {
    unsafe {
        // Single instance guard.
        let _mutex = CreateMutexW(None, true, w!("Global\\HyperswitchSingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }

        let config = Config::load_or_create();
        let _ = startup::sync(config.run_on_startup);

        if overlay::register_class().is_err() {
            return;
        }
        let hwnd = match overlay::create_overlay() {
            Ok(h) => h,
            Err(_) => return,
        };
        set_window_icons(hwnd);

        state::init(AppState {
            overlay: hwnd,
            visible: false,
            all_windows: Vec::new(),
            windows: Vec::new(),
            selected: 0,
            config,
            recent_hwnds: Vec::new(),
            lctrl: false,
            rctrl: false,
            lalt: false,
            ralt: false,
            lwin: false,
            rwin: false,
            lshift: false,
            rshift: false,
        });

        if hook::install().is_err() {
            return;
        }
        if win_event::install().is_err() {
            return;
        }

        // Seed MRU and pre-build the sorted window list.
        let fg = GetForegroundWindow();
        state::with(|st| {
            if windows_enum::is_switchable_window(fg) {
                st.touch_recent(fg);
            }
            st.refresh_all_windows();
        });

        add_tray_icon(hwnd);

        // Standard message loop.
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Cleanup.
        remove_tray_icon(hwnd);
        hook::uninstall();
        win_event::uninstall();
    }
}

/// Resource id of the embedded application icon (see build.rs).
const APP_ICON_ID: PCWSTR = PCWSTR(1 as *const u16);

/// Load the embedded application icon (resource id 1) at the requested size.
/// Falls back to the stock application icon if the resource is missing.
pub fn load_app_icon(cx: i32, cy: i32) -> HICON {
    unsafe {
        let hinstance = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(_) => return LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
        };

        let try_image = |w: i32, h: i32, flags: IMAGE_FLAGS| -> Option<HICON> {
            LoadImageW(
                Some(hinstance.into()),
                APP_ICON_ID,
                IMAGE_ICON,
                w,
                h,
                flags,
            )
            .ok()
            .filter(|handle| !handle.0.is_null())
            .map(|handle| HICON(handle.0))
        };

        if cx > 0 && cy > 0 {
            if let Some(icon) = try_image(cx, cy, LR_DEFAULTCOLOR) {
                return icon;
            }
        }

        if let Some(icon) = try_image(0, 0, LR_DEFAULTCOLOR | LR_DEFAULTSIZE) {
            return icon;
        }

        for size in [256i32, 128, 64, 48, 32, 16] {
            if let Some(icon) = try_image(size, size, LR_DEFAULTCOLOR) {
                return icon;
            }
        }

        if let Ok(icon) = LoadIconW(Some(hinstance.into()), APP_ICON_ID) {
            if !icon.0.is_null() {
                return icon;
            }
        }

        LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
    }
}

/// Assign large and small icons to a window (Task Manager, Alt+Tab, etc.).
pub fn set_window_icons(hwnd: HWND) {
    unsafe {
        let big = load_app_icon(GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON));
        let small = load_app_icon(
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
        );
        let _ = SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_BIG as usize)),
            Some(LPARAM(big.0 as isize)),
        );
        let _ = SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_SMALL as usize)),
            Some(LPARAM(small.0 as isize)),
        );
    }
}

fn add_tray_icon(hwnd: HWND) {
    unsafe {
        let hicon = load_app_icon(
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
        );
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_UID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: state::WM_HS_TRAY,
            hIcon: hicon,
            ..Default::default()
        };
        let tip = "Hyperswitch";
        for (i, c) in tip.encode_utf16().enumerate() {
            if i < nid.szTip.len() - 1 {
                nid.szTip[i] = c;
            }
        }
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

fn remove_tray_icon(hwnd: HWND) {
    unsafe {
        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_UID,
            ..Default::default()
        };
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

/// Show the tray right-click menu (Reload config / Exit).
pub fn show_tray_menu(hwnd: HWND) {
    unsafe {
        let menu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => return,
        };
        // Reflect current values so the checkmarks are accurate.
        let (show_title, current_monitor_only, run_on_startup) =
            state::with(|st| {
                (
                    st.config.show_title,
                    st.config.current_monitor_only,
                    st.config.run_on_startup,
                )
            })
            .unwrap_or((true, false, false));
        let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };

        let _ = AppendMenuW(
            menu,
            MF_STRING | check(show_title),
            ID_TRAY_SHOW_TITLE as usize,
            w!("Show title"),
        );
        let _ = AppendMenuW(
            menu,
            MF_STRING | check(current_monitor_only),
            ID_TRAY_CURRENT_MONITOR as usize,
            w!("Current monitor only"),
        );
        let _ = AppendMenuW(
            menu,
            MF_STRING | check(run_on_startup),
            ID_TRAY_STARTUP as usize,
            w!("Run on startup"),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_TRAY_RELOAD as usize, w!("Reload config"));
        let _ = AppendMenuW(menu, MF_STRING, ID_TRAY_EXIT as usize, w!("Exit"));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);

        // Required so the menu dismisses correctly for a NOACTIVATE window.
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            Some(0),
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
    }
}

/// Handle a WM_COMMAND from the tray menu.
pub fn handle_command(hwnd: HWND, id: u32) {
    match id {
        ID_TRAY_RELOAD => {
            let cfg = Config::load_or_create();
            let _ = startup::sync(cfg.run_on_startup);
            state::with(|st| st.config = cfg);
        }
        ID_TRAY_SHOW_TITLE => {
            state::with(|st| {
                st.config.show_title = !st.config.show_title;
                let _ = st.config.save();
            });
        }
        ID_TRAY_CURRENT_MONITOR => {
            state::with(|st| {
                st.config.current_monitor_only = !st.config.current_monitor_only;
                let _ = st.config.save();
            });
        }
        ID_TRAY_STARTUP => {
            state::with(|st| {
                st.config.run_on_startup = !st.config.run_on_startup;
                let _ = st.config.save();
                let _ = startup::sync(st.config.run_on_startup);
            });
        }
        ID_TRAY_EXIT => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        _ => {}
    }
}
