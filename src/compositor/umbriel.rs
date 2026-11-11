//! Umbriel, over its IPC socket: one JSON request line, one JSON reply line.
//! Replies are `{"ok": ...}` or `{"err": "..."}`.
//! <https://github.com/noctalia-dev/umbriel>

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

use super::{
    BuiltinCommand, Compositor, FocusedWindow, Label, Op, Output, Scope, Window, Workspace,
    exchange,
};

/// `$UMBRIEL_SOCKET`, else the path Umbriel uses when the variable is unset.
pub fn socket_path_with(var: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(path) = var("UMBRIEL_SOCKET").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let runtime = var("XDG_RUNTIME_DIR")?;
    let display = var("WAYLAND_DISPLAY")?;
    let mut name = OsString::from("umbriel-");
    name.push(display);
    name.push(".sock");
    Some(PathBuf::from(runtime).join(name))
}

pub struct Umbriel {
    socket: PathBuf,
}

impl Umbriel {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    /// Sends a query and returns the `ok` payload.
    fn query(&self, cmd: &str) -> io::Result<Value> {
        reply_payload(exchange(&self.socket, &json!({ "cmd": cmd }))?)
    }

    /// Sends a typed action, with the fields it takes beside `action`.
    fn action(&self, action: &str, fields: Value) -> io::Result<()> {
        let mut request = Map::new();
        request.insert("cmd".into(), json!("action"));
        request.insert("action".into(), json!(action));
        if let Value::Object(fields) = fields {
            request.extend(fields);
        }
        reply_payload(exchange(&self.socket, &Value::Object(request))?).map(drop)
    }
}

fn reply_payload(reply: Value) -> io::Result<Value> {
    if let Some(ok) = reply.get("ok") {
        return Ok(ok.clone());
    }
    let message = reply
        .get("err")
        .and_then(Value::as_str)
        .map_or_else(|| format!("umbriel replied {reply}"), str::to_owned);
    Err(io::Error::other(message))
}

/// Ids are strings in Umbriel's listings; numbers are accepted too.
fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn workspaces_from_payload(payload: &Value) -> Vec<Workspace> {
    payload
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| Workspace {
            id: text(&entry["id"]),
            position: entry["index"].as_u64().unwrap_or(0),
            name: entry["named"]
                .as_bool()
                .unwrap_or(false)
                .then(|| entry["name"].as_str().unwrap_or_default().to_owned()),
            output: entry["output"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            is_focused: entry["focused"].as_bool().unwrap_or(false),
        })
        .collect()
}

fn windows_from_payload(payload: &Value, workspaces: &[Workspace]) -> Vec<Window> {
    payload
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            let workspace_id = text(&entry["workspace"]);
            let workspace = workspaces
                .iter()
                .find(|w| w.id == workspace_id)
                .map(|w| w.name.clone().unwrap_or_else(|| w.position.to_string()));
            let app_id = entry["app_id"].as_str().filter(|s| !s.is_empty());
            let title = entry["title"].as_str().filter(|s| !s.is_empty());
            Window {
                id: text(&entry["id"]),
                title: title.unwrap_or("Untitled").to_owned(),
                app_id: app_id.map(str::to_owned),
                workspace,
                is_focused: entry["active"].as_bool().unwrap_or(false),
                is_floating: entry["floating"].as_bool().unwrap_or(false),
            }
        })
        .collect()
}

