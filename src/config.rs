//! `$XDG_CONFIG_HOME/rockhopper/config.yaml`, usually written by the Home
//! Manager module. Every key is optional.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub appearance: Appearance,
    /// Prefix for `Terminal=true` apps; the app's command is appended.
    pub terminal: Vec<String>,
    pub launch: Launch,
    /// Icon theme name; defaults to the GTK setting.
    pub icon_theme: Option<String>,
    pub system: System,
    pub clipboard: ClipboardConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ClipboardConfig {
    /// Watch the clipboard. Read at startup only.
    pub enabled: bool,
    pub max_entries: usize,
    /// Keep copied PNG images as well as text.
    pub images: bool,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_entries: 200,
            images: true,
        }
    }
}

/// Overrides for session commands, as program and arguments.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct System {
    /// Default: `loginctl lock-session`.
    pub lock: Option<Vec<String>>,
    /// Default: niri's quit action, else `loginctl terminate-session`.
    pub logout: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub theme: ThemeChoice,
    /// Panel width in logical pixels.
    pub width: f32,
    /// Distance from the top of the screen, as a fraction of its height.
    pub top: f32,
    /// Dim the rest of the screen while open.
    pub dim: bool,
    /// Output to open on, such as `DP-1`; defaults to the focused one.
    pub output: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Launch {
    /// Start each app in its own systemd user scope, so stopping Rockhopper
    /// never takes launched apps with it.
    pub systemd_scope: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            appearance: Appearance::default(),
            terminal: vec!["xdg-terminal-exec".into()],
            launch: Launch::default(),
            icon_theme: None,
            system: System::default(),
            clipboard: ClipboardConfig::default(),
        }
    }
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            width: 720.0,
            top: 0.2,
            dim: true,
            output: None,
        }
    }
}

impl Default for Launch {
    fn default() -> Self {
        Self {
            systemd_scope: true,
        }
    }
}

pub fn path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|d| d.join("rockhopper/config.yaml"))
}

/// Identifies a version of the config file. Home Manager makes the file a
/// symlink into the Nix store, where every mtime is 1, so the resolved path
/// is part of it: each generation resolves to a new store path.
pub type Stamp = Option<(PathBuf, Option<SystemTime>)>;

pub fn stamp() -> Stamp {
    let real = std::fs::canonicalize(path()?).ok()?;
    let mtime = std::fs::metadata(&real).and_then(|m| m.modified()).ok();
    Some((real, mtime))
}

/// A missing file means defaults; a broken one is reported and also means
/// defaults, so a typo never leaves you without a launcher.
pub fn load() -> Config {
    let Some(path) = path() else {
        return Config::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text).unwrap_or_else(|err| {
            eprintln!("rockhopper: {}: {err}; using defaults", path.display());
            Config::default()
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(err) => {
            eprintln!("rockhopper: {}: {err}; using defaults", path.display());
            Config::default()
        }
    }
}

fn parse(text: &str) -> Result<Config, serde_saphyr::Error> {
    if text.trim().is_empty() {
        return Ok(Config::default());
    }
    serde_saphyr::from_str(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default() {
        assert_eq!(parse("").unwrap(), Config::default());
    }

    #[test]
    fn partial_overrides() {
        let c = parse("appearance:\n  theme: dark\n  width: 800\nterminal: [ghostty, -e]\n").unwrap();
        assert_eq!(c.appearance.theme, ThemeChoice::Dark);
        assert_eq!(c.appearance.width, 800.0);
        assert_eq!(c.appearance.top, 0.2);
        assert_eq!(c.terminal, ["ghostty", "-e"]);
        assert!(c.launch.systemd_scope);
    }

    /// The header `pkgs.formats.yaml` writes.
    #[test]
    fn home_manager_output() {
        let c = parse("%YAML 1.1\n---\nappearance:\n  theme: dark\n  width: 760\nterminal:\n- ghostty\n- -e\n").unwrap();
        assert_eq!(c.appearance.width, 760.0);
        assert_eq!(c.terminal, ["ghostty", "-e"]);
    }

    #[test]
    fn rejects_typos() {
        assert!(parse("apperance: {}\n").is_err());
    }
}
