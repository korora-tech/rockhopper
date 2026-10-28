//! niri IPC over `$NIRI_SOCKET`: one JSON request line, one JSON reply line.
//! <https://yalter.github.io/niri/niri_ipc/>

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

fn request(request: &Value) -> Option<Value> {
    let path = std::env::var_os("NIRI_SOCKET")?;
    let mut stream = UnixStream::connect(path).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .ok()?;
    writeln!(stream, "{request}").ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

pub fn available() -> bool {
    std::env::var_os("NIRI_SOCKET").is_some()
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

fn send_action_request(value: &Value) -> std::io::Result<()> {
    match request(value) {
        Some(reply) if reply.get("Ok").is_some() => Ok(()),
        Some(reply) => Err(std::io::Error::other(format!("niri replied {reply}"))),
        None => Err(std::io::Error::other("niri did not answer")),
    }
}

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
