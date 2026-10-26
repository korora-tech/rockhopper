//! `$XDG_CONFIG_HOME/rockhopper/config.yaml`, usually written by the Home
//! Manager module. Every key is optional.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::Deserialize;

use crate::commands::CommandConfig;

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
    pub emoji: EmojiConfig,
    /// Text that, typed first, narrows the search to one kind of result.
    pub prefixes: Prefixes,
    /// Commands by key, laid over the built-in ones.
    pub commands: BTreeMap<String, CommandConfig>,
}

/// What a prefix narrows the search to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Apps,
    Windows,
    Commands,
    Emoji,
    Clipboard,
}

/// A prefix per scope; `None` or empty turns it off. The defaults are in
/// `defaults.json`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Prefixes {
    pub apps: Option<String>,
    pub windows: Option<String>,
    pub commands: Option<String>,
    pub emoji: Option<String>,
    pub clipboard: Option<String>,
}

impl Default for Prefixes {
    fn default() -> Self {
        // Not `Prefixes` itself: its serde default would call this again.
        #[derive(Deserialize)]
        struct Raw {
            apps: Option<String>,
            windows: Option<String>,
            commands: Option<String>,
            emoji: Option<String>,
            clipboard: Option<String>,
        }
        #[derive(Deserialize)]
        struct Defaults {
            prefixes: Raw,
        }
        let p = serde_json::from_str::<Defaults>(crate::commands::DEFAULTS)
            .expect("defaults.json is valid")
            .prefixes;
        Self {
            apps: p.apps,
            windows: p.windows,
            commands: p.commands,
            emoji: p.emoji,
            clipboard: p.clipboard,
        }
    }
}

impl Prefixes {
    /// The scope whose prefix `text` starts with, and the text after it. The
    /// longest prefix wins, so `>` and `>>` can both be set.
    pub fn split<'a>(&self, text: &'a str) -> Option<(Scope, &'a str)> {
        [
            (Scope::Apps, &self.apps),
            (Scope::Windows, &self.windows),
            (Scope::Commands, &self.commands),
            (Scope::Emoji, &self.emoji),
            (Scope::Clipboard, &self.clipboard),
        ]
        .into_iter()
        .filter_map(|(scope, prefix)| {
            let prefix = prefix.as_deref().filter(|p| !p.is_empty())?;
            Some((scope, prefix, text.strip_prefix(prefix)?))
        })
        .max_by_key(|(_, prefix, _)| prefix.len())
        .map(|(scope, _, rest)| (scope, rest))
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct EmojiConfig {
    /// Applied to every emoji that has skin tones.
    pub skin_tone: crate::emoji::SkinTone,
    /// Colour emoji font family for the picker, with PNG glyphs (such as
    /// Noto Color Emoji or JoyPixels); defaults to the first one installed.
    pub font: Option<String>,
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
            emoji: EmojiConfig::default(),
            prefixes: Prefixes::default(),
            commands: BTreeMap::new(),
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

/// A file's resolved path and modification time for cheap change detection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStamp {
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
}

/// Identifies a version of the config file. Home Manager makes the file a
/// symlink into the Nix store, where every mtime is 1, so the resolved path
/// is part of it: each generation resolves to a new store path.
pub type Stamp = Option<FileStamp>;

pub fn stamp() -> Stamp {
    let real = std::fs::canonicalize(path()?).ok()?;
    let mtime = std::fs::metadata(&real).and_then(|m| m.modified()).ok();
    Some(FileStamp {
        path: real,
        modified: mtime,
    })
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
        let c =
            parse("appearance:\n  theme: dark\n  width: 800\nterminal: [ghostty, -e]\n").unwrap();
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
    fn skin_tone() {
        let c = parse("emoji:\n  skin_tone: medium-dark\n").unwrap();
        assert_eq!(c.emoji.skin_tone, crate::emoji::SkinTone::MediumDark);
    }

    #[test]
    fn prefixes() {
        let p = Prefixes::default();
        assert_eq!(p.split(">full"), Some((Scope::Commands, "full")));
        assert_eq!(p.split("e rocket"), Some((Scope::Emoji, "rocket")));
        assert_eq!(p.split("emacs"), None);
        let c = parse("prefixes:\n  commands: null\n  windows: \"w \"\n  apps: \">>\"\n").unwrap();
        assert_eq!(c.prefixes.split(">x"), None);
        assert_eq!(c.prefixes.split(">>x"), Some((Scope::Apps, "x")));
        assert_eq!(c.prefixes.split("w term"), Some((Scope::Windows, "term")));
        assert_eq!(c.prefixes.emoji.as_deref(), Some("e "));
    }

    #[test]
    fn commands() {
        let c = parse("commands:\n  niri-close:\n    enabled: false\n  term:\n    name: Terminal\n    run: [ghostty]\n").unwrap();
        assert_eq!(c.commands["niri-close"].enabled, Some(false));
        assert_eq!(
            c.commands["term"].run.as_deref(),
            Some(&["ghostty".to_string()][..])
        );
    }

    #[test]
    fn rejects_typos() {
        assert!(parse("apperance: {}\n").is_err());
    }
}
