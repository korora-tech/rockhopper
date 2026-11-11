//! niri, over `$NIRI_SOCKET`: one JSON request line, one JSON reply line.
//! <https://yalter.github.io/niri/niri_ipc/>

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    BuiltinCommand, Compositor, FocusedWindow, Label, Op, Output, Scope, Window, Workspace,
    exchange,
};

pub struct Niri;

fn socket_path() -> io::Result<PathBuf> {
    std::env::var_os("NIRI_SOCKET")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("NIRI_SOCKET is not set"))
}

fn request_result(request: &Value) -> io::Result<Value> {
    exchange(&socket_path()?, request)
}

fn request(request: &Value) -> Option<Value> {
    request_result(request).ok()
}

fn parse_id(id: &str) -> io::Result<u64> {
    id.parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("not a niri id: {id}")))
}

fn focused_window_from_reply(reply: Value) -> io::Result<Option<FocusedWindow>> {
    let windows = reply
        .get("Ok")
        .and_then(|ok| ok.get("Windows"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            if reply.get("Err").is_some() {
                io::Error::other(format!("niri replied {reply}"))
            } else {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("niri returned an invalid Windows reply: {reply}"),
                )
            }
        })?;

    let mut focused = None;
    for window in windows {
        let is_focused = window
            .get("is_focused")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("niri returned a window without a valid is_focused field: {window}"),
                )
            })?;
        if !is_focused {
            continue;
        }
        let id = window.get("id").and_then(Value::as_u64).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("niri returned a focused window without a valid id: {window}"),
            )
        })?;
        let is_floating = match window.get("is_floating") {
            None => false,
            Some(value) => value.as_bool().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "niri returned a focused window without a valid is_floating field: {window}"
                    ),
                )
            })?,
        };
        if focused
            .replace(FocusedWindow {
                id: id.to_string(),
                is_floating,
            })
            .is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "niri returned more than one focused window",
            ));
        }
    }
    Ok(focused)
}

#[derive(Deserialize)]
struct RawWindow {
    id: u64,
    title: Option<String>,
    app_id: Option<String>,
    workspace_id: Option<u64>,
    #[serde(default)]
    is_focused: bool,
    #[serde(default)]
    is_floating: bool,
    focus_timestamp: Option<Timestamp>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, PartialOrd)]
struct Timestamp {
    secs: u64,
    nanos: u32,
}

#[derive(Deserialize)]
struct RawWorkspace {
    id: u64,
    idx: u64,
    name: Option<String>,
    output: Option<String>,
    #[serde(default)]
    is_focused: bool,
}

fn workspaces_from_reply(reply: Value) -> Vec<Workspace> {
    serde_json::from_value::<Vec<RawWorkspace>>(reply["Ok"]["Workspaces"].clone())
        .unwrap_or_default()
        .into_iter()
        .map(|w| Workspace {
            id: w.id.to_string(),
            position: w.idx,
            name: w.name,
            output: w.output,
            is_focused: w.is_focused,
        })
        .collect()
}

fn outputs_from_reply(reply: Value) -> Vec<Output> {
    let Some(outputs) = reply["Ok"]["Outputs"].as_object() else {
        return Vec::new();
    };
    let mut outputs: Vec<_> = outputs
        .keys()
        .cloned()
        .map(|name| Output { name })
        .collect();
    outputs.sort_by(|a, b| a.name.cmp(&b.name));
    outputs
}

