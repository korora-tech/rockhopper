//! Icon theme lookup for desktop entries.

use std::path::{Path, PathBuf};

use crate::apps::App;

/// Logical icon size; looked up at 2× so it stays sharp on HiDPI outputs.
const SIZE: u16 = 32;
const SCALE: u16 = 2;

/// The configured theme, else the GTK setting, else `hicolor` only.
pub fn theme(configured: Option<&str>) -> Option<String> {
    configured
        .map(str::to_owned)
        .or_else(freedesktop_icons::default_theme_gtk)
}

pub fn resolve_all(apps: &mut [App], theme: Option<&str>) {
    for app in apps {
        app.icon_path = app.icon.as_deref().and_then(|icon| resolve(icon, theme));
    }
}

fn resolve(icon: &str, theme: Option<&str>) -> Option<PathBuf> {
    let path = Path::new(icon);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_owned());
    }
    // Some entries wrongly include the extension.
    let name = icon
        .strip_suffix(".png")
        .or_else(|| icon.strip_suffix(".svg"))
        .unwrap_or(icon);
    let lookup = |theme: &str| {
        freedesktop_icons::lookup(name)
            .with_size(SIZE)
            .with_scale(SCALE)
            .with_theme(theme)
            .with_cache()
            .find()
    };
    theme
        .and_then(lookup)
        .or_else(|| lookup("hicolor"))
        .or_else(|| {
            ["/usr/share/pixmaps", "/run/current-system/sw/share/pixmaps"]
                .iter()
                .flat_map(|d| ["png", "svg"].map(|ext| Path::new(d).join(format!("{name}.{ext}"))))
                .find(|p| p.is_file())
        })
}
