//! Everything the launcher can list, and what can be done with each.

use std::path::PathBuf;

use crate::apps::App;
use crate::commands::Command;
use crate::history::History;
use crate::niri::Window;
use crate::search::Candidate;
use crate::system::SystemCommand;

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// Index into the app list.
    App(usize),
    /// An open window, with the app it belongs to when known.
    Window {
        window: Window,
        app: Option<usize>,
    },
    System(SystemCommand),
    /// A configured command, with the window it acts on (the focused one)
    /// and that window's app when known.
    Command {
        command: Command,
        window: Option<Window>,
        app: Option<usize>,
    },
    /// Opens the clipboard history.
    Clipboard,
    /// Opens the emoji picker.
    Emoji,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Symbol {
    Clipboard,
    Emoji,
    Text,
    Image,
    Lock,
    Suspend,
    LogOut,
    Restart,
    Power,
    Window,
    Command,
}

pub enum ItemIcon {
    File(PathBuf),
    /// A coloured tile with this text's first letter.
    Initial(String),
    Symbol(Symbol),
}

/// An item with what the ranker matches it on.
#[derive(Clone, Debug)]
pub struct Entry {
    pub item: Item,
    pub candidate: Candidate,
}

/// What a search scope lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    App,
    Window,
    Command,
}

/// Apps, then windows, then commands. Commands that act on a window are
/// listed only when a window has focus.
pub fn entries(
    apps: &[App],
    windows: &[Window],
    commands: &[Command],
    clipboard: bool,
) -> Vec<Entry> {
    let app_of = |window: &Window| {
        window
            .app_id
            .as_deref()
            .and_then(|id| apps.iter().position(|a| a.owns_window(id)))
    };
    let app_entries = apps.iter().enumerate().map(|(i, app)| {
        let program = app
            .argv
            .first()
            .map(|p| p.rsplit('/').next().unwrap_or(p).to_owned());
        Entry {
            item: Item::App(i),
            candidate: Candidate {
                name: app.name.clone(),
                secondary: app
                    .generic_name
                    .iter()
                    .cloned()
                    .chain(app.keywords.iter().cloned())
                    .chain(program)
                    .collect(),
                key: Some(app.id.clone()),
            },
        }
    });
    let window_entries = windows.iter().map(|window| {
        let app = app_of(window);
        Entry {
            candidate: Candidate {
                name: window.title.clone(),
                secondary: app
                    .map(|a| apps[a].name.clone())
                    .into_iter()
                    .chain(window.app_id.clone())
                    .collect(),
                key: None,
            },
            item: Item::Window {
                window: window.clone(),
                app,
            },
        }
    });
    let system_entries = SystemCommand::ALL.into_iter().map(|cmd| Entry {
        item: Item::System(cmd),
        candidate: Candidate {
            name: cmd.name().into(),
            secondary: cmd.keywords().iter().map(|k| k.to_string()).collect(),
            key: Some(cmd.key().into()),
        },
    });
    let focused = windows.iter().find(|w| w.is_focused);
    let command_entries = commands
        .iter()
        .filter(|c| focused.is_some() || !c.needs_window())
        .map(|command| Entry {
            candidate: Candidate {
                name: command.name.clone(),
                secondary: command.keywords.clone(),
                key: Some(command.history_key()),
            },
            item: Item::Command {
                command: command.clone(),
                window: focused.filter(|_| command.needs_window()).cloned(),
                app: focused.filter(|_| command.needs_window()).and_then(app_of),
            },
        });
    let clipboard_entry = clipboard.then(|| Entry {
        item: Item::Clipboard,
        candidate: Candidate {
            name: "Clipboard History".into(),
            secondary: vec!["clipboard".into(), "paste".into(), "copied".into()],
            key: Some("rockhopper:clipboard".into()),
        },
    });
    let emoji_entry = Entry {
        item: Item::Emoji,
        candidate: Candidate {
            name: "Search Emoji".into(),
            secondary: vec!["emoji".into(), "emoticon".into(), "smiley".into()],
            key: Some("rockhopper:emoji".into()),
        },
    };
    app_entries
        .chain(window_entries)
        .chain(system_entries)
        .chain(command_entries)
        .chain(clipboard_entry)
        .chain([emoji_entry])
        .collect()
}

impl Item {
    pub fn kind_of(&self) -> Kind {
        match self {
            Self::App(_) => Kind::App,
            Self::Window { .. } => Kind::Window,
            Self::System(_) | Self::Command { .. } | Self::Clipboard | Self::Emoji => Kind::Command,
        }
    }

    pub fn title(&self, apps: &[App]) -> String {
        match self {
            Self::App(i) => apps[*i].name.clone(),
            Self::Window { window, .. } => window.title.clone(),
            Self::System(cmd) => cmd.name().into(),
            Self::Command { command, .. } => command.name.clone(),
            Self::Clipboard => "Clipboard History".into(),
            Self::Emoji => "Search Emoji".into(),
        }
    }