fn windows_from_reply(reply: Value, workspaces: Vec<Workspace>) -> Vec<Window> {
    let Ok(mut raw) = serde_json::from_value::<Vec<RawWindow>>(reply["Ok"]["Windows"].clone())
    else {
        return Vec::new();
    };
    let names: HashMap<u64, String> = workspaces
        .into_iter()
        .filter_map(|w| {
            Some((
                w.id.parse().ok()?,
                w.name.unwrap_or_else(|| w.position.to_string()),
            ))
        })
        .collect();
    raw.sort_by(|a, b| {
        b.focus_timestamp
            .partial_cmp(&a.focus_timestamp)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    raw.into_iter()
        .map(|w| Window {
            id: w.id.to_string(),
            title: w
                .title
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "Untitled".into()),
            app_id: w.app_id,
            workspace: w.workspace_id.and_then(|id| names.get(&id).cloned()),
            is_focused: w.is_focused,
            is_floating: w.is_floating,
        })
        .collect()
}

/// `{"Action": {name: fields}}`, the shape of every niri action request.
fn action(name: &str, fields: Value) -> Value {
    json!({ "Action": { name: fields } })
}

/// The request for `op` on the window `id`, or `None` when niri has no
/// window-addressed form of it.
fn window_request(id: u64, op: Op) -> Option<Value> {
    Some(match op {
        Op::ToggleFullscreen => action("FullscreenWindow", json!({ "id": id })),
        Op::ToggleWindowedFullscreen => action("ToggleWindowedFullscreen", json!({ "id": id })),
        Op::ToggleFloating => action("ToggleWindowFloating", json!({ "id": id })),
        Op::Center => action("CenterWindow", json!({ "id": id })),
        Op::SetWidth(width) => action(
            "SetWindowWidth",
            json!({ "id": id, "change": { "SetProportion": width } }),
        ),
        Op::Screenshot => action(
            "ScreenshotWindow",
            json!({ "id": id, "write_to_disk": true, "show_pointer": false, "path": null }),
        ),
        Op::Close => action("CloseWindow", json!({ "id": id })),
        _ => return None,
    })
}

/// The request for `op` on the focused window or column, or `None` when niri
/// has no such form of it.
fn focused_request(op: Op) -> Option<Value> {
    Some(match op {
        Op::SetWidth(width) => action(
            "SetColumnWidth",
            json!({ "change": { "SetProportion": width } }),
        ),
        Op::MaximizeColumn => action("MaximizeColumn", json!({})),
        Op::CenterColumn => action("CenterColumn", json!({})),
        Op::ToggleTabbedColumn => action("ToggleColumnTabbedDisplay", json!({})),
        Op::MoveToNextMonitor => action("MoveWindowToMonitorNext", json!({})),
        Op::MoveToWorkspaceUp => action("MoveWindowToWorkspaceUp", json!({ "focus": true })),
        Op::MoveToWorkspaceDown => action("MoveWindowToWorkspaceDown", json!({ "focus": true })),
        Op::OpenOverview => action("OpenOverview", json!({})),
        _ => return None,
    })
}

fn send_action(value: &Value) -> io::Result<()> {
    let reply = request_result(value)?;
    if reply.get("Ok").is_some() {
        Ok(())
    } else {
        Err(io::Error::other(format!("niri replied {reply}")))
    }
}

/// The searchable command list. Keys stay as they were, so `commands:`
/// overrides written for the shell defaults still apply.
pub const BUILTIN_COMMANDS: &[BuiltinCommand] = &[
    BuiltinCommand {
        key: "niri:toggle-fullscreen",
        config_key: Some("niri-fullscreen"),
        label: Label::Static("Toggle Fullscreen"),
        keywords: &["fullscreen", "full screen", "toggle fullscreen", "window"],
        scope: Scope::Window,
        op: Op::ToggleFullscreen,
    },
    BuiltinCommand {
        key: "niri:toggle-floating",
        config_key: Some("niri-floating"),
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
        key: "niri:center-window",
        config_key: Some("niri-center"),
        label: Label::Static("Center Window"),
        keywords: &["center", "window", "position"],
        scope: Scope::Window,
        op: Op::Center,
    },
    BuiltinCommand {
        key: "niri:maximize-column",
        config_key: None,
        label: Label::Static("Maximize Column"),
        keywords: &["maximize", "column", "full width", "expand"],
        scope: Scope::Focused,
        op: Op::MaximizeColumn,
    },
    BuiltinCommand {
        key: "niri:column-width-third",
        config_key: None,
        label: Label::Static("Column Width 1/3"),
        keywords: &["column", "width", "third", "one third", "resize"],
        scope: Scope::Focused,
        op: Op::SetWidth(1.0 / 3.0),
    },
    BuiltinCommand {
        key: "niri:column-width-half",
        config_key: None,
        label: Label::Static("Column Width 1/2"),
        keywords: &["column", "width", "half", "resize"],
        scope: Scope::Focused,
        op: Op::SetWidth(0.5),
    },
    BuiltinCommand {
        key: "niri:column-width-two-thirds",
        config_key: None,
        label: Label::Static("Column Width 2/3"),
        keywords: &["column", "width", "two thirds", "resize"],
        scope: Scope::Focused,
        op: Op::SetWidth(2.0 / 3.0),
    },
    BuiltinCommand {
        key: "niri:center-column",
        config_key: None,
        label: Label::Static("Center Column"),
        keywords: &["center", "column", "position"],
        scope: Scope::Focused,
        op: Op::CenterColumn,
    },
    BuiltinCommand {
        key: "niri:toggle-tabbed-column",
        config_key: None,
        label: Label::Static("Toggle Tabbed Column"),
        keywords: &["tabbed", "tabs", "column", "display"],
        scope: Scope::Focused,
        op: Op::ToggleTabbedColumn,
    },
    BuiltinCommand {
        key: "niri:next-monitor",
        config_key: None,
        label: Label::Static("Move Window to Next Monitor"),
        keywords: &["move", "window", "next", "monitor", "display", "output"],
        scope: Scope::Focused,
        op: Op::MoveToNextMonitor,
    },
    BuiltinCommand {
        key: "niri:workspace-up",
        config_key: None,
        label: Label::Static("Move Window to Workspace Up"),
        keywords: &["move", "window", "workspace", "up", "previous"],
        scope: Scope::Focused,
        op: Op::MoveToWorkspaceUp,
    },
    BuiltinCommand {
        key: "niri:workspace-down",
        config_key: None,
        label: Label::Static("Move Window to Workspace Down"),
        keywords: &["move", "window", "workspace", "down", "next"],
        scope: Scope::Focused,
        op: Op::MoveToWorkspaceDown,
    },
    BuiltinCommand {
        key: "niri:open-overview",
        config_key: None,
        label: Label::Static("Open Overview"),
        keywords: &["overview", "windows", "workspace overview"],
        scope: Scope::Overview,
        op: Op::OpenOverview,
    },
];

impl Compositor for Niri {
    fn name(&self) -> &'static str {
        "niri"
    }

    fn supports(&self, _op: Op) -> bool {
        true
    }

    fn windows(&self) -> Vec<Window> {
        let Some(reply) = request(&json!("Windows")) else {
            return Vec::new();
        };
        windows_from_reply(reply, self.workspaces())
    }

    fn workspaces(&self) -> Vec<Workspace> {
        request(&json!("Workspaces"))
            .map(workspaces_from_reply)
            .unwrap_or_default()
    }

    fn outputs(&self) -> Vec<Output> {
        request(&json!("Outputs"))
            .map(outputs_from_reply)
            .unwrap_or_default()
    }

    fn focused_window(&self) -> io::Result<Option<FocusedWindow>> {
        focused_window_from_reply(request_result(&json!("Windows"))?)
    }

    fn focused_output(&self) -> Option<String> {
        let reply = request(&json!("FocusedOutput"))?;
        reply["Ok"]["FocusedOutput"]["name"]
            .as_str()
            .map(str::to_owned)
    }

    fn focus_window(&self, id: &str) -> io::Result<()> {
        send_action(&action("FocusWindow", json!({ "id": parse_id(id)? })))
    }

    fn window_op(&self, id: &str, op: Op) -> io::Result<()> {
        let request = window_request(parse_id(id)?, op)
            .ok_or_else(|| io::Error::other("niri cannot perform this on a window"))?;
        send_action(&request)
    }

    fn focused_op(&self, op: Op) -> io::Result<()> {
        let request = focused_request(op)
            .ok_or_else(|| io::Error::other("niri cannot perform this on the focused window"))?;
        send_action(&request)
    }

    fn move_window_to_workspace(&self, id: &str, workspace: &str) -> io::Result<()> {
        send_action(&action(
            "MoveWindowToWorkspace",
            json!({
                "window_id": parse_id(id)?,
                "reference": { "Id": parse_id(workspace)? },
                "focus": true
            }),
        ))
    }

    fn move_window_to_monitor(&self, id: &str, output: &str) -> io::Result<()> {
        send_action(&action(
            "MoveWindowToMonitor",
            json!({ "id": parse_id(id)?, "output": output }),
        ))
    }

    fn log_out(&self) -> Option<io::Result<()>> {
        Some(send_action(&action(
            "Quit",
            json!({ "skip_confirmation": true }),
        )))
    }

    fn builtin_commands(&self) -> &'static [BuiltinCommand] {
        BUILTIN_COMMANDS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_management_requests_target_the_selected_window() {
        let id = 41;
        let cases: Vec<(Op, Value)> = vec![
            (
                Op::ToggleFullscreen,
                json!({ "Action": { "FullscreenWindow": { "id": id } } }),
            ),
            (
                Op::ToggleWindowedFullscreen,
                json!({ "Action": { "ToggleWindowedFullscreen": { "id": id } } }),
            ),
            (
                Op::ToggleFloating,
                json!({ "Action": { "ToggleWindowFloating": { "id": id } } }),
            ),
            (
                Op::Center,
                json!({ "Action": { "CenterWindow": { "id": id } } }),
            ),
            (
                Op::SetWidth(1.0 / 3.0),
                json!({ "Action": { "SetWindowWidth": { "id": id, "change": { "SetProportion": 1.0 / 3.0 } } } }),
            ),
            (
                Op::SetWidth(1.0),
                json!({ "Action": { "SetWindowWidth": { "id": id, "change": { "SetProportion": 1.0 } } } }),
            ),
            (
                Op::Screenshot,
                json!({ "Action": { "ScreenshotWindow": { "id": id, "write_to_disk": true, "show_pointer": false, "path": null } } }),
            ),
            (
                Op::Close,
                json!({ "Action": { "CloseWindow": { "id": id } } }),
            ),
        ];

        for (op, expected) in cases {
            assert_eq!(window_request(id, op).unwrap(), expected, "{op:?}");
        }
        assert!(window_request(id, Op::MaximizeColumn).is_none());
    }

    #[test]
    fn workspace_and_output_queries_expose_picker_labels() {
        let workspaces = workspaces_from_reply(json!({
            "Ok": { "Workspaces": [{
                "id": 17,
                "idx": 2,
                "name": "web",
                "output": "DP-1",
                "is_focused": true
            }] }
        }));
        assert_eq!(
            workspaces,
            vec![Workspace {
                id: "17".into(),
                position: 2,
                name: Some("web".into()),
                output: Some("DP-1".into()),
                is_focused: true,
            }]
        );

        let outputs = outputs_from_reply(json!({
            "Ok": { "Outputs": {
                "DP-1": { "name": "DP-1" },
                "eDP-1": { "name": "eDP-1" }
            } }
        }));
        assert_eq!(
            outputs
                .iter()
                .map(|output| output.name.as_str())
                .collect::<Vec<_>>(),
            ["DP-1", "eDP-1"]
        );
    }

    #[test]
    fn windows_take_their_workspace_name_from_the_workspace_list() {
        let workspaces = vec![Workspace {
            id: "17".into(),
            position: 2,
            name: Some("web".into()),
            output: None,
            is_focused: false,
        }];
        let windows = windows_from_reply(
            json!({ "Ok": { "Windows": [
                { "id": 5, "title": "", "workspace_id": 17, "is_focused": true, "is_floating": true }
            ] } }),
            workspaces,
        );
        assert_eq!(
            windows,
            vec![Window {
                id: "5".into(),
                title: "Untitled".into(),
                app_id: None,
                workspace: Some("web".into()),
                is_focused: true,
                is_floating: true,
            }]
        );
    }

    #[test]
    fn captures_the_focused_window_snapshot_from_the_open_reply() {
        let reply = json!({
            "Ok": { "Windows": [
                { "id": 17, "is_focused": false },
                { "id": 41, "is_focused": true, "is_floating": true }
            ] }
        });
        assert_eq!(
            focused_window_from_reply(reply).unwrap(),
            Some(FocusedWindow {
                id: "41".into(),
                is_floating: true,
            })
        );

        let no_focus = json!({ "Ok": { "Windows": [{ "id": 17, "is_focused": false }] } });
        assert_eq!(focused_window_from_reply(no_focus).unwrap(), None);
        assert!(focused_window_from_reply(json!({ "Err": "niri unavailable" })).is_err());
    }

    #[test]
    fn focused_commands_build_the_expected_requests() {
        let cases = [
            (
                "niri:maximize-column",
                json!({ "Action": { "MaximizeColumn": {} } }),
            ),
            (
                "niri:column-width-third",
                json!({ "Action": { "SetColumnWidth": { "change": { "SetProportion": 1.0 / 3.0 } } } }),
            ),
            (
                "niri:column-width-half",
                json!({ "Action": { "SetColumnWidth": { "change": { "SetProportion": 0.5 } } } }),
            ),
            (
                "niri:column-width-two-thirds",
                json!({ "Action": { "SetColumnWidth": { "change": { "SetProportion": 2.0 / 3.0 } } } }),
            ),
            (
                "niri:center-column",
                json!({ "Action": { "CenterColumn": {} } }),
            ),
            (
                "niri:toggle-tabbed-column",
                json!({ "Action": { "ToggleColumnTabbedDisplay": {} } }),
            ),
            (
                "niri:next-monitor",
                json!({ "Action": { "MoveWindowToMonitorNext": {} } }),
            ),
            (
                "niri:workspace-up",
                json!({ "Action": { "MoveWindowToWorkspaceUp": { "focus": true } } }),
            ),
            (
                "niri:workspace-down",
                json!({ "Action": { "MoveWindowToWorkspaceDown": { "focus": true } } }),
            ),
            (
                "niri:open-overview",
                json!({ "Action": { "OpenOverview": {} } }),
            ),
        ];

        for (key, expected) in cases {
            let command = BUILTIN_COMMANDS
                .iter()
                .find(|command| command.key == key)
                .unwrap_or_else(|| panic!("missing command {key}"));
            assert_eq!(focused_request(command.op).unwrap(), expected, "{key}");
        }
    }

    #[test]
    fn every_window_scoped_builtin_has_a_window_request() {
        for command in BUILTIN_COMMANDS.iter().filter(|c| c.scope == Scope::Window) {
            assert!(window_request(1, command.op).is_some(), "{}", command.key);
        }
    }
}
