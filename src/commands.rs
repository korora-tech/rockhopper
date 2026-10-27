//! Commands from the config: command lines listed in the launcher, which may
//! act on a window. The built-in set (niri window actions) comes from
//! `defaults.json`, which the Home Manager module reads too.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::niri::Window;

/// Replaced by the target window's id; a command that uses it is only
/// offered when there is a window to act on.
pub const WINDOW_ID: &str = "{window_id}";

#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    /// The config key, also its history key.
    pub key: String,
    pub name: String,
    pub keywords: Vec<String>,
    pub run: Vec<String>,
}

/// One entry under `commands:` in the config. For a built-in key, the fields
/// given replace the built-in ones; a new key needs `name` and `run`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CommandConfig {
    pub enabled: Option<bool>,
    pub name: Option<String>,
    pub keywords: Option<Vec<String>>,
    pub run: Option<Vec<String>>,
}

impl Command {
    pub fn history_key(&self) -> String {
        format!("command:{}", self.key)
    }

    pub fn needs_window(&self) -> bool {
        self.run.iter().any(|a| a.contains(WINDOW_ID))
    }

    /// The command line for `window`, or `None` when it needs a window and
    /// there is none.
    pub fn argv(&self, window: Option<&Window>) -> Option<Vec<String>> {
        let id = window.map(|w| w.id.to_string());
        self.run
            .iter()
            .map(|arg| match &id {
                _ if !arg.contains(WINDOW_ID) => Some(arg.clone()),
                Some(id) => Some(arg.replace(WINDOW_ID, id)),
                None => None,
            })
            .collect()
    }
}

#[derive(Deserialize)]
struct Defaults {
    commands: BTreeMap<String, CommandConfig>,
}

pub const DEFAULTS: &str = include_str!("defaults.json");

fn builtin() -> BTreeMap<String, CommandConfig> {
    serde_json::from_str::<Defaults>(DEFAULTS)
        .expect("defaults.json is valid")
        .commands
}

/// The built-in commands with the configured ones laid over them, by key.
/// Disabled commands are left out, and incomplete ones are reported.
pub fn resolve(configured: &BTreeMap<String, CommandConfig>) -> Vec<Command> {
    let mut merged = builtin();
    for (key, over) in configured {
        let entry = merged.entry(key.clone()).or_default();
        let over = over.clone();
        entry.enabled = over.enabled.or(entry.enabled);
        entry.name = over.name.or(entry.name.take());
        entry.keywords = over.keywords.or(entry.keywords.take());
        entry.run = over.run.or(entry.run.take());
    }
    merged
        .into_iter()
        .filter(|(_, c)| c.enabled != Some(false))
        .filter_map(|(key, c)| match (c.name, c.run) {
            (Some(name), Some(run)) if !run.is_empty() => Some(Command {
                key,
                name,
                keywords: c.keywords.unwrap_or_default(),
                run,
            }),
            _ => {
                eprintln!("rockhopper: command {key:?} needs a name and a command line to run");
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Window {
        Window {
            id: 42,
            title: "t".into(),
            app_id: None,
            workspace: None,
            is_focused: true,
        }
    }

    #[test]
    fn builtins_act_on_a_window() {
        let commands = resolve(&BTreeMap::new());
        let fullscreen = commands
            .iter()
            .find(|c| c.key == "niri-fullscreen")
            .unwrap();
        assert!(fullscreen.needs_window());
        assert_eq!(fullscreen.argv(None), None);
        assert_eq!(
            fullscreen.argv(Some(&window())).unwrap(),
            ["niri", "msg", "action", "fullscreen-window", "--id", "42"]
        );
    }

    #[test]
    fn overrides_merge_by_key() {
        let configured = BTreeMap::from([
            (
                "niri-fullscreen".to_string(),
                CommandConfig {
                    keywords: Some(vec!["big".into()]),
                    ..Default::default()
                },
            ),
            (
                "niri-close".to_string(),
                CommandConfig {
                    enabled: Some(false),
                    ..Default::default()
                },
            ),
            (
                "editor".to_string(),
                CommandConfig {
                    name: Some("Edit Config".into()),
                    run: Some(vec!["xdg-open".into(), "config.yaml".into()]),
                    ..Default::default()
                },
            ),
            ("broken".to_string(), CommandConfig::default()),
        ]);
        let commands = resolve(&configured);
        let fullscreen = commands
            .iter()
            .find(|c| c.key == "niri-fullscreen")
            .unwrap();
        assert_eq!(fullscreen.name, "Toggle Fullscreen");
        assert_eq!(fullscreen.keywords, ["big"]);
        assert!(!commands.iter().any(|c| c.key == "niri-close"));
        assert!(!commands.iter().any(|c| c.key == "broken"));
        let editor = commands.iter().find(|c| c.key == "editor").unwrap();
        assert!(!editor.needs_window());
        assert_eq!(editor.argv(None).unwrap(), ["xdg-open", "config.yaml"]);
    }
}
