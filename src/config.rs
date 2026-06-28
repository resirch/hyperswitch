use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VIRTUAL_KEY, VK_BACK, VK_C, VK_DOWN, VK_LEFT, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP, VK_OEM_3,
};

/// Persisted user configuration. Loaded from / created at
/// `%APPDATA%\hyperswitch\config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Require Ctrl to be held as part of the activation hyperkey.
    pub hold_ctrl: bool,
    /// Require Alt to be held as part of the activation hyperkey.
    pub hold_alt: bool,
    /// Require Win to be held as part of the activation hyperkey.
    pub hold_win: bool,
    /// Require Shift to be held as part of the activation hyperkey.
    pub hold_shift: bool,
    /// Name of the key that advances the selection (e.g. "C").
    pub cycle_key: String,
    /// Name of the modifier that reverses cycling direction (e.g. "Shift").
    pub reverse_modifier: String,
    /// Whole-window opacity, 0 (transparent) - 255 (opaque).
    pub opacity: u8,
    /// Icon edge length in pixels.
    pub icon_size: i32,
    /// When true, only show windows on the monitor currently under the mouse
    /// cursor (and the overlay appears on that monitor).
    pub current_monitor_only: bool,
    /// When true, draw the selected window's title below the icon row.
    pub show_title: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hold_ctrl: true,
            hold_alt: true,
            hold_win: true,
            hold_shift: false,
            cycle_key: "C".to_string(),
            reverse_modifier: "Shift".to_string(),
            opacity: 235,
            icon_size: 64,
            current_monitor_only: true,
            show_title: true,
        }
    }
}

impl Config {
    /// Directory holding the config file.
    pub fn config_dir() -> PathBuf {
        let mut dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        dir.push("hyperswitch");
        dir
    }

    /// Full path to `config.toml`.
    pub fn config_path() -> PathBuf {
        let mut p = Self::config_dir();
        p.push("config.toml");
        p
    }

    /// Load config from disk, creating a default file if it does not exist.
    /// Any parse error falls back to defaults so the app always starts.
    pub fn load_or_create() -> Config {
        let path = Self::config_path();
        if let Ok(text) = std::fs::read_to_string(&path) {
            match toml::from_str::<Config>(&text) {
                Ok(cfg) => return cfg.sanitized(),
                Err(_) => return Config::default(),
            }
        }

        let cfg = Config::default();
        let _ = cfg.save();
        cfg
    }

    /// Write the current config to disk (best effort).
    pub fn save(&self) -> std::io::Result<()> {
        let dir = Self::config_dir();
        std::fs::create_dir_all(&dir)?;
        let text = toml::to_string_pretty(self)
            .unwrap_or_else(|_| String::from("# failed to serialize config"));
        std::fs::write(Self::config_path(), text)
    }

    /// Clamp / repair any out-of-range fields.
    fn sanitized(mut self) -> Config {
        if self.icon_size < 16 {
            self.icon_size = 16;
        }
        if self.icon_size > 256 {
            self.icon_size = 256;
        }
        self
    }

    /// Virtual key code for the configured cycle key.
    pub fn cycle_vk(&self) -> VIRTUAL_KEY {
        key_name_to_vk(&self.cycle_key).unwrap_or(VK_C)
    }
}

/// Map a friendly key name to a Win32 virtual key code.
/// Supports single letters/digits and a handful of named keys.
pub fn key_name_to_vk(name: &str) -> Option<VIRTUAL_KEY> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Single ASCII letter or digit maps directly to its VK (uppercased).
    if trimmed.chars().count() == 1 {
        let c = trimmed.chars().next().unwrap().to_ascii_uppercase();
        if c.is_ascii_alphanumeric() {
            return Some(VIRTUAL_KEY(c as u16));
        }
    }

    match trimmed.to_ascii_lowercase().as_str() {
        "tab" => Some(VK_TAB),
        "space" => Some(VK_SPACE),
        "backspace" | "back" => Some(VK_BACK),
        "left" => Some(VK_LEFT),
        "right" => Some(VK_RIGHT),
        "up" => Some(VK_UP),
        "down" => Some(VK_DOWN),
        "grave" | "backtick" | "tilde" => Some(VK_OEM_3),
        _ => None,
    }
}
