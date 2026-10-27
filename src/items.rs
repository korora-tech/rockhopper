//! Everything the launcher can list, and what can be done with each.

use std::path::PathBuf;

use crate::apps::App;
use crate::commands::Command;
use crate::history::History;
use crate::niri::{CommandScope, FocusedWindowSnapshot, NIRI_COMMANDS, NiriCommand, Window};
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
    /// A searchable niri command, bound to the window focused when the
    /// launcher opened.
    NiriCommand {
        command: NiriCommand,
        window_id: Option<u64>,
        is_floating: bool,
    },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppLaunchPolicy {
    PreferExisting,
    AlwaysLaunch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppLaunchIntent {
    Default,
    NewInstance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppOpen {
    FocusWindow(u64),
    Launch,
}

/// Choose how to activate an app. `windows` is niri's most-recently-focused-first list.
pub fn choose_app_open(
    app: &App,
    windows: &[Window],
    policy: AppLaunchPolicy,
    intent: AppLaunchIntent,
) -> AppOpen {
    if policy == AppLaunchPolicy::AlwaysLaunch || intent == AppLaunchIntent::NewInstance {
        return AppOpen::Launch;
    }
    windows
        .iter()
        .find(|window| {
            window
                .app_id
                .as_deref()
                .is_some_and(|id| app.owns_window(id))
        })
        .map_or(AppOpen::Launch, |window| AppOpen::FocusWindow(window.id))
}

/// What a search scope lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    App,
    Window,
    Command,
}

fn configured_key_for_niri_command(command: &NiriCommand) -> Option<&'static str> {
    match command.key {
        "niri:toggle-fullscreen" => Some("niri-fullscreen"),
        "niri:toggle-floating" => Some("niri-floating"),
        "niri:center-window" => Some("niri-center"),
        _ => None,
    }
}

fn configured_keywords_for_niri_command<'a>(
    command: &NiriCommand,
    configured: &'a [Command],
    defaults: &[Command],
) -> Option<&'a [String]> {
    let key = configured_key_for_niri_command(command)?;
    let configured = configured.iter().find(|command| command.key == key)?;
    let default = defaults.iter().find(|command| command.key == key)?;
    (configured.name == default.name
        && configured.run == default.run
        && configured.keywords != default.keywords)
        .then_some(configured.keywords.as_slice())
}

