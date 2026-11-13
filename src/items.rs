//! Everything the launcher can list, and what can be done with each.

use std::path::PathBuf;

use crate::apps::App;
use crate::commands::Command;
use crate::compositor::{BuiltinCommand, Compositor, FocusedWindow, Op, Scope, Window, WindowId};
use crate::history::History;
use crate::search::Candidate;
use crate::system::SystemCommand;
use crate::systemd::{Unit, UnitOp};

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
    /// A built-in command, bound to the window focused when the launcher
    /// opened.
    Builtin {
        command: BuiltinCommand,
        window_id: Option<WindowId>,
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
    /// A systemd service.
    Service(Unit),
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
    Service,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppOpen {
    FocusWindow(WindowId),
    Launch,
}

/// Choose how to activate an app. `windows` is most recently focused first.
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
        .map_or(AppOpen::Launch, |window| {
            AppOpen::FocusWindow(window.id.clone())
        })
}

/// What a search scope lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    App,
    Window,
    Command,
    /// Listed only under the services prefix, never in the main search.
    Service,
}

fn configured_key_for_builtin(command: &BuiltinCommand) -> Option<&'static str> {
    command.config_key
}

fn configured_keywords_for_builtin<'a>(
    command: &BuiltinCommand,
    configured: &'a [Command],
    defaults: &[Command],
) -> Option<&'a [String]> {
    let key = configured_key_for_builtin(command)?;
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
    compositor: &dyn Compositor,
    focused_window: Option<FocusedWindow>,
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
    let window_id = focused_window.as_ref().map(|window| window.id.clone());
    let is_floating = focused_window
        .as_ref()
        .is_some_and(|window| window.is_floating);
    let default_commands = crate::commands::resolve(&Default::default(), compositor.name());
    let builtin_entries: Vec<_> = compositor
        .builtin_commands()
        .iter()
        .copied()
        .filter(|command| compositor.supports(command.op))
        .filter(|command| {
            configured_key_for_builtin(command)
                .is_none_or(|key| commands.iter().any(|configured| configured.key == key))
        })
        .filter(|command| command.scope == Scope::Overview || window_id.is_some())
        .map(|command| {
            let name = command.name(is_floating);
            let secondary: Vec<String> =
                configured_keywords_for_builtin(&command, commands, &default_commands).map_or_else(
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
                item: Item::Builtin {
                    command,
                    window_id: window_id.clone(),
                    is_floating,
                },
                candidate: Candidate {
                    name: name.into(),
                    secondary,
                    key: Some(command.key.into()),
                },
            }
        })
        .collect();
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
            !builtin_entries.iter().any(|entry| {
                matches!(
                    &entry.item,
                    Item::Builtin { command: native, .. }
                        if configured_key_for_builtin(native)
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
        .chain(builtin_entries)
        .chain(clipboard_entry)
        .chain([emoji_entry])
        .collect()
}

/// One entry per systemd service.
pub fn unit_entries(units: &[Unit]) -> impl Iterator<Item = Entry> + '_ {
    units.iter().map(|unit| Entry {
        item: Item::Service(unit.clone()),
        candidate: Candidate {
            name: unit.short_name().to_owned(),
            secondary: vec![
                unit.name.clone(),
                unit.description.clone(),
                unit.bus.label().to_owned(),
            ],
            key: Some(unit_history_key(unit)),
        },
    })
}

/// What `action` does to `unit`, when it changes the unit's state.
pub fn unit_op(unit: &Unit, action: Action) -> Option<UnitOp> {
    match action {
        Action::Primary if unit.is_running() => Some(UnitOp::Stop),
        Action::Primary => Some(UnitOp::Start),
        Action::Restart => Some(UnitOp::Restart),
        _ => None,
    }
}

fn unit_history_key(unit: &Unit) -> String {
    format!("systemd:{}:{}", unit.bus.label(), unit.name)
}

impl Item {
    pub fn kind_of(&self) -> Kind {
        match self {
            Self::App(_) => Kind::App,
            Self::Window { .. } => Kind::Window,
            Self::Service(_) => Kind::Service,
            Self::System(_)
            | Self::Command { .. }
            | Self::Builtin { .. }
            | Self::Clipboard
            | Self::Emoji => Kind::Command,
        }
    }

    pub fn title(&self, apps: &[App]) -> String {
        match self {
            Self::App(i) => apps[*i].name.clone(),
            Self::Window { window, .. } => window.title.clone(),
            Self::System(cmd) => cmd.name().into(),
            Self::Builtin {
                command,
                is_floating,
                ..
            } => command.name(*is_floating).into(),
            Self::Command { command, .. } => command.name.clone(),
            Self::Clipboard => "Clipboard History".into(),
            Self::Emoji => "Search Emoji".into(),
            Self::Service(unit) => unit.short_name().into(),
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
            Self::System(_) | Self::Builtin { .. } => None,
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
            Self::Service(unit) => Some(unit.status_line()),
        }
    }

    /// The kind shown at the right of the row.
    pub fn kind(&self, apps: &[App]) -> &'static str {
        match self {
            Self::App(i) if apps[*i].terminal => "Terminal",
            Self::App(_) => "Application",
            Self::Window { .. } => "Window",
            Self::Service(_) => "Service",
            Self::System(_)
            | Self::Command { .. }
            | Self::Builtin { .. }
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
            Self::Builtin { .. } => ItemIcon::Symbol(Symbol::Window),
            Self::Clipboard => ItemIcon::Symbol(Symbol::Clipboard),
            Self::Emoji => ItemIcon::Symbol(Symbol::Emoji),
            Self::Service(_) => ItemIcon::Symbol(Symbol::Service),
        }
    }

    /// The verb for `enter`, shown in the footer.
    pub fn verb(&self) -> &'static str {
        match self {
            Self::App(_) => "Open",
            Self::Window { .. } => "Switch to",
            Self::System(_) | Self::Command { .. } | Self::Builtin { .. } => "Run",
            Self::Clipboard | Self::Emoji => "Open",
            Self::Service(unit) if unit.is_running() => "Stop",
            Self::Service(_) => "Start",
        }
    }

    pub fn history_key(&self, apps: &[App]) -> Option<String> {
        match self {
            Self::App(i) => Some(apps[*i].id.clone()),
            Self::Window { .. } => None,
            Self::System(cmd) => Some(cmd.key().into()),
            Self::Command { command, .. } => Some(command.history_key()),
            Self::Builtin { command, .. } => Some(command.key.into()),
            Self::Clipboard => Some("rockhopper:clipboard".into()),
            Self::Emoji => Some("rockhopper:emoji".into()),
            Self::Service(unit) => Some(unit_history_key(unit)),
        }
    }

    /// Everything that can be done with this item, the `enter` action first.
    /// A window also offers every command that acts on a window.
    pub fn actions(
        &self,
        apps: &[App],
        commands: &[Command],
        history: &History,
        compositor: &dyn Compositor,
    ) -> Vec<ActionDef> {
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
                let supports = |op| compositor.supports(op);
                if supports(Op::ToggleFullscreen) {
                    actions.push(ActionDef::new(
                        Action::ToggleFullscreen,
                        "Toggle Fullscreen",
                    ));
                }
                if supports(Op::ToggleWindowedFullscreen) {
                    actions.push(ActionDef::new(
                        Action::ToggleWindowedFullscreen,
                        "Toggle Windowed Fullscreen",
                    ));
                }
                if supports(Op::ToggleFloating) {
                    actions.push(ActionDef::new(
                        Action::ToggleFloating,
                        if window.is_floating {
                            "Tile Window"
                        } else {
                            "Float Window"
                        },
                    ));
                }
                if supports(Op::Center) {
                    actions.push(ActionDef::new(Action::CenterWindow, "Center Window"));
                }
                if supports(Op::SetWidth(1.0)) {
                    actions.extend([
                        ActionDef::new(Action::SetWidthThird, "Set Width 1/3"),
                        ActionDef::new(Action::SetWidthHalf, "Set Width 1/2"),
                        ActionDef::new(Action::SetWidthTwoThirds, "Set Width 2/3"),
                        ActionDef::new(Action::SetWidthFull, "Set Width Full"),
                    ]);
                }
                if compositor.can_move_to_workspace() {
                    actions.push(ActionDef::new(
                        Action::MoveToWorkspace,
                        "Move to Workspace…",
                    ));
                }
                if compositor.can_move_to_output() {
                    actions.push(ActionDef::new(Action::MoveToMonitor, "Move to Monitor…"));
                }
                if compositor.can_move_to_workspace() {
                    actions.push(ActionDef::new(
                        Action::BringToThisWorkspace,
                        "Bring to This Workspace",
                    ));
                }
                if supports(Op::Screenshot) {
                    actions.push(ActionDef::new(
                        Action::ScreenshotWindow,
                        "Screenshot Window",
                    ));
                }
                for (n, command) in commands.iter().enumerate() {
                    // Such as a configured Close Window.
                    if command.needs_window() && !actions.iter().any(|a| a.label == command.name) {
                        actions.push(ActionDef::new(Action::Command(n), &command.name));
                    }
                }
            }
            Self::Service(_) => {
                actions.extend([
                    ActionDef::new(Action::Restart, "Restart"),
                    ActionDef::new(Action::ShowLogs, "Show Logs"),
                    ActionDef::new(Action::CopyName, "Copy Unit Name"),
                ]);
            }
            Self::System(_)
            | Self::Command { .. }
            | Self::Builtin { .. }
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
    /// Restart a service.
    Restart,
    /// Follow a service's journal in a terminal.
    ShowLogs,
    CopyName,
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
    use crate::compositor::{Unknown, niri::Niri};

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

    fn window_with_id(app_id: &str, id: &str) -> Window {
        Window {
            id: id.into(),
            title: "README.md".into(),
            app_id: Some(app_id.into()),
            workspace: Some("2".into()),
            is_focused: false,
            is_floating: false,
        }
    }

    fn window(app_id: &str) -> Window {
        window_with_id(app_id, "7")
    }

    #[test]
    fn app_activation_chooses_existing_or_launches_when_requested() {
        let app = app("com.mitchellh.ghostty.desktop", "Ghostty");
        // niri supplies windows most-recently-focused first.
        let windows = vec![
            window_with_id("other", "10"),
            window_with_id("com.mitchellh.ghostty", "9"),
            window_with_id("com.mitchellh.ghostty", "8"),
        ];
        assert_eq!(
            choose_app_open(
                &app,
                &windows,
                AppLaunchPolicy::PreferExisting,
                AppLaunchIntent::Default
            ),
            AppOpen::FocusWindow("9".into())
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
            &Unknown,
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
        let commands = crate::commands::resolve(&Default::default(), "niri");
        let count = |windows: &[Window]| {
            entries(&apps, windows, &commands, false, &Unknown, None)
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
        let entries = entries(&apps, &[focused], &commands, false, &Unknown, None);
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
    fn builtin_commands_are_gated_by_compositor_and_focused_window() {
        let captured = Some(FocusedWindow {
            id: "41".into(),
            is_floating: false,
        });
        let on_other_compositor = entries(&[], &[], &[], false, &Unknown, captured);
        assert!(
            on_other_compositor
                .iter()
                .all(|entry| !matches!(entry.item, Item::Builtin { .. }))
        );

        let configured = crate::commands::resolve(&Default::default(), "niri");
        let no_focused_window = entries(&[], &[], &configured, false, &Niri, None);
        let names: Vec<_> = no_focused_window
            .iter()
            .filter_map(|entry| match &entry.item {
                Item::Builtin { .. } => Some(entry.item.title(&[])),
                _ => None,
            })
            .collect();
        assert_eq!(names, ["Open Overview"]);

        let focused = Some(FocusedWindow {
            id: "41".into(),
            is_floating: true,
        });
        let with_focus = entries(&[], &[], &configured, false, &Niri, focused);
        let commands: Vec<_> = with_focus
            .iter()
            .filter(|entry| matches!(entry.item, Item::Builtin { .. }))
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
            Item::Builtin {
                window_id: Some(ref id),
                is_floating: true,
                ..
            } if id == "41"
        ));
        assert!(floating.candidate.secondary.contains(&"float".into()));
    }

    #[test]
    fn action_lists() {
        let apps = vec![app("a.desktop", "A")];
        let commands = crate::commands::resolve(&Default::default(), "niri");
        let mut history = History::default();
        let labels = |item: &Item, h: &History| -> Vec<String> {
            item.actions(&apps, &commands, h, &Niri)
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
    fn window_actions_are_available_and_floating_label_tracks_state() {
        let apps = vec![app("a.desktop", "A")];
        let history = History::default();
        let labels = |window: Window| {
            Item::Window {
                window,
                app: Some(0),
            }
            .actions(&apps, &[], &history, &Niri)
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

    #[test]
    fn services_toggle_and_offer_restart_logs_and_copy() {
        use crate::systemd::{ActiveState, Bus};
        let unit = |state| Unit {
            name: "sshd.service".into(),
            description: "OpenSSH Daemon".into(),
            bus: Bus::User,
            state,
            sub_state: "running".into(),
        };
        let running = Item::Service(unit(ActiveState::Active));
        let stopped = Item::Service(unit(ActiveState::Inactive));
        assert_eq!(running.verb(), "Stop");
        assert_eq!(stopped.verb(), "Start");
        assert_eq!(running.title(&[]), "sshd");
        assert_eq!(running.kind_of(), Kind::Service);
        let labels: Vec<_> = running
            .actions(&[], &[], &History::default(), &Unknown)
            .into_iter()
            .map(|a| a.label)
            .collect();
        assert_eq!(labels, ["Stop", "Restart", "Show Logs", "Copy Unit Name"]);
        let Item::Service(running) = running else {
            unreachable!()
        };
        assert_eq!(unit_op(&running, Action::Primary), Some(UnitOp::Stop));
        assert_eq!(unit_op(&running, Action::Restart), Some(UnitOp::Restart));
        assert_eq!(unit_op(&running, Action::ShowLogs), None);
        let entries: Vec<_> = unit_entries(std::slice::from_ref(&running)).collect();
        assert_eq!(entries[0].candidate.name, "sshd");
        assert!(
            entries[0]
                .candidate
                .secondary
                .contains(&"sshd.service".into())
        );
        assert_eq!(
            entries[0].candidate.key.as_deref(),
            Some("systemd:user:sshd.service")
        );
    }
}