    pub fn subtitle(&self, apps: &[App]) -> Option<String> {
        match self {
            Self::App(i) => apps[*i]
                .comment
                .clone()
                .or_else(|| apps[*i].generic_name.clone()),
            Self::Window { window, app } => {
                let owner = app
                    .map(|a| apps[a].name.clone())
                    .or_else(|| window.app_id.clone());
                let place = window.workspace.as_ref().map(|w| format!("Workspace {w}"));
                let parts: Vec<String> = owner.into_iter().chain(place).collect();
                (!parts.is_empty()).then(|| parts.join(" · "))
            }
            Self::System(_) => None,
            // The window it acts on.
            Self::Command {
                window: Some(window),
                app,
                ..
            } => Some(match app {
                Some(a) => format!("{} · {}", apps[*a].name, window.title),
                None => window.title.clone(),
            }),
            Self::Command { .. } => None,
            Self::Clipboard => Some("Search what you copied".into()),
            Self::Emoji => Some("Find an emoji and copy it".into()),
        }
    }

    /// The kind shown at the right of the row.
    pub fn kind(&self, apps: &[App]) -> &'static str {
        match self {
            Self::App(i) if apps[*i].terminal => "Terminal",
            Self::App(_) => "Application",
            Self::Window { .. } => "Window",
            Self::System(_) | Self::Command { .. } | Self::Clipboard | Self::Emoji => "Command",
        }
    }

    pub fn icon(&self, apps: &[App]) -> ItemIcon {
        let app_icon = |app: &App| match &app.icon_path {
            Some(path) => ItemIcon::File(path.clone()),
            None => ItemIcon::Initial(app.name.clone()),
        };
        match self {
            Self::App(i) => app_icon(&apps[*i]),
            Self::Window { app: Some(a), .. } => app_icon(&apps[*a]),
            Self::Window { app: None, .. } => ItemIcon::Symbol(Symbol::Window),
            Self::System(cmd) => ItemIcon::Symbol(match cmd {
                SystemCommand::Lock => Symbol::Lock,
                SystemCommand::Suspend => Symbol::Suspend,
                SystemCommand::LogOut => Symbol::LogOut,
                SystemCommand::Reboot => Symbol::Restart,
                SystemCommand::PowerOff => Symbol::Power,
            }),
            Self::Command { app: Some(a), .. } => app_icon(&apps[*a]),
            Self::Command {
                window: Some(_), ..
            } => ItemIcon::Symbol(Symbol::Window),
            Self::Command { .. } => ItemIcon::Symbol(Symbol::Command),
            Self::Clipboard => ItemIcon::Symbol(Symbol::Clipboard),
            Self::Emoji => ItemIcon::Symbol(Symbol::Emoji),
        }
    }

    /// The verb for `enter`, shown in the footer.
    pub fn verb(&self) -> &'static str {
        match self {
            Self::App(_) => "Open",
            Self::Window { .. } => "Switch to",
            Self::System(_) | Self::Command { .. } => "Run",
            Self::Clipboard | Self::Emoji => "Open",
        }
    }

    pub fn history_key(&self, apps: &[App]) -> Option<String> {
        match self {
            Self::App(i) => Some(apps[*i].id.clone()),
            Self::Window { .. } => None,
            Self::System(cmd) => Some(cmd.key().into()),
            Self::Command { command, .. } => Some(command.history_key()),
            Self::Clipboard => Some("rockhopper:clipboard".into()),
            Self::Emoji => Some("rockhopper:emoji".into()),
        }
    }

    /// Everything that can be done with this item, the `enter` action first.
    /// A window also offers every command that acts on a window.
    pub fn actions(&self, apps: &[App], commands: &[Command], history: &History) -> Vec<ActionDef> {
        let mut actions = vec![ActionDef::new(Action::Primary, self.verb())];
        match self {
            Self::App(i) => {
                let app = &apps[*i];
                actions.extend(
                    app.actions
                        .iter()
                        .enumerate()
                        .map(|(n, a)| ActionDef::new(Action::Desktop(n), &a.name)),
                );
                actions.push(ActionDef::new(Action::CopyCommand, "Copy Command"));
            }
            Self::Window { app, .. } => {
                if let Some(a) = app {
                    actions.push(ActionDef::new(
                        Action::NewInstance,
                        &format!("Open New {}", apps[*a].name),
                    ));
                }
                actions.push(ActionDef::new(Action::CloseWindow, "Close Window"));
                actions.push(ActionDef::new(Action::CopyTitle, "Copy Title"));
                for (n, command) in commands.iter().enumerate() {
                    // Such as a configured Close Window.
                    if command.needs_window() && !actions.iter().any(|a| a.label == command.name) {
                        actions.push(ActionDef::new(Action::Command(n), &command.name));
                    }
                }
            }
            Self::System(_) | Self::Command { .. } | Self::Clipboard | Self::Emoji => {}
        }
        if self
            .history_key(apps)
            .is_some_and(|k| history.frecency(&k, crate::history::now()) > 0.0)
        {
            actions.push(ActionDef::new(Action::Forget, "Reset Ranking"));
        }
        actions
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// What `enter` does: open, switch to or run.
    Primary,
    /// Index into the app's desktop actions.
    Desktop(usize),
    CopyCommand,
    NewInstance,
    CloseWindow,
    CopyTitle,
    /// Index into the commands, run on this window.
    Command(usize),
    /// Drop the item from launch history.
    Forget,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActionDef {
    pub action: Action,
    pub label: String,
}

impl ActionDef {
    fn new(action: Action, label: &str) -> Self {
        Self {
            action,
            label: label.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::DesktopAction;

    fn app(id: &str, name: &str) -> App {
        App {
            id: id.into(),
            name: name.into(),
            generic_name: None,
            comment: None,
            keywords: vec![],
            argv: vec![format!("/bin/{}", name.to_lowercase())],
            terminal: false,
            cwd: None,
            icon: None,
            icon_path: None,
            wm_class: None,
            actions: vec![DesktopAction {
                name: "New Window".into(),
                argv: vec!["x".into()],
            }],
        }
    }

    fn window(app_id: &str) -> Window {
        Window {
            id: 7,
            title: "README.md".into(),
            app_id: Some(app_id.into()),
            workspace: Some("2".into()),
            is_focused: false,
        }
    }

    #[test]
    fn windows_find_their_app() {
        let apps = vec![app("com.mitchellh.ghostty.desktop", "Ghostty")];
        let entries = entries(
            &apps,
            &[window("com.mitchellh.ghostty"), window("unknown")],
            &[],
            false,
        );
        let windows: Vec<_> = entries
            .iter()
            .filter_map(|e| match &e.item {
                Item::Window { app, .. } => Some(*app),
                _ => None,
            })
            .collect();
        assert_eq!(windows, [Some(0), None]);
        let ghostty_window = &entries[1];
        assert_eq!(
            ghostty_window.candidate.secondary,
            ["Ghostty", "com.mitchellh.ghostty"]
        );
        assert_eq!(
            ghostty_window.item.subtitle(&apps).as_deref(),
            Some("Ghostty · Workspace 2")
        );
    }

    #[test]
    fn window_commands_need_a_focused_window() {
        let apps = vec![app("com.mitchellh.ghostty.desktop", "Ghostty")];
        let commands = crate::commands::resolve(&Default::default());
        let count = |windows: &[Window]| {
            entries(&apps, windows, &commands, false)
                .iter()
                .filter(|e| matches!(e.item, Item::Command { .. }))
                .count()
        };
        assert_eq!(count(&[window("com.mitchellh.ghostty")]), 0);
        let focused = Window {
            is_focused: true,
            ..window("com.mitchellh.ghostty")
        };
        assert_eq!(count(std::slice::from_ref(&focused)), commands.len());
        let entries = entries(&apps, &[focused], &commands, false);
        let fullscreen = entries
            .iter()
            .find(|e| e.candidate.name == "Toggle Fullscreen")
            .unwrap();
        assert_eq!(
            fullscreen.item.subtitle(&apps).as_deref(),
            Some("Ghostty · README.md")
        );
        assert_eq!(fullscreen.item.kind_of(), Kind::Command);
    }

    #[test]
    fn action_lists() {
        let apps = vec![app("a.desktop", "A")];
        let commands = crate::commands::resolve(&Default::default());
        let mut history = History::default();
        let labels = |item: &Item, h: &History| -> Vec<String> {
            item.actions(&apps, &commands, h)
                .into_iter()
                .map(|a| a.label)
                .collect()
        };
        assert_eq!(
            labels(&Item::App(0), &history),
            ["Open", "New Window", "Copy Command"]
        );
        history.record("a.desktop", crate::history::now());
        assert_eq!(
            labels(&Item::App(0), &history).last().unwrap(),
            "Reset Ranking"
        );
        let w = Item::Window {
            window: window("a"),
            app: Some(0),
        };
        assert_eq!(
            labels(&w, &history)[..4],
            ["Switch to", "Open New A", "Close Window", "Copy Title"]
        );
        let after: Vec<String> = labels(&w, &history)[4..].to_vec();
        assert!(after.contains(&"Toggle Fullscreen".to_string()));
        // The configured Close Window is not offered twice.
        assert!(!after.contains(&"Close Window".to_string()));
        assert_eq!(
            labels(&Item::System(SystemCommand::Lock), &history),
            ["Run"]
        );
    }
}
