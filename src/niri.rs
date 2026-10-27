//! niri IPC over `$NIRI_SOCKET`: one JSON request line, one JSON reply line.
//! <https://yalter.github.io/niri/niri_ipc/>

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

fn request_result(request: &Value) -> io::Result<Value> {
    let path = std::env::var_os("NIRI_SOCKET")
        .ok_or_else(|| io::Error::other("NIRI_SOCKET is not set"))?;
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_millis(200)))?;
    writeln!(stream, "{request}")?;
    let mut line = String::new();
    if BufReader::new(stream).read_line(&mut line)? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "niri closed the IPC connection without replying",
        ));
    }
    serde_json::from_str(&line).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn request(request: &Value) -> Option<Value> {
    request_result(request).ok()
}

pub fn available() -> bool {
    std::env::var_os("NIRI_SOCKET").is_some()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusedWindowSnapshot {
    pub id: u64,
    pub is_floating: bool,
}

fn focused_window_from_reply(reply: Value) -> io::Result<Option<FocusedWindowSnapshot>> {
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
            .replace(FocusedWindowSnapshot { id, is_floating })
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

fn focused_window_snapshot_with(
    request: impl FnOnce(&Value) -> io::Result<Value>,
) -> io::Result<Option<FocusedWindowSnapshot>> {
    focused_window_from_reply(request(&json!("Windows"))?)
}

/// Snapshot the focused window's id and floating state when the launcher opens.
pub fn focused_window_snapshot() -> io::Result<Option<FocusedWindowSnapshot>> {
    focused_window_snapshot_with(request_result)
}

/// The currently focused window id, without querying workspace metadata.
pub fn focused_window_id() -> io::Result<Option<u64>> {
    focused_window_snapshot().map(|window| window.map(|window| window.id))
}

/// The name of the output with keyboard focus, such as `DP-1`.
pub fn focused_output() -> Option<String> {
    let reply = request(&json!("FocusedOutput"))?;
    reply["Ok"]["FocusedOutput"]["name"]
        .as_str()
        .map(str::to_owned)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub id: u64,
    pub title: String,
    pub app_id: Option<String>,
    /// The workspace's name, else its index, for display.
    pub workspace: Option<String>,
    pub is_focused: bool,
    pub is_floating: bool,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub id: u64,
    pub idx: u64,
    pub name: Option<String>,
    pub output: Option<String>,
    pub is_focused: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub name: String,
}

/// Open windows, most recently focused first. Empty when niri is not
/// running or does not answer.
pub fn windows() -> Vec<Window> {
    let Some(reply) = request(&json!("Windows")) else {
        return Vec::new();
    };
    let Ok(mut raw) = serde_json::from_value::<Vec<RawWindow>>(reply["Ok"]["Windows"].clone())
    else {
        return Vec::new();
    };
    let workspaces: HashMap<u64, String> = workspaces()
        .into_iter()
        .map(|w| (w.id, w.name.unwrap_or_else(|| w.idx.to_string())))
        .collect();
    raw.sort_by(|a, b| {
        b.focus_timestamp
            .partial_cmp(&a.focus_timestamp)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    raw.into_iter()
        .map(|w| Window {
            id: w.id,
            title: w
                .title
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "Untitled".into()),
            app_id: w.app_id,
            workspace: w.workspace_id.and_then(|id| workspaces.get(&id).cloned()),
            is_focused: w.is_focused,
            is_floating: w.is_floating,
        })
        .collect()
}

/// Workspaces, with their stable ids and display information.
pub fn workspaces() -> Vec<Workspace> {
    request(&json!("Workspaces"))
        .map(workspaces_from_reply)
        .unwrap_or_default()
}

fn workspaces_from_reply(reply: Value) -> Vec<Workspace> {
    serde_json::from_value::<Vec<RawWorkspace>>(reply["Ok"]["Workspaces"].clone())
        .unwrap_or_default()
        .into_iter()
        .map(|w| Workspace {
            id: w.id,
            idx: w.idx,
            name: w.name,
            output: w.output,
            is_focused: w.is_focused,
        })
        .collect()
}

/// Connected output names, sorted for stable presentation.
pub fn outputs() -> Vec<Output> {
    request(&json!("Outputs"))
        .map(outputs_from_reply)
        .unwrap_or_default()
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

#[derive(Clone, Debug, PartialEq)]
pub enum WindowAction {
    ToggleFullscreen,
    ToggleWindowedFullscreen,
    ToggleFloating,
    Center,
    SetWidth(f64),
    MoveToWorkspace(u64),
    MoveToMonitor(String),
    Screenshot,
}

fn window_action_request(id: u64, action: WindowAction) -> Value {
    let action = match action {
        WindowAction::ToggleFullscreen => json!({ "FullscreenWindow": { "id": id } }),
        WindowAction::ToggleWindowedFullscreen => {
            json!({ "ToggleWindowedFullscreen": { "id": id } })
        }
        WindowAction::ToggleFloating => json!({ "ToggleWindowFloating": { "id": id } }),
        WindowAction::Center => json!({ "CenterWindow": { "id": id } }),
        WindowAction::SetWidth(width) => json!({
            "SetWindowWidth": { "id": id, "change": { "SetProportion": width } }
        }),
        WindowAction::MoveToWorkspace(workspace_id) => json!({
            "MoveWindowToWorkspace": {
                "window_id": id,
                "reference": { "Id": workspace_id },
                "focus": true
            }
        }),
        WindowAction::MoveToMonitor(output) => {
            json!({ "MoveWindowToMonitor": { "id": id, "output": output } })
        }
        WindowAction::Screenshot => json!({
            "ScreenshotWindow": {
                "id": id,
                "write_to_disk": true,
                "show_pointer": false,
                "path": null
            }
        }),
    };
    json!({ "Action": action })
}

fn send_action_request(value: &Value) -> io::Result<()> {
    let reply = request_result(value)?;
    if reply.get("Ok").is_some() {
        Ok(())
    } else {
        Err(io::Error::other(format!("niri replied {reply}")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandScope {
    Window,
    Focused,
    Overview,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CommandLabel {
    Static(&'static str),
    FloatingToggle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CommandRequest {
    WindowId(&'static str),
    Unit(&'static str),
    Width(&'static str, f64),
    Bool(&'static str, &'static str, bool),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NiriCommand {
    pub key: &'static str,
    pub label: CommandLabel,
    pub keywords: &'static [&'static str],
    pub scope: CommandScope,
    request: CommandRequest,
}

impl NiriCommand {
    pub fn name(self, is_floating: bool) -> &'static str {
        match self.label {
            CommandLabel::Static(name) => name,
            CommandLabel::FloatingToggle if is_floating => "Tile Window",
            CommandLabel::FloatingToggle => "Float Window",
        }
    }

    pub fn build_request(self, window_id: Option<u64>) -> std::io::Result<Value> {
        let (name, fields) = match self.request {
            CommandRequest::WindowId(name) => {
                let id = window_id.ok_or_else(|| {
                    std::io::Error::other("the captured focused window is no longer available")
                })?;
                (name, json!({ "id": id }))
            }
            CommandRequest::Unit(name) => (name, json!({})),
            CommandRequest::Width(name, proportion) => {
                (name, json!({ "change": { "SetProportion": proportion } }))
            }
            CommandRequest::Bool(name, field, value) => (name, json!({ field: value })),
        };
        let mut action = serde_json::Map::new();
        action.insert(name.into(), fields);
        Ok(json!({ "Action": action }))
    }

    pub fn perform(self, window_id: Option<u64>) -> std::io::Result<()> {
        send_action_request(&self.build_request(window_id)?)
    }
}

/// The complete searchable command list and its IPC mapping.
/// Add new focused-window commands here; their request shape stays beside them.
pub const NIRI_COMMANDS: &[NiriCommand] = &[
    NiriCommand {
        key: "niri:toggle-fullscreen",
        label: CommandLabel::Static("Toggle Fullscreen"),
        keywords: &["fullscreen", "full screen", "toggle fullscreen", "window"],
        scope: CommandScope::Window,
        request: CommandRequest::WindowId("FullscreenWindow"),
    },
    NiriCommand {
        key: "niri:toggle-floating",
        label: CommandLabel::FloatingToggle,
        keywords: &[
            "float",
            "floating",
            "tile",
            "tiling",
            "toggle floating",
            "window",
        ],
        scope: CommandScope::Window,
        request: CommandRequest::WindowId("ToggleWindowFloating"),
    },
    NiriCommand {
        key: "niri:center-window",
        label: CommandLabel::Static("Center Window"),
        keywords: &["center", "window", "position"],
        scope: CommandScope::Window,
        request: CommandRequest::WindowId("CenterWindow"),
    },
    NiriCommand {
        key: "niri:maximize-column",
        label: CommandLabel::Static("Maximize Column"),
        keywords: &["maximize", "column", "full width", "expand"],
        scope: CommandScope::Focused,
        request: CommandRequest::Unit("MaximizeColumn"),
    },
    NiriCommand {
        key: "niri:column-width-third",
        label: CommandLabel::Static("Column Width 1/3"),
        keywords: &["column", "width", "third", "one third", "resize"],
        scope: CommandScope::Focused,
        request: CommandRequest::Width("SetColumnWidth", 1.0 / 3.0),
    },
    NiriCommand {
        key: "niri:column-width-half",
        label: CommandLabel::Static("Column Width 1/2"),
        keywords: &["column", "width", "half", "resize"],
        scope: CommandScope::Focused,
        request: CommandRequest::Width("SetColumnWidth", 0.5),
    },
    NiriCommand {
        key: "niri:column-width-two-thirds",
        label: CommandLabel::Static("Column Width 2/3"),
        keywords: &["column", "width", "two thirds", "resize"],
        scope: CommandScope::Focused,
        request: CommandRequest::Width("SetColumnWidth", 2.0 / 3.0),
    },
    NiriCommand {
        key: "niri:center-column",
        label: CommandLabel::Static("Center Column"),
        keywords: &["center", "column", "position"],
        scope: CommandScope::Focused,
        request: CommandRequest::Unit("CenterColumn"),
    },
    NiriCommand {
        key: "niri:toggle-tabbed-column",
        label: CommandLabel::Static("Toggle Tabbed Column"),
        keywords: &["tabbed", "tabs", "column", "display"],
        scope: CommandScope::Focused,
        request: CommandRequest::Unit("ToggleColumnTabbedDisplay"),
    },
    NiriCommand {
        key: "niri:next-monitor",
        label: CommandLabel::Static("Move Window to Next Monitor"),
        keywords: &["move", "window", "next", "monitor", "display", "output"],
        scope: CommandScope::Focused,
        request: CommandRequest::Unit("MoveWindowToMonitorNext"),
    },
    NiriCommand {
        key: "niri:workspace-up",
        label: CommandLabel::Static("Move Window to Workspace Up"),
        keywords: &["move", "window", "workspace", "up", "previous"],
        scope: CommandScope::Focused,
        request: CommandRequest::Bool("MoveWindowToWorkspaceUp", "focus", true),
    },
    NiriCommand {
        key: "niri:workspace-down",
        label: CommandLabel::Static("Move Window to Workspace Down"),
        keywords: &["move", "window", "workspace", "down", "next"],
        scope: CommandScope::Focused,
        request: CommandRequest::Bool("MoveWindowToWorkspaceDown", "focus", true),
    },
    NiriCommand {
        key: "niri:open-overview",
        label: CommandLabel::Static("Open Overview"),
        keywords: &["overview", "windows", "workspace overview"],
        scope: CommandScope::Overview,
        request: CommandRequest::Unit("OpenOverview"),
    },
];

fn action(action: Value) -> std::io::Result<()> {
    send_action_request(&json!({ "Action": action }))
}

pub fn perform_window_action(id: u64, action: WindowAction) -> std::io::Result<()> {
    send_action_request(&window_action_request(id, action))
}

pub fn move_window_to_workspace(id: u64, workspace_id: u64) -> std::io::Result<()> {
    perform_window_action(id, WindowAction::MoveToWorkspace(workspace_id))
}

pub fn move_window_to_monitor(id: u64, output: String) -> std::io::Result<()> {
    perform_window_action(id, WindowAction::MoveToMonitor(output))
}

pub fn focus_window(id: u64) -> std::io::Result<()> {
    action(json!({ "FocusWindow": { "id": id } }))
}

pub fn close_window(id: u64) -> std::io::Result<()> {
    action(json!({ "CloseWindow": { "id": id } }))
}

pub fn quit() -> std::io::Result<()> {
    action(json!({ "Quit": { "skip_confirmation": true } }))
}

#[cfg(test)]
mod tests {
    use super::{WindowAction, outputs_from_reply, window_action_request, workspaces_from_reply};
    use serde_json::{Value, json};

    #[test]
    fn window_management_requests_target_the_selected_window() {
        let id = 41;
        let cases: Vec<(WindowAction, Value)> = vec![
            (
                WindowAction::ToggleFullscreen,
                json!({ "Action": { "FullscreenWindow": { "id": id } } }),
            ),
            (
                WindowAction::ToggleWindowedFullscreen,
                json!({ "Action": { "ToggleWindowedFullscreen": { "id": id } } }),
            ),
            (
                WindowAction::ToggleFloating,
                json!({ "Action": { "ToggleWindowFloating": { "id": id } } }),
            ),
            (
                WindowAction::Center,
                json!({ "Action": { "CenterWindow": { "id": id } } }),
            ),
            (
                WindowAction::SetWidth(1.0 / 3.0),
                json!({ "Action": { "SetWindowWidth": { "id": id, "change": { "SetProportion": 1.0 / 3.0 } } } }),
            ),
            (
                WindowAction::SetWidth(0.5),
                json!({ "Action": { "SetWindowWidth": { "id": id, "change": { "SetProportion": 0.5 } } } }),
            ),
            (
                WindowAction::SetWidth(2.0 / 3.0),
                json!({ "Action": { "SetWindowWidth": { "id": id, "change": { "SetProportion": 2.0 / 3.0 } } } }),
            ),
            (
                WindowAction::SetWidth(1.0),
                json!({ "Action": { "SetWindowWidth": { "id": id, "change": { "SetProportion": 1.0 } } } }),
            ),
            (
                WindowAction::MoveToWorkspace(17),
                json!({ "Action": { "MoveWindowToWorkspace": { "window_id": id, "reference": { "Id": 17 }, "focus": true } } }),
            ),
            (
                WindowAction::MoveToMonitor("DP-1".into()),
                json!({ "Action": { "MoveWindowToMonitor": { "id": id, "output": "DP-1" } } }),
            ),
            (
                WindowAction::Screenshot,
                json!({ "Action": { "ScreenshotWindow": { "id": id, "write_to_disk": true, "show_pointer": false, "path": null } } }),
            ),
        ];

        for (action, expected) in cases {
            assert_eq!(window_action_request(id, action), expected);
        }
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
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].id, 17);
        assert_eq!(workspaces[0].idx, 2);
        assert_eq!(workspaces[0].name.as_deref(), Some("web"));
        assert_eq!(workspaces[0].output.as_deref(), Some("DP-1"));
        assert!(workspaces[0].is_focused);

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
}

#[cfg(test)]
mod searchable_command_tests {
    use super::{NIRI_COMMANDS, focused_window_from_reply, focused_window_snapshot_with};
    use serde_json::json;

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
            Some(super::FocusedWindowSnapshot {
                id: 41,
                is_floating: true,
            })
        );

        let no_focus = json!({ "Ok": { "Windows": [{ "id": 17, "is_focused": false }] } });
        assert_eq!(focused_window_from_reply(no_focus).unwrap(), None);
        assert!(focused_window_from_reply(json!({ "Err": "niri unavailable" })).is_err());
    }

    #[test]
    fn failed_snapshot_ipc_is_not_reported_as_no_focused_window() {
        let result =
            focused_window_snapshot_with(|_| Err(std::io::Error::other("niri socket unavailable")));
        assert_eq!(result.unwrap_err().to_string(), "niri socket unavailable");
    }

    #[test]
    fn focused_window_commands_build_the_expected_requests() {
        let id = 41;
        let cases = [
            (
                "niri:toggle-fullscreen",
                Some(id),
                json!({ "Action": { "FullscreenWindow": { "id": id } } }),
            ),
            (
                "niri:toggle-floating",
                Some(id),
                json!({ "Action": { "ToggleWindowFloating": { "id": id } } }),
            ),
            (
                "niri:center-window",
                Some(id),
                json!({ "Action": { "CenterWindow": { "id": id } } }),
            ),
            (
                "niri:maximize-column",
                None,
                json!({ "Action": { "MaximizeColumn": {} } }),
            ),
            (
                "niri:column-width-third",
                None,
                json!({ "Action": { "SetColumnWidth": { "change": { "SetProportion": 1.0 / 3.0 } } } }),
            ),
            (
                "niri:column-width-half",
                None,
                json!({ "Action": { "SetColumnWidth": { "change": { "SetProportion": 0.5 } } } }),
            ),
            (
                "niri:column-width-two-thirds",
                None,
                json!({ "Action": { "SetColumnWidth": { "change": { "SetProportion": 2.0 / 3.0 } } } }),
            ),
            (
                "niri:center-column",
                None,
                json!({ "Action": { "CenterColumn": {} } }),
            ),
            (
                "niri:toggle-tabbed-column",
                None,
                json!({ "Action": { "ToggleColumnTabbedDisplay": {} } }),
            ),
            (
                "niri:next-monitor",
                None,
                json!({ "Action": { "MoveWindowToMonitorNext": {} } }),
            ),
            (
                "niri:workspace-up",
                None,
                json!({ "Action": { "MoveWindowToWorkspaceUp": { "focus": true } } }),
            ),
            (
                "niri:workspace-down",
                None,
                json!({ "Action": { "MoveWindowToWorkspaceDown": { "focus": true } } }),
            ),
            (
                "niri:open-overview",
                None,
                json!({ "Action": { "OpenOverview": {} } }),
            ),
        ];

        for (key, window_id, expected) in cases {
            let command = NIRI_COMMANDS
                .iter()
                .find(|command| command.key == key)
                .unwrap_or_else(|| panic!("missing command {key}"));
            assert_eq!(command.build_request(window_id).unwrap(), expected, "{key}");
        }

        let fullscreen = NIRI_COMMANDS
            .iter()
            .find(|command| command.key == "niri:toggle-fullscreen")
            .unwrap();
        assert!(fullscreen.build_request(None).is_err());
    }
}
