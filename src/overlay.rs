use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::focus::focus_window;
use crate::icon_draw;
use crate::state::{self, AppState};

// Layout constants (logical pixels).
const MARGIN: i32 = 24;
const ICON_GAP: i32 = 18;
const TITLE_HEIGHT: i32 = 44;
const HILITE_PAD: i32 = 8;

pub const CLASS_NAME: PCWSTR = w!("HyperswitchOverlay");

/// Register the overlay window class. Called once at startup.
pub fn register_class() -> windows::core::Result<()> {
    unsafe {
        let hinstance = GetModuleHandleW(None)?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: CLASS_NAME,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: crate::load_app_icon(
                GetSystemMetrics(SM_CXICON),
                GetSystemMetrics(SM_CYICON),
            ),
            style: CS_HREDRAW | CS_VREDRAW,
            ..Default::default()
        };
        let atom = RegisterClassW(&wc);
        if atom == 0 {
            return Err(windows::core::Error::from_thread());
        }
        Ok(())
    }
}

/// Create the (initially hidden) overlay window.
pub fn create_overlay() -> windows::core::Result<HWND> {
    unsafe {
        let hinstance = GetModuleHandleW(None)?;
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
            CLASS_NAME,
            w!("Hyperswitch"),
            WS_POPUP,
            0,
            0,
            200,
            200,
            None,
            None,
            Some(hinstance.into()),
            None,
        )?;

        // Win11 rounded corners.
        let pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        );

        Ok(hwnd)
    }
}

/// Overlay pixel size for the current window count. Height drops the title
/// strip when titles are hidden so there is no empty gap below the icons.
fn content_size(st: &AppState) -> (i32, i32) {
    let n = st.windows.len() as i32;
    let icon = st.config.icon_size;
    let width = MARGIN * 2 + n * icon + (n - 1) * ICON_GAP;
    let height = if st.config.show_title {
        MARGIN * 2 + icon + TITLE_HEIGHT
    } else {
        MARGIN * 2 + icon
    };
    (width, height)
}

/// Position and size the overlay for the current window count.
/// Pixel content is supplied separately by `render` via `UpdateLayeredWindow`.
fn position_overlay(hwnd: HWND, st: &AppState) {
    unsafe {
        if st.windows.is_empty() {
            return;
        }
        let (width, height) = content_size(st);

        // Center on the monitor under the cursor (nicer multi-monitor UX).
        let area = cursor_monitor_work_area();
        let x = area.left + (area.right - area.left - width) / 2;
        let y = area.top + (area.bottom - area.top - height) / 2;

        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE,
        );
    }
}

/// Hit-test a screen point against the icon row. Returns the index of the item
/// under the point, if any. `icon` is the configured icon edge length and
/// `count` the number of windows currently shown. Does not touch shared state,
/// so it is safe to call from within a `state::with` closure.
pub fn item_index_at(hwnd: HWND, icon: i32, count: usize, pt: POINT) -> Option<usize> {
    unsafe {
        let mut rc = RECT::default();
        if GetWindowRect(hwnd, &mut rc).is_err() {
            return None;
        }
        let cx = pt.x - rc.left;
        let cy = pt.y - rc.top;
        let top = MARGIN;
        if cy < top - HILITE_PAD || cy > top + icon + HILITE_PAD {
            return None;
        }
        for i in 0..count {
            let x = MARGIN + i as i32 * (icon + ICON_GAP);
            if cx >= x - HILITE_PAD && cx <= x + icon + HILITE_PAD {
                return Some(i);
            }
        }
        None
    }
}

/// Work area (excludes taskbar) of the monitor under the mouse cursor.
fn cursor_monitor_work_area() -> RECT {
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(hmon, &mut mi).as_bool() {
            mi.rcWork
        } else {
            RECT {
                left: 0,
                top: 0,
                right: GetSystemMetrics(SM_CXSCREEN),
                bottom: GetSystemMetrics(SM_CYSCREEN),
            }
        }
    }
}

/// Rebuild the overlay's pixels and push them to the screen via
/// `UpdateLayeredWindow`. The background keeps the configured translucency;
/// app icons are drawn with Lanczos downscaling for smooth anti-aliased edges.
fn render(hwnd: HWND) {
    state::with(|st| unsafe {
        if !st.windows.is_empty() {
            draw_layered(hwnd, st);
        }
    });
}

