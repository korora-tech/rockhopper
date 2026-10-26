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
}

#[derive(Deserialize)]
struct RawWindow {
    id: u64,
    title: Option<String>,
    app_id: Option<String>,
    workspace_id: Option<u64>,
    #[serde(default)]
    is_focused: bool,
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
    let workspaces: HashMap<u64, String> = request(&json!("Workspaces"))
        .and_then(|r| {
            serde_json::from_value::<Vec<RawWorkspace>>(r["Ok"]["Workspaces"].clone()).ok()
        })
        .unwrap_or_default()
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
        })
        .collect()
}

fn action(action: Value) -> std::io::Result<()> {
    match request(&json!({ "Action": action })) {
        Some(reply) if reply.get("Ok").is_some() => Ok(()),
        Some(reply) => Err(std::io::Error::other(format!("niri replied {reply}"))),
        None => Err(std::io::Error::other("niri did not answer")),
    }
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