fn outputs_from_payload(payload: &Value) -> Vec<Output> {
    let mut outputs: Vec<Output> = payload["outputs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["name"].as_str())
        .map(|name| Output { name: name.into() })
        .collect();
    outputs.sort_by(|a, b| a.name.cmp(&b.name));
    outputs
}

/// The focused-window reply. Umbriel answers `no focused window` as an error,
/// which is an empty result rather than a failure.
fn focused_from_reply(reply: Value) -> io::Result<Option<FocusedWindow>> {
    match reply_payload(reply) {
        Ok(entry) => Ok(Some(FocusedWindow {
            id: text(&entry["id"]),
            is_floating: entry["floating"].as_bool().unwrap_or(false),
        })),
        Err(err) if err.to_string() == "no focused window" => Ok(None),
        Err(err) => Err(err),
    }
}

/// The action and its fields for a focused-window or focused-column `op`.
fn focused_action(op: Op) -> Option<(&'static str, Value)> {
    Some(match op {
        Op::ToggleFullscreen => ("window-toggle-fullscreen", json!({})),
        Op::ToggleFloating => ("window-toggle-floating", json!({})),
        Op::Center => ("window-center", json!({})),
        Op::SetWidth(fraction) => ("window-set-primary-extent", json!({ "fraction": fraction })),
        Op::MaximizeColumn => ("window-toggle-maximize", json!({})),
        Op::CenterColumn => ("column-center", json!({})),
        Op::MoveToNextMonitor => ("window-move-to-output-next", json!({})),
        Op::MoveToWorkspaceUp => ("window-move-to-workspace-previous", json!({})),
        Op::MoveToWorkspaceDown => ("window-move-to-workspace-next", json!({})),
        Op::OpenOverview => ("overview-open", json!({})),
        Op::Close => ("window-close", json!({})),
        Op::ToggleWindowedFullscreen | Op::ToggleTabbedColumn | Op::Screenshot => return None,
    })
}

/// Commands listed in the launcher. Umbriel has no shell defaults, so these
/// have no `commands:` config keys.
pub const BUILTIN_COMMANDS: &[BuiltinCommand] = &[
    BuiltinCommand {
        key: "umbriel:toggle-fullscreen",
        config_key: None,
        label: Label::Static("Toggle Fullscreen"),
        keywords: &["fullscreen", "full screen", "toggle fullscreen", "window"],
        scope: Scope::Window,
        op: Op::ToggleFullscreen,
    },
    BuiltinCommand {
        key: "umbriel:toggle-floating",
        config_key: None,
        label: Label::FloatingToggle,
        keywords: &[
            "float",
            "floating",
            "tile",
            "tiling",
            "toggle floating",
            "window",
        ],
        scope: Scope::Window,
        op: Op::ToggleFloating,
    },
    BuiltinCommand {
        key: "umbriel:center-window",
        config_key: None,
        label: Label::Static("Center Window"),
        keywords: &["center", "window", "position"],
        scope: Scope::Window,
        op: Op::Center,
    },
    BuiltinCommand {
        key: "umbriel:close-window",
        config_key: None,
        label: Label::Static("Close Window"),
        keywords: &["quit", "kill", "close"],
        scope: Scope::Window,
        op: Op::Close,
    },
    BuiltinCommand {
        key: "umbriel:maximize-column",
        config_key: None,
        label: Label::Static("Maximize Column"),
        keywords: &["maximize", "column", "full width", "expand"],
        scope: Scope::Focused,
        op: Op::MaximizeColumn,
    },
    BuiltinCommand {
        key: "umbriel:column-width-third",
        config_key: None,
        label: Label::Static("Column Width 1/3"),
        keywords: &["column", "width", "third", "one third", "resize"],
        scope: Scope::Focused,
        op: Op::SetWidth(1.0 / 3.0),
    },
    BuiltinCommand {
        key: "umbriel:column-width-half",
        config_key: None,
        label: Label::Static("Column Width 1/2"),
        keywords: &["column", "width", "half", "resize"],
        scope: Scope::Focused,
        op: Op::SetWidth(0.5),
    },
    BuiltinCommand {
        key: "umbriel:column-width-two-thirds",
        config_key: None,
        label: Label::Static("Column Width 2/3"),
        keywords: &["column", "width", "two thirds", "resize"],
        scope: Scope::Focused,
        op: Op::SetWidth(2.0 / 3.0),
    },
    BuiltinCommand {
        key: "umbriel:center-column",
        config_key: None,
        label: Label::Static("Center Column"),
        keywords: &["center", "column", "position"],
        scope: Scope::Focused,
        op: Op::CenterColumn,
    },
    BuiltinCommand {
        key: "umbriel:next-monitor",
        config_key: None,
        label: Label::Static("Move Window to Next Monitor"),
        keywords: &["move", "window", "next", "monitor", "display", "output"],
        scope: Scope::Focused,
        op: Op::MoveToNextMonitor,
    },
    BuiltinCommand {
        key: "umbriel:workspace-up",
        config_key: None,
        label: Label::Static("Move Window to Workspace Up"),
        keywords: &["move", "window", "workspace", "up", "previous"],
        scope: Scope::Focused,
        op: Op::MoveToWorkspaceUp,
    },
    BuiltinCommand {
        key: "umbriel:workspace-down",
        config_key: None,
        label: Label::Static("Move Window to Workspace Down"),
        keywords: &["move", "window", "workspace", "down", "next"],
        scope: Scope::Focused,
        op: Op::MoveToWorkspaceDown,
    },
    BuiltinCommand {
        key: "umbriel:open-overview",
        config_key: None,
        label: Label::Static("Open Overview"),
        keywords: &["overview", "windows", "workspace overview"],
        scope: Scope::Overview,
        op: Op::OpenOverview,
    },
];

impl Compositor for Umbriel {
    fn name(&self) -> &'static str {
        "umbriel"
    }

    fn supports(&self, op: Op) -> bool {
        focused_action(op).is_some()
    }

    fn windows(&self) -> Vec<Window> {
        let Ok(windows) = self.query("windows") else {
            return Vec::new();
        };
        windows_from_payload(&windows, &self.workspaces())
    }

    fn workspaces(&self) -> Vec<Workspace> {
        self.query("workspaces")
            .map(|payload| workspaces_from_payload(&payload))
            .unwrap_or_default()
    }

    fn outputs(&self) -> Vec<Output> {
        self.query("outputs")
            .map(|payload| outputs_from_payload(&payload))
            .unwrap_or_default()
    }

    fn focused_window(&self) -> io::Result<Option<FocusedWindow>> {
        focused_from_reply(exchange(&self.socket, &json!({ "cmd": "focused-window" }))?)
    }

    fn focused_output(&self) -> Option<String> {
        let payload = self.query("outputs").ok()?;
        payload["outputs"]
            .as_array()?
            .iter()
            .find(|entry| entry["focused"].as_bool() == Some(true))?["name"]
            .as_str()
            .map(str::to_owned)
    }

    fn focus_window(&self, id: &str) -> io::Result<()> {
        self.action("window-focus", json!({ "window": id }))
    }

    fn window_op(&self, id: &str, op: Op) -> io::Result<()> {
        match op {
            Op::Close => self.action("window-close", json!({ "window": id })),
            Op::ToggleFloating => self.action("window-toggle-floating", json!({ "window": id })),
            _ => {
                // The rest act on the focused window or column only.
                self.focus_window(id)?;
                self.focused_op(op)
            }
        }
    }

    fn focused_op(&self, op: Op) -> io::Result<()> {
        let (action, fields) = focused_action(op)
            .ok_or_else(|| io::Error::other("Umbriel cannot perform this action"))?;
        self.action(action, fields)
    }

    fn move_window_to_workspace(&self, id: &str, workspace: &str) -> io::Result<()> {
        self.focus_window(id)?;
        let reference = workspace
            .parse::<u64>()
            .map_or_else(|_| json!(workspace), Value::from);
        self.action(
            "window-move-to-workspace",
            json!({ "workspace": reference }),
        )
    }

    fn move_window_to_monitor(&self, _id: &str, _output: &str) -> io::Result<()> {
        Err(io::Error::other(
            "Umbriel moves windows between outputs by direction, not by name",
        ))
    }

    fn can_move_to_output(&self) -> bool {
        false
    }

    fn log_out(&self) -> Option<io::Result<()>> {
        Some(self.action("session-quit", json!({ "skip_confirmation": true })))
    }

    fn builtin_commands(&self) -> &'static [BuiltinCommand] {
        BUILTIN_COMMANDS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_listing_maps_to_windows_with_workspace_names() {
        let workspaces = workspaces_from_payload(&json!([
            { "id": 3, "name": "WEB", "named": true, "index": 1, "output": "DP-1", "focused": true }
        ]));
        let windows = windows_from_payload(
            &json!([
                { "id": "abc", "app_id": "firefox", "title": "", "workspace": 3, "active": true, "floating": false },
                { "id": "def", "app_id": "", "title": "Notes", "workspace": 9, "active": false, "floating": true }
            ]),
            &workspaces,
        );
        assert_eq!(
            windows,
            vec![
                Window {
                    id: "abc".into(),
                    title: "Untitled".into(),
                    app_id: Some("firefox".into()),
                    workspace: Some("WEB".into()),
                    is_focused: true,
                    is_floating: false,
                },
                Window {
                    id: "def".into(),
                    title: "Notes".into(),
                    app_id: None,
                    workspace: None,
                    is_focused: false,
                    is_floating: true,
                },
            ]
        );
    }

    #[test]
    fn workspaces_expose_their_index_for_moves() {
        let workspaces = workspaces_from_payload(&json!([
            { "id": 7, "name": "7", "named": false, "index": 4, "output": "DP-2", "focused": false }
        ]));
        assert_eq!(
            workspaces,
            vec![Workspace {
                id: "7".into(),
                position: 4,
                name: None,
                output: Some("DP-2".into()),
                is_focused: false,
            }]
        );
    }

    #[test]
    fn outputs_are_sorted_by_name() {
        let outputs = outputs_from_payload(&json!({ "outputs": [
            { "name": "eDP-1" }, { "name": "DP-1" }
        ] }));
        assert_eq!(
            outputs.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(),
            ["DP-1", "eDP-1"]
        );
    }

    #[test]
    fn focused_window_reply_distinguishes_none_from_failure() {
        assert_eq!(
            focused_from_reply(json!({ "ok": { "id": "abc", "floating": true } })).unwrap(),
            Some(FocusedWindow {
                id: "abc".into(),
                is_floating: true,
            })
        );
        assert_eq!(
            focused_from_reply(json!({ "err": "no focused window" })).unwrap(),
            None
        );
        assert!(focused_from_reply(json!({ "err": "boom" })).is_err());
    }

    #[test]
    fn typed_actions_use_umbriel_names_and_fields() {
        assert_eq!(
            focused_action(Op::SetWidth(0.5)),
            Some(("window-set-primary-extent", json!({ "fraction": 0.5 })))
        );
        assert_eq!(
            focused_action(Op::CenterColumn),
            Some(("column-center", json!({})))
        );
        assert_eq!(focused_action(Op::Screenshot), None);
        assert_eq!(focused_action(Op::ToggleTabbedColumn), None);
    }

    #[test]
    fn every_builtin_is_supported_by_umbriel() {
        let umbriel = Umbriel::new(PathBuf::from("/nonexistent"));
        for command in umbriel.builtin_commands() {
            assert!(umbriel.supports(command.op), "{}", command.key);
        }
    }

    #[test]
    fn replies_surface_umbriel_errors() {
        assert_eq!(reply_payload(json!({ "ok": null })).unwrap(), Value::Null);
        assert_eq!(
            reply_payload(json!({ "err": "unknown action" }))
                .unwrap_err()
                .to_string(),
            "unknown action"
        );
    }
}
