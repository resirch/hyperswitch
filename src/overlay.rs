use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::focus::focus_window;
use crate::hook;
use crate::icon_draw;
use crate::input;
use crate::state;
use crate::windows_enum::WindowInfo;

// Base layout constants at 96 DPI (100% scaling).
const DEFAULT_DPI: u32 = 96;
const MARGIN: i32 = 24;
const ICON_GAP: i32 = 18;
const TITLE_HEIGHT: i32 = 44;
const HILITE_PAD: i32 = 8;
const CORNER_RADIUS: i32 = 12;
const TEXT_PAD: i32 = 6;
const TITLE_FONT_PX: i32 = 16;

/// DPI-scaled overlay geometry shared by positioning, drawing, and hit-testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OverlayLayout {
    dpi: u32,
    count: usize,
    margin: i32,
    icon_gap: i32,
    hilite_pad: i32,
    icon_size: i32,
    corner_radius: i32,
    border_width: i32,
    text_pad: i32,
    width: i32,
    height: i32,
}

impl OverlayLayout {
    fn new(logical_icon_size: i32, show_title: bool, count: usize, dpi: u32) -> Self {
        let dpi = if dpi == 0 { DEFAULT_DPI } else { dpi };
        let margin = scale_dpi(MARGIN, dpi);
        let icon_gap = scale_dpi(ICON_GAP, dpi);
        let title_height = scale_dpi(TITLE_HEIGHT, dpi);
        let hilite_pad = scale_dpi(HILITE_PAD, dpi);
        let icon_size = scale_dpi(logical_icon_size, dpi).max(1);
        let corner_radius = scale_dpi(CORNER_RADIUS, dpi);
        let border_width = scale_dpi(1, dpi).max(1);
        let text_pad = scale_dpi(TEXT_PAD, dpi);

        let n = count as i32;
        let width = if n > 0 {
            margin * 2 + n * icon_size + (n - 1) * icon_gap
        } else {
            margin * 2
        };
        let height = if show_title {
            margin * 2 + icon_size + title_height
        } else {
            margin * 2 + icon_size
        };

        Self {
            dpi,
            count,
            margin,
            icon_gap,
            hilite_pad,
            icon_size,
            corner_radius,
            border_width,
            text_pad,
            width,
            height,
        }
    }

    fn item_x(&self, index: usize) -> i32 {
        self.margin + index as i32 * (self.icon_size + self.icon_gap)
    }

    fn item_highlight_rect(&self, index: usize) -> RECT {
        let x = self.item_x(index);
        let top = self.margin;
        RECT {
            left: x - self.hilite_pad,
            top: top - self.hilite_pad,
            right: x + self.icon_size + self.hilite_pad,
            bottom: top + self.icon_size + self.hilite_pad,
        }
    }

    fn hit_test(&self, cx: i32, cy: i32) -> Option<usize> {
        let top = self.margin;
        if cy < top - self.hilite_pad || cy > top + self.icon_size + self.hilite_pad {
            return None;
        }
        for i in 0..self.count {
            let hl = self.item_highlight_rect(i);
            if cx >= hl.left && cx <= hl.right {
                return Some(i);
            }
        }
        None
    }
}

fn scale_dpi(px: i32, dpi: u32) -> i32 {
    ((px as i64 * dpi as i64 + (DEFAULT_DPI as i64 / 2)) / DEFAULT_DPI as i64) as i32
}

/// Watchdog timer active only while the overlay is open. Recovers cleanly if
/// the OS drops a modifier key-up (e.g. UAC, exclusive fullscreen transition,
/// or low-level hook timeout).
const TIMER_MOD_CHECK: usize = 1;
const MOD_CHECK_INTERVAL_MS: u32 = 50;

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