/// Apps, then windows, then commands. Commands that act on a window are
/// listed only when a window has focus.
pub fn entries(
    apps: &[App],
    windows: &[Window],
    commands: &[Command],
    clipboard: bool,
    niri_available: bool,
    focused_window: Option<FocusedWindowSnapshot>,
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
    let window_id = focused_window.map(|window| window.id);
    let is_floating = focused_window.is_some_and(|window| window.is_floating);
    let default_commands = crate::commands::resolve(&Default::default());
    let niri_entries: Vec<_> = if niri_available {
        NIRI_COMMANDS
            .iter()
            .copied()
            .filter(|command| {
                configured_key_for_niri_command(command)
                    .is_none_or(|key| commands.iter().any(|configured| configured.key == key))
            })
            .filter(|command| command.scope == CommandScope::Overview || window_id.is_some())
            .map(|command| {
                let name = command.name(is_floating);
                let secondary: Vec<String> =
                    configured_keywords_for_niri_command(&command, commands, &default_commands)
                        .map_or_else(
                            || {
                                command
                                    .keywords
                                    .iter()
                                    .map(|keyword| (*keyword).into())
                                    .collect()
                            },
                            |keywords| keywords.to_vec(),
                        );
                Entry {
                    item: Item::NiriCommand {
                        command,
                        window_id,
                        is_floating,
                    },
                    candidate: Candidate {
                        name: name.into(),
                        secondary,
                        key: Some(command.key.into()),
                    },
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let command_entries: Vec<_> = commands
        .iter()
        .filter(|c| focused.is_some() || !c.needs_window())
        .filter(|configured| {
            let Some(default) = default_commands
                .iter()
                .find(|default| default.key == configured.key)
            else {
                return true;
            };
            if configured.name != default.name || configured.run != default.run {
                return true;
            }
            !niri_entries.iter().any(|entry| {
                matches!(
                    &entry.item,
                    Item::NiriCommand { command: native, .. }
                        if configured_key_for_niri_command(native)
                            == Some(configured.key.as_str())
                )
            })
        })
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
        })
        .collect();
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
        .chain(niri_entries)
        .chain(clipboard_entry)
        .chain([emoji_entry])
        .collect()
}

impl Item {
    pub fn kind_of(&self) -> Kind {
        match self {
            Self::App(_) => Kind::App,
            Self::Window { .. } => Kind::Window,
            Self::System(_)
            | Self::Command { .. }
            | Self::NiriCommand { .. }
            | Self::Clipboard
            | Self::Emoji => Kind::Command,
        }
    }

    pub fn title(&self, apps: &[App]) -> String {
        match self {
            Self::App(i) => apps[*i].name.clone(),
            Self::Window { window, .. } => window.title.clone(),
            Self::System(cmd) => cmd.name().into(),
            Self::NiriCommand {
                command,
                is_floating,
                ..
            } => command.name(*is_floating).into(),
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
            Self::System(_) | Self::NiriCommand { .. } => None,
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
            Self::System(_)
            | Self::Command { .. }
            | Self::NiriCommand { .. }
            | Self::Clipboard
            | Self::Emoji => "Command",
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
            Self::NiriCommand { .. } => ItemIcon::Symbol(Symbol::Window),
            Self::Clipboard => ItemIcon::Symbol(Symbol::Clipboard),
            Self::Emoji => ItemIcon::Symbol(Symbol::Emoji),
        }
    }

    /// The verb for `enter`, shown in the footer.
    pub fn verb(&self) -> &'static str {
        match self {
            Self::App(_) => "Open",
            Self::Window { .. } => "Switch to",
            Self::System(_) | Self::Command { .. } | Self::NiriCommand { .. } => "Run",
            Self::Clipboard | Self::Emoji => "Open",
        }
    }

    pub fn history_key(&self, apps: &[App]) -> Option<String> {
        match self {
            Self::App(i) => Some(apps[*i].id.clone()),
            Self::Window { .. } => None,
            Self::System(cmd) => Some(cmd.key().into()),
            Self::Command { command, .. } => Some(command.history_key()),
            Self::NiriCommand { command, .. } => Some(command.key.into()),
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
            Self::Window { window, app } => {
                if let Some(a) = app {
                    actions.push(ActionDef::new(
                        Action::NewInstance,
                        &format!("Open New {}", apps[*a].name),
                    ));
                }
                actions.push(ActionDef::new(Action::CloseWindow, "Close Window"));
                actions.push(ActionDef::new(Action::CopyTitle, "Copy Title"));
                actions.push(ActionDef::new(
                    Action::ToggleFullscreen,
                    "Toggle Fullscreen",
                ));
                actions.push(ActionDef::new(
                    Action::ToggleWindowedFullscreen,
                    "Toggle Windowed Fullscreen",
                ));
                actions.push(ActionDef::new(
                    Action::ToggleFloating,
                    if window.is_floating {
                        "Tile Window"
                    } else {
                        "Float Window"
                    },
                ));
                actions.push(ActionDef::new(Action::CenterWindow, "Center Window"));
                actions.extend([
                    ActionDef::new(Action::SetWidthThird, "Set Width 1/3"),
                    ActionDef::new(Action::SetWidthHalf, "Set Width 1/2"),
                    ActionDef::new(Action::SetWidthTwoThirds, "Set Width 2/3"),
                    ActionDef::new(Action::SetWidthFull, "Set Width Full"),
                ]);
                actions.push(ActionDef::new(
                    Action::MoveToWorkspace,
                    "Move to Workspace…",
                ));
                actions.push(ActionDef::new(Action::MoveToMonitor, "Move to Monitor…"));
                actions.push(ActionDef::new(
                    Action::BringToThisWorkspace,
                    "Bring to This Workspace",
                ));
                actions.push(ActionDef::new(
                    Action::ScreenshotWindow,
                    "Screenshot Window",
                ));
                for (n, command) in commands.iter().enumerate() {
                    // Such as a configured Close Window.
                    if command.needs_window() && !actions.iter().any(|a| a.label == command.name) {
                        actions.push(ActionDef::new(Action::Command(n), &command.name));
                    }
                }
            }
            Self::System(_)
            | Self::Command { .. }
            | Self::NiriCommand { .. }
            | Self::Clipboard
            | Self::Emoji => {}
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
    ToggleFullscreen,
    ToggleWindowedFullscreen,
    ToggleFloating,
    CenterWindow,
    SetWidthThird,
    SetWidthHalf,
    SetWidthTwoThirds,
    SetWidthFull,
    MoveToWorkspace,
    MoveToMonitor,
    BringToThisWorkspace,
    ScreenshotWindow,
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

    fn window_with_id(app_id: &str, id: u64) -> Window {
        Window {
            id,
            title: "README.md".into(),
            app_id: Some(app_id.into()),
            workspace: Some("2".into()),
            is_focused: false,
            is_floating: false,
        }
    }

    fn window(app_id: &str) -> Window {
        window_with_id(app_id, 7)
    }

    #[test]
    fn app_activation_chooses_existing_or_launches_when_requested() {
        let app = app("com.mitchellh.ghostty.desktop", "Ghostty");
        // niri supplies windows most-recently-focused first.
        let windows = vec![
            window_with_id("other", 10),
            window_with_id("com.mitchellh.ghostty", 9),
            window_with_id("com.mitchellh.ghostty", 8),
        ];
        assert_eq!(
            choose_app_open(
                &app,
                &windows,
                AppLaunchPolicy::PreferExisting,
                AppLaunchIntent::Default
            ),
            AppOpen::FocusWindow(9)
        );
        assert_eq!(
            choose_app_open(
                &app,
                &windows,
                AppLaunchPolicy::AlwaysLaunch,
                AppLaunchIntent::Default
            ),
            AppOpen::Launch
        );
        assert_eq!(
            choose_app_open(
                &app,
                &windows,
                AppLaunchPolicy::PreferExisting,
                AppLaunchIntent::NewInstance
            ),
            AppOpen::Launch
        );
        assert_eq!(
            choose_app_open(
                &app,
                &windows[..1],
                AppLaunchPolicy::PreferExisting,
                AppLaunchIntent::Default
            ),
            AppOpen::Launch
        );
    }

    #[test]
    fn windows_find_their_app() {
        let apps = vec![app("com.mitchellh.ghostty.desktop", "Ghostty")];
        let entries = entries(
            &apps,
            &[window("com.mitchellh.ghostty"), window("unknown")],
            &[],
            false,
            false,
            None,
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
            entries(&apps, windows, &commands, false, false, None)
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
        let entries = entries(&apps, &[focused], &commands, false, false, None);
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
    fn niri_commands_are_gated_by_compositor_and_focused_window() {
        let captured = Some(FocusedWindowSnapshot {
            id: 41,
            is_floating: false,
        });
        let on_other_compositor = entries(&[], &[], &[], false, false, captured);
        assert!(
            on_other_compositor
                .iter()
                .all(|entry| !matches!(entry.item, Item::NiriCommand { .. }))
        );

        let no_focused_window = entries(&[], &[], &[], false, true, None);
        let names: Vec<_> = no_focused_window
            .iter()
            .filter_map(|entry| match &entry.item {
                Item::NiriCommand { .. } => Some(entry.item.title(&[])),
                _ => None,
            })
            .collect();
        assert_eq!(names, ["Open Overview"]);

        let focused = Some(FocusedWindowSnapshot {
            id: 41,
            is_floating: true,
        });
        let with_focus = entries(&[], &[], &[], false, true, focused);
        let commands: Vec<_> = with_focus
            .iter()
            .filter(|entry| matches!(entry.item, Item::NiriCommand { .. }))
            .collect();
        let names: Vec<_> = commands.iter().map(|entry| entry.item.title(&[])).collect();
        assert_eq!(names.len(), 13);
        assert!(names.contains(&"Tile Window".to_owned()));
        assert!(!names.contains(&"Float Window".to_owned()));
        let floating = commands
            .iter()
            .find(|entry| entry.item.title(&[]) == "Tile Window")
            .unwrap();
        assert!(matches!(
            floating.item,
            Item::NiriCommand {
                window_id: Some(41),
                is_floating: true,
                ..
            }
        ));
        assert!(floating.candidate.secondary.contains(&"float".into()));
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
            &labels(&w, &history)[..16],
            [
                "Switch to",
                "Open New A",
                "Close Window",
                "Copy Title",
                "Toggle Fullscreen",
                "Toggle Windowed Fullscreen",
                "Float Window",
                "Center Window",
                "Set Width 1/3",
                "Set Width 1/2",
                "Set Width 2/3",
                "Set Width Full",
                "Move to Workspace…",
                "Move to Monitor…",
                "Bring to This Workspace",
                "Screenshot Window",
            ]
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

    #[test]
    fn niri_window_actions_are_available_and_floating_label_tracks_state() {
        let apps = vec![app("a.desktop", "A")];
        let history = History::default();
        let labels = |window: Window| {
            Item::Window {
                window,
                app: Some(0),
            }
            .actions(&apps, &[], &history)
            .into_iter()
            .map(|a| a.label)
            .collect::<Vec<_>>()
        };

        let expected = [
            "Switch to",
            "Open New A",
            "Close Window",
            "Copy Title",
            "Toggle Fullscreen",
            "Toggle Windowed Fullscreen",
            "Float Window",
            "Center Window",
            "Set Width 1/3",
            "Set Width 1/2",
            "Set Width 2/3",
            "Set Width Full",
            "Move to Workspace…",
            "Move to Monitor…",
            "Bring to This Workspace",
            "Screenshot Window",
        ];
        assert_eq!(labels(window("a")), expected);

        let mut floating = window("a");
        floating.is_floating = true;
        assert_eq!(labels(floating)[6], "Tile Window");
    }
}