unsafe fn draw_layered(hwnd: HWND, st: &AppState) {
    let mut rc = RECT::default();
    if GetWindowRect(hwnd, &mut rc).is_err() {
        return;
    }
    let width = rc.right - rc.left;
    let height = rc.bottom - rc.top;
    if width <= 0 || height <= 0 {
        return;
    }

    let screen = GetDC(None);
    let mem_dc = CreateCompatibleDC(Some(screen));

    // Top-down 32-bit DIB section we can both GDI-draw into and edit per pixel.
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib = match CreateDIBSection(Some(mem_dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
        Ok(b) if !bits.is_null() => b,
        _ => {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(None, screen);
            return;
        }
    };
    let old = SelectObject(mem_dc, dib.into());
    SetStretchBltMode(mem_dc, HALFTONE);
    let _ = SetBrushOrgEx(mem_dc, 0, 0, None);

    std::ptr::write_bytes(bits as *mut u8, 0, (width as usize) * (height as usize) * 4);

    let icon = st.config.icon_size;
    let top = MARGIN;

    // Chrome (background, selection highlight, title) drawn opaque; GDI leaves
    // the alpha byte at 0, which the fixup pass below turns into translucency.
    let full = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    let bg = CreateSolidBrush(COLORREF(0x00202020));
    FillRect(mem_dc, &full, bg);
    let _ = DeleteObject(bg.into());

    if st.selected < st.windows.len() {
        let x = MARGIN + st.selected as i32 * (icon + ICON_GAP);
        let hl = RECT {
            left: x - HILITE_PAD,
            top: top - HILITE_PAD,
            right: x + icon + HILITE_PAD,
            bottom: top + icon + HILITE_PAD,
        };
        let hilite = CreateSolidBrush(COLORREF(0x00603C2E));
        let pen = CreatePen(PS_SOLID, 1, COLORREF(0x00C08050));
        let old_pen = SelectObject(mem_dc, pen.into());
        let old_brush = SelectObject(mem_dc, hilite.into());
        let _ = RoundRect(mem_dc, hl.left, hl.top, hl.right, hl.bottom, 12, 12);
        SelectObject(mem_dc, old_brush);
        SelectObject(mem_dc, old_pen);
        let _ = DeleteObject(hilite.into());
        let _ = DeleteObject(pen.into());
    }

    if st.config.show_title {
        if let Some(win) = st.windows.get(st.selected) {
            let mut title: Vec<u16> = win.title.encode_utf16().collect();
            title.push(0);
            let mut text_rc = RECT {
                left: MARGIN,
                top: MARGIN + icon + 6,
                right: width - MARGIN,
                bottom: height - 6,
            };
            SetBkMode(mem_dc, TRANSPARENT);
            SetTextColor(mem_dc, COLORREF(0x00F0F0F0));
            let _ = DrawTextW(
                mem_dc,
                &mut title,
                &mut text_rc,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }
    }

    let _ = GdiFlush();

    // Give all chrome pixels the configured opacity and premultiply their RGB,
    // producing a valid premultiplied-ARGB translucent background.
    let opacity = st.config.opacity as u32;
    let pixels = (width as usize) * (height as usize);
    let px = std::slice::from_raw_parts_mut(bits as *mut u32, pixels);
    for p in px.iter_mut() {
        let v = *p;
        let b = (v & 0xff) * opacity / 255;
        let g = ((v >> 8) & 0xff) * opacity / 255;
        let r = ((v >> 16) & 0xff) * opacity / 255;
        *p = (opacity << 24) | (r << 16) | (g << 8) | b;
    }

    // Draw icons with Lanczos downscale and proper alpha compositing so edges
    // stay smooth on the premultiplied translucent background.
    for (i, win) in st.windows.iter().enumerate() {
        if !win.icon.0.is_null() {
            let x = MARGIN + i as i32 * (icon + ICON_GAP);
            icon_draw::draw_icon_smooth(px, width, height, x, top, win.icon, icon);
        }
    }

    let _ = GdiFlush();

    let pt_src = POINT { x: 0, y: 0 };
    let pt_dst = POINT {
        x: rc.left,
        y: rc.top,
    };
    let size = SIZE {
        cx: width,
        cy: height,
    };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let _ = UpdateLayeredWindow(
        hwnd,
        Some(screen),
        Some(&pt_dst),
        Some(&size),
        Some(mem_dc),
        Some(&pt_src),
        COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    );

    SelectObject(mem_dc, old);
    let _ = DeleteObject(dib.into());
    let _ = DeleteDC(mem_dc);
    ReleaseDC(None, screen);
}

pub extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                // Content is supplied via UpdateLayeredWindow; just validate.
                let _ = ValidateRect(Some(hwnd), None);
                LRESULT(0)
            }
            state::WM_HS_SHOW => {
                // Size, paint, then show so the user never sees a stale frame.
                state::with(|st| position_overlay(hwnd, st));
                render(hwnd);
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                LRESULT(0)
            }
            state::WM_HS_UPDATE => {
                render(hwnd);
                LRESULT(0)
            }
            state::WM_HS_HIDE => {
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            state::WM_HS_COMMIT => {
                let target = state::with(|st| {
                    st.windows.get(st.selected).map(|w| w.hwnd)
                });
                let _ = ShowWindow(hwnd, SW_HIDE);
                if let Some(Some(target)) = target {
                    focus_window(target);
                }
                LRESULT(0)
            }
            state::WM_HS_TRAY => {
                // Right button release or context-menu key -> show menu.
                let event = (lparam.0 & 0xFFFF) as u32;
                if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                    crate::show_tray_menu(hwnd);
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as u32;
                crate::handle_command(hwnd, id);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