/// Position and size the overlay centered on `hmon` using that monitor's DPI.
/// Pixel content is supplied separately by `render_layered` via `UpdateLayeredWindow`.
fn position_overlay_on_monitor(hwnd: HWND, hmon: HMONITOR) {
    let dpi = monitor_dpi(hmon);
    let layout = state::with(|st| {
        if st.windows.is_empty() {
            None
        } else {
            st.overlay_dpi = dpi;
            Some(OverlayLayout::new(
                st.config.icon_size,
                st.config.show_title,
                st.windows.len(),
                dpi,
            ))
        }
    })
    .flatten();
    let Some(layout) = layout else {
        return;
    };

    unsafe {
        let area = monitor_work_area(hmon);
        let x = area.left + (area.right - area.left - layout.width) / 2;
        let y = area.top + (area.bottom - area.top - layout.height) / 2;

        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            x,
            y,
            layout.width,
            layout.height,
            SWP_NOACTIVATE,
        );
    }
}

/// Keep the overlay on its current monitor and only reposition when window
/// count or DPI changed.
fn reposition_if_needed(hwnd: HWND) {
    let hmon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let dpi = monitor_dpi(hmon);
    let mut rc = RECT::default();
    let have_rect = unsafe { GetWindowRect(hwnd, &mut rc).is_ok() };

    let needs_reposition = state::with(|st| {
        if st.windows.is_empty() {
            return false;
        }
        let layout = OverlayLayout::new(
            st.config.icon_size,
            st.config.show_title,
            st.windows.len(),
            dpi,
        );
        let matches = have_rect
            && (rc.right - rc.left) == layout.width
            && (rc.bottom - rc.top) == layout.height
            && st.overlay_dpi == dpi;
        !matches
    })
    .unwrap_or(false);

    if needs_reposition {
        position_overlay_on_monitor(hwnd, hmon);
    }
}

/// Hit-test a per-monitor-aware screen point against the icon row. Returns the
/// index of the item under the point, if any. `icon` is the configured logical
/// icon edge length, `count` the number of windows currently shown, and `dpi`
/// the monitor DPI used to lay out the overlay. Does not touch shared state,
/// so it is safe to call from within a `state::with` closure.
pub fn item_index_at(
    hwnd: HWND,
    icon: i32,
    count: usize,
    dpi: u32,
    pt: POINT,
) -> Option<usize> {
    unsafe {
        if count == 0 || !IsWindowVisible(hwnd).as_bool() {
            return None;
        }
        let mut rc = RECT::default();
        if GetWindowRect(hwnd, &mut rc).is_err() {
            return None;
        }
        let layout = OverlayLayout::new(icon, false, count, dpi);
        layout.hit_test(pt.x - rc.left, pt.y - rc.top)
    }
}

/// Effective DPI for `hmon`, falling back to standard 96 DPI.
fn monitor_dpi(hmon: HMONITOR) -> u32 {
    unsafe {
        let mut dpi_x = DEFAULT_DPI;
        let mut dpi_y = DEFAULT_DPI;
        if GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).is_ok() && dpi_x > 0 {
            dpi_x
        } else {
            DEFAULT_DPI
        }
    }
}

