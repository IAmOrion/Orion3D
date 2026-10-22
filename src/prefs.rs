//! App preferences, stored as JSON in the user's config folder:
//!   Windows: %APPDATA%\Orion3D\settings.json
//!   macOS:   ~/Library/Application Support/Orion3D/settings.json
//!   Linux:   $XDG_CONFIG_HOME/orion3d/settings.json (or ~/.config/...)

use crate::doc::{Rgb, Settings};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub show_grid: bool,
    pub background: Rgb,
    /// 1..=10, 5 is the middle.
    pub zoom_speed: u8,
    /// Workplane, units and scale for new designs ("Make default").
    pub new_design: Settings,
    /// New shapes land on the face under the cursor.
    pub cruise: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { show_grid: true, background: Rgb::hex(0xf3f5f8), zoom_speed: 5, new_design: Settings::default(), cruise: true }
    }
}

fn path() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let dir = if cfg!(windows) {
        env("APPDATA")?.join("Orion3D")
    } else if cfg!(target_os = "macos") {
        env("HOME")?.join("Library/Application Support/Orion3D")
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))?.join("orion3d")
    };
    Some(dir.join("settings.json"))
}

impl Prefs {
    pub fn load() -> Prefs {
        path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let p = path().ok_or("No settings folder found")?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(p, json).map_err(|e| e.to_string())
    }

    /// Zoom factor per wheel point.
    pub fn zoom_rate(&self) -> f32 {
        0.0025 * 1.25f32.powi(self.zoom_speed as i32 - 5)
    }
}
