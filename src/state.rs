use std::cell::RefCell;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::WM_APP;

use crate::config::Config;
use crate::windows_enum::WindowInfo;

// Custom window messages posted from the hook (which runs on the main/UI
// thread) so that all window creation/painting happens via the window proc.
pub const WM_HS_SHOW: u32 = WM_APP + 1;
pub const WM_HS_UPDATE: u32 = WM_APP + 2;
pub const WM_HS_HIDE: u32 = WM_APP + 3;
pub const WM_HS_COMMIT: u32 = WM_APP + 4;
pub const WM_HS_TRAY: u32 = WM_APP + 5;

/// Aggregate which modifier the configured reverse key refers to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ModKind {
    Ctrl,
    Alt,
    Win,
    Shift,
}

/// All shared, single-threaded application state. The low-level keyboard hook
/// callback and the overlay window proc both run on the main thread, so a
/// thread-local is sufficient and avoids `Send`/`Sync` issues with raw HWNDs.
pub struct AppState {
    pub overlay: HWND,
    pub visible: bool,
    /// Full MRU-sorted window list, kept current while the overlay is hidden.
    pub all_windows: Vec<WindowInfo>,
    /// Windows shown in the overlay (filtered view of `all_windows`).
    pub windows: Vec<WindowInfo>,
    pub selected: usize,
    pub config: Config,

    /// Most-recently-used window order (front = most recent foreground).
    pub recent_hwnds: Vec<HWND>,

    // Tracked physical modifier state (left/right tracked separately so that
    // releasing one side does not clear the other).
    pub lctrl: bool,
    pub rctrl: bool,
    pub lalt: bool,
    pub ralt: bool,
    pub lwin: bool,
    pub rwin: bool,
    pub lshift: bool,
    pub rshift: bool,
}

impl AppState {
    pub fn ctrl(&self) -> bool {
        self.lctrl || self.rctrl
    }
    pub fn alt(&self) -> bool {
        self.lalt || self.ralt
    }
    pub fn win(&self) -> bool {
        self.lwin || self.rwin
    }
    pub fn shift(&self) -> bool {
        self.lshift || self.rshift
    }

    /// Are all configured hold-modifiers currently held?
    pub fn all_hold_mods_down(&self) -> bool {
        let c = &self.config;
        (!c.hold_ctrl || self.ctrl())
            && (!c.hold_alt || self.alt())
            && (!c.hold_win || self.win())
            && (!c.hold_shift || self.shift())
            // at least one modifier must be configured to avoid accidental fire
            && (c.hold_ctrl || c.hold_alt || c.hold_win || c.hold_shift)
    }

    /// Do any of the configured hold-modifiers remain held?
    pub fn any_hold_mod_held(&self) -> bool {
        let c = &self.config;
        (c.hold_ctrl && self.ctrl())
            || (c.hold_alt && self.alt())
            || (c.hold_win && self.win())
            || (c.hold_shift && self.shift())
    }

    /// Is the configured reverse modifier currently held?
    pub fn reverse_held(&self) -> bool {
        match self.reverse_kind() {
            Some(ModKind::Ctrl) => self.ctrl(),
            Some(ModKind::Alt) => self.alt(),
            Some(ModKind::Win) => self.win(),
            Some(ModKind::Shift) => self.shift(),
            None => false,
        }
    }

    fn reverse_kind(&self) -> Option<ModKind> {
        match self.config.reverse_modifier.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Some(ModKind::Ctrl),
            "alt" | "menu" => Some(ModKind::Alt),
            "win" | "windows" | "super" => Some(ModKind::Win),
            "shift" => Some(ModKind::Shift),
            _ => None,
        }
    }

    /// Record that `hwnd` was just brought to the foreground.
    pub fn touch_recent(&mut self, hwnd: HWND) {
        if hwnd.0.is_null() {
            return;
        }
        self.recent_hwnds.retain(|h| h.0 != hwnd.0);
        self.recent_hwnds.insert(0, hwnd);
    }

    /// Rebuild the cached window list from the live desktop and sort by MRU.
    pub fn refresh_all_windows(&mut self) {
        let fresh = crate::windows_enum::enumerate_windows();
        self.all_windows = self.sort_by_recency(fresh);
    }

    /// Build the overlay row from the cached list (optionally filtered to the
    /// cursor monitor). Cheap enough to call right before showing the overlay.
    pub fn apply_display_filter(&mut self) {
        if self.config.current_monitor_only {
            let cm = crate::windows_enum::cursor_monitor();
            self.windows = self
                .all_windows
                .iter()
                .filter(|w| crate::windows_enum::window_monitor(w.hwnd).0 == cm.0)
                .cloned()
                .collect();
        } else {
            self.windows.clone_from(&self.all_windows);
        }
    }

    /// Sort `fresh` by MRU order (most recently used first). Updates the tracked
    /// list to drop closed windows and append any newly seen ones.
    pub fn sort_by_recency(&mut self, fresh: Vec<WindowInfo>) -> Vec<WindowInfo> {
        let mut fresh = fresh;
        fresh.sort_by_key(|w| {
            self.recent_hwnds
                .iter()
                .position(|h| h.0 == w.hwnd.0)
                .unwrap_or(usize::MAX)
        });

        let open: std::collections::HashSet<isize> =
            fresh.iter().map(|w| w.hwnd.0 as isize).collect();
        self.recent_hwnds.retain(|h| open.contains(&(h.0 as isize)));
        for w in &fresh {
            if !self.recent_hwnds.iter().any(|h| h.0 == w.hwnd.0) {
                self.recent_hwnds.push(w.hwnd);
            }
        }
        fresh
    }

    pub fn select_next(&mut self) {
        if self.windows.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.windows.len();
    }

    pub fn select_prev(&mut self) {
        if self.windows.is_empty() {
            return;
        }
        self.selected = (self.selected + self.windows.len() - 1) % self.windows.len();
    }
}

thread_local! {
    static STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

pub fn init(state: AppState) {
    STATE.with(|s| *s.borrow_mut() = Some(state));
}

/// Run a closure with mutable access to the app state, if initialized.
pub fn with<R>(f: impl FnOnce(&mut AppState) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}