/// Work area (excludes taskbar) of `hmon` in physical screen coordinates.
fn monitor_work_area(hmon: HMONITOR) -> RECT {
    unsafe {
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

struct RenderSnapshot {
    windows: Vec<WindowInfo>,
    selected: usize,
    icon_size: i32,
    show_title: bool,
    opacity: u8,
    overlay_dpi: u32,
}

/// Rebuild the overlay's pixels and push them to the screen via
/// `UpdateLayeredWindow`. Returns false if the frame could not be painted
/// (usually GDI resource exhaustion after leaking handles).
///
/// Snapshots state first so `STATE` is not borrowed across `draw_layered`
/// (which may invoke cross-thread `SendMessageTimeoutW` for window icons and
/// pump low-level hooks re-entrantly).
fn render_layered(hwnd: HWND) -> bool {
    let snapshot = state::with(|st| {
        if st.windows.is_empty() {
            return None;
        }
        st.rendering = true;
        Some(RenderSnapshot {
            windows: st.windows.clone(),
            selected: st.selected,
            icon_size: st.config.icon_size,
            show_title: st.config.show_title,
            opacity: st.config.opacity,
            overlay_dpi: st.overlay_dpi,
        })
    })
    .flatten();

    let Some(snap) = snapshot else {
        return false;
    };

    let ok = unsafe { draw_layered(hwnd, &snap) };

    let stale = state::with(|st| {
        st.rendering = false;
        std::mem::take(&mut st.stale_icons)
    })
    .unwrap_or_default();
    for hicon in stale {
        unsafe {
            let _ = DestroyIcon(hicon);
        }
    }

    ok
}

unsafe fn draw_layered(hwnd: HWND, snap: &RenderSnapshot) -> bool {
    let mut rc = RECT::default();
    if GetWindowRect(hwnd, &mut rc).is_err() {
        return false;
    }
    let width = rc.right - rc.left;
    let height = rc.bottom - rc.top;
    if width <= 0 || height <= 0 {
        return false;
    }

    let layout = OverlayLayout::new(
        snap.icon_size,
        snap.show_title,
        snap.windows.len(),
        snap.overlay_dpi,
    );

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
            return false;
        }
    };
    let old = SelectObject(mem_dc, dib.into());
    SetStretchBltMode(mem_dc, HALFTONE);
    let _ = SetBrushOrgEx(mem_dc, 0, 0, None);

    std::ptr::write_bytes(bits as *mut u8, 0, (width as usize) * (height as usize) * 4);

    let icon_sz = layout.icon_size;
    let top = layout.margin;

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

    if snap.selected < snap.windows.len() {
        let hl = layout.item_highlight_rect(snap.selected);
        let hilite = CreateSolidBrush(COLORREF(0x00603C2E));
        let pen = CreatePen(PS_SOLID, layout.border_width, COLORREF(0x00C08050));
        let old_pen = SelectObject(mem_dc, pen.into());
        let old_brush = SelectObject(mem_dc, hilite.into());
        let _ = RoundRect(
            mem_dc,
            hl.left,
            hl.top,
            hl.right,
            hl.bottom,
            layout.corner_radius,
            layout.corner_radius,
        );
        SelectObject(mem_dc, old_brush);
        SelectObject(mem_dc, old_pen);
        let _ = DeleteObject(hilite.into());
        let _ = DeleteObject(pen.into());
    }

    if snap.show_title {
        if let Some(win) = snap.windows.get(snap.selected) {
            let mut title: Vec<u16> = win.title.encode_utf16().collect();
            title.push(0);
            let mut text_rc = RECT {
                left: layout.margin,
                top: layout.margin + icon_sz + layout.text_pad,
                right: width - layout.margin,
                bottom: height - layout.text_pad,
            };
            let font = CreateFontW(
                -scale_dpi(TITLE_FONT_PX, layout.dpi),
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                (DEFAULT_PITCH.0 | FF_SWISS.0) as u32,
                w!("Segoe UI"),
            );
            let old_font = if !font.0.is_null() {
                Some(SelectObject(mem_dc, font.into()))
            } else {
                None
            };
            SetBkMode(mem_dc, TRANSPARENT);
            SetTextColor(mem_dc, COLORREF(0x00F0F0F0));
            let _ = DrawTextW(
                mem_dc,
                &mut title,
                &mut text_rc,
                DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
            if let Some(prev) = old_font {
                SelectObject(mem_dc, prev);
                let _ = DeleteObject(font.into());
            }
        }
    }

    let _ = GdiFlush();

    // Give all chrome pixels the configured opacity and premultiply their RGB,
    // producing a valid premultiplied-ARGB translucent background.
    let opacity = snap.opacity as u32;
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
    for (i, win) in snap.windows.iter().enumerate() {
        let hicon = icon_draw::pick_icon_for_draw(win.icon, win.hwnd);
        if !hicon.0.is_null() {
            let x = layout.item_x(i);
            icon_draw::draw_icon_smooth(px, width, height, x, top, hicon, icon_sz);
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
    let ok = UpdateLayeredWindow(
        hwnd,
        Some(screen),
        Some(&pt_dst),
        Some(&size),
        Some(mem_dc),
        Some(&pt_src),
        COLORREF(0),
        Some(&blend),
        ULW_ALPHA,
    )
    .is_ok();

    SelectObject(mem_dc, old);
    let _ = DeleteObject(dib.into());
    let _ = DeleteDC(mem_dc);
    ReleaseDC(None, screen);
    ok
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OverlayCheck {
    Continue,
    Skip,
    Commit,
}

/// Verify the overlay should still be shown and that at least one hold
/// modifier is physically down according to Windows' live key state.
fn check_visible_and_modifiers() -> OverlayCheck {
    state::with(|st| {
        if !st.visible {
            return OverlayCheck::Skip;
        }
        hook::sync_modifiers_from_os(st);
        if !st.any_hold_mod_held() {
            st.pending_commit = st.windows.get(st.selected).map(|w| w.hwnd);
            st.visible = false;
            return OverlayCheck::Commit;
        }
        OverlayCheck::Continue
    })
    .unwrap_or(OverlayCheck::Skip)
}

unsafe fn hide_overlay(hwnd: HWND) {
    let _ = KillTimer(Some(hwnd), TIMER_MOD_CHECK);
    let _ = ShowWindow(hwnd, SW_HIDE);
}

unsafe fn commit_pending(hwnd: HWND) {
    let _ = KillTimer(Some(hwnd), TIMER_MOD_CHECK);
    let target = state::with(|st| {
        st.visible = false;
        st.pending_commit.take()
    })
    .flatten();
    let _ = ShowWindow(hwnd, SW_HIDE);
    if let Some(target) = target {
        focus_window(target);
    }
}

unsafe fn update_visible_overlay(hwnd: HWND) {
    match check_visible_and_modifiers() {
        OverlayCheck::Skip => return,
        OverlayCheck::Commit => {
            commit_pending(hwnd);
            return;
        }
        OverlayCheck::Continue => {}
    }

    let empty = state::with(|st| st.windows.is_empty()).unwrap_or(true);
    if empty {
        state::with(|st| {
            st.pending_commit = None;
            st.visible = false;
        });
        hide_overlay(hwnd);
        return;
    }

    reposition_if_needed(hwnd);
    let ok = render_layered(hwnd);

    match check_visible_and_modifiers() {
        OverlayCheck::Skip => return,
        OverlayCheck::Commit => {
            commit_pending(hwnd);
            return;
        }
        OverlayCheck::Continue => {}
    }

    if !ok {
        state::with(|st| {
            st.pending_commit = None;
            st.visible = false;
        });
        hide_overlay(hwnd);
    } else {
        input::unlock_cursor();
        input::raise_overlay(hwnd);
    }
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
            state::WM_HS_INIT => {
                state::refresh_all_windows_unlocked();
                LRESULT(0)
            }
            state::WM_HS_REFRESH => {
                state::with(|st| st.refresh_pending = false);
                state::refresh_all_windows_unlocked();
                if state::with(|st| st.visible).unwrap_or(false) {
                    update_visible_overlay(hwnd);
                }
                LRESULT(0)
            }
            state::WM_HS_SHOW => {
                match check_visible_and_modifiers() {
                    OverlayCheck::Skip => return LRESULT(0),
                    OverlayCheck::Commit => {
                        commit_pending(hwnd);
                        return LRESULT(0);
                    }
                    OverlayCheck::Continue => {}
                }

                // Refresh live window list off the keyboard hook thread without
                // holding STATE borrowed during enumeration.
                state::refresh_all_windows_unlocked();

                match check_visible_and_modifiers() {
                    OverlayCheck::Skip => return LRESULT(0),
                    OverlayCheck::Commit => {
                        commit_pending(hwnd);
                        return LRESULT(0);
                    }
                    OverlayCheck::Continue => {}
                }

                let empty = state::with(|st| st.windows.is_empty()).unwrap_or(true);
                if empty {
                    state::with(|st| {
                        st.pending_commit = None;
                        st.visible = false;
                    });
                    hide_overlay(hwnd);
                    return LRESULT(0);
                }

                // Size, paint, unlock cursor, raise above fullscreen, then show.
                let hmon = crate::windows_enum::cursor_monitor();
                position_overlay_on_monitor(hwnd, hmon);
                let ok = render_layered(hwnd);

                match check_visible_and_modifiers() {
                    OverlayCheck::Skip => return LRESULT(0),
                    OverlayCheck::Commit => {
                        commit_pending(hwnd);
                        return LRESULT(0);
                    }
                    OverlayCheck::Continue => {}
                }

                if !ok {
                    state::with(|st| {
                        st.pending_commit = None;
                        st.visible = false;
                    });
                    hide_overlay(hwnd);
                } else {
                    input::unlock_cursor();
                    input::raise_overlay(hwnd);
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    let _ = SetTimer(Some(hwnd), TIMER_MOD_CHECK, MOD_CHECK_INTERVAL_MS, None);
                }
                LRESULT(0)
            }
            state::WM_HS_UPDATE => {
                update_visible_overlay(hwnd);
                LRESULT(0)
            }
            state::WM_HS_HIDE => {
                hide_overlay(hwnd);
                LRESULT(0)
            }
            state::WM_HS_COMMIT => {
                commit_pending(hwnd);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == TIMER_MOD_CHECK => {
                match check_visible_and_modifiers() {
                    OverlayCheck::Commit => commit_pending(hwnd),
                    OverlayCheck::Skip => {
                        let _ = KillTimer(Some(hwnd), TIMER_MOD_CHECK);
                    }
                    OverlayCheck::Continue => {}
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
                let _ = KillTimer(Some(hwnd), TIMER_MOD_CHECK);
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_base_metrics_at_96_dpi() {
        let layout = OverlayLayout::new(64, true, 4, 96);
        assert_eq!(layout.margin, 24);
        assert_eq!(layout.icon_gap, 18);
        assert_eq!(layout.icon_size, 64);
        assert_eq!(layout.hilite_pad, 8);
        assert_eq!(layout.width, 24 * 2 + 4 * 64 + 3 * 18);
        assert_eq!(layout.height, 24 * 2 + 64 + 44);
    }

    #[test]
    fn layout_scales_and_hit_tests_accurately_at_144_dpi() {
        let layout = OverlayLayout::new(64, true, 4, 144);
        assert_eq!(layout.margin, 36);
        assert_eq!(layout.icon_gap, 27);
        assert_eq!(layout.icon_size, 96);
        assert_eq!(layout.hilite_pad, 12);
        assert_eq!(layout.width, 36 * 2 + 4 * 96 + 3 * 27);
        assert_eq!(layout.height, 36 * 2 + 96 + 66);

        for i in 0..4 {
            let hl = layout.item_highlight_rect(i);
            let mid_x = (hl.left + hl.right) / 2;
            let mid_y = (hl.top + hl.bottom) / 2;
            assert_eq!(layout.hit_test(mid_x, mid_y), Some(i));
            assert_eq!(layout.hit_test(hl.left, hl.top), Some(i));
            assert_eq!(layout.hit_test(hl.right, hl.bottom), Some(i));
        }

        // Above, below, and in the narrow gap between highlight boxes should miss.
        let first = layout.item_highlight_rect(0);
        let second = layout.item_highlight_rect(1);
        assert_eq!(layout.hit_test(first.left, first.top - 1), None);
        assert_eq!(layout.hit_test(first.left, first.bottom + 1), None);
        assert!(second.left - first.right > 1);
        assert_eq!(
            layout.hit_test((first.right + second.left) / 2, (first.top + first.bottom) / 2),
            None
        );
    }

    #[test]
    fn zero_dpi_falls_back_to_default() {
        let layout = OverlayLayout::new(64, false, 2, 0);
        assert_eq!(layout.dpi, DEFAULT_DPI);
        assert_eq!(layout.icon_size, 64);
    }

    #[test]
    fn embedded_manifest_enables_per_monitor_v2_dpi_awareness() {
        use windows::Win32::UI::HiDpi::{
            AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        };
        unsafe {
            let ctx = GetThreadDpiAwarenessContext();
            assert!(
                AreDpiAwarenessContextsEqual(ctx, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
                    .as_bool()
            );
        }
    }
}
