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
    pub windows: Vec<WindowInfo>,
    pub selected: usize,
    pub config: Config,

    /// Persistent window ordering across activations so the row does not
    /// reshuffle every time the overlay opens.
    pub cached_order: Vec<HWND>,

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

    /// Reconcile freshly enumerated windows against the cached order: keep the
    /// existing relative order for windows still present, append newly-appeared
    /// windows at the end, and drop windows that have closed. Returns the
    /// windows in the stable order and updates the cache.
    pub fn reconcile(&mut self, fresh: Vec<WindowInfo>) -> Vec<WindowInfo> {
        let mut ordered: Vec<WindowInfo> = Vec::with_capacity(fresh.len());
        let mut used = vec![false; fresh.len()];

        for cached in &self.cached_order {
            if let Some(idx) = fresh.iter().position(|w| w.hwnd.0 == cached.0) {
                if !used[idx] {
                    used[idx] = true;
                    ordered.push(fresh[idx].clone());
                }
            }
        }
        for (i, w) in fresh.iter().enumerate() {
            if !used[i] {
                ordered.push(w.clone());
            }
        }

        self.cached_order = ordered.iter().map(|w| w.hwnd).collect();
        ordered
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
