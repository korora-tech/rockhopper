//! A window for MCP Apps (`ui://` resources): a webview that hosts the app
//! in a sandboxed iframe and speaks the host side of the MCP Apps protocol
//! (SEP-1865) to it.
//!
//! It runs as its own process, `rockhopper view <spec file>`, because
//! WebKitGTK wants to own the GTK main loop, which GPUI does not use. The
//! daemon writes a [`Spec`] and starts the process.

use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::unix::WindowExtUnix;
use tao::window::WindowBuilder;
use wry::{WebViewBuilder, WebViewBuilderExtUnix};

use crate::mcp::ServerConfig;

/// What the viewer shows and how it talks to the server behind it.
#[derive(Serialize, Deserialize)]
pub struct Spec {
    pub title: String,
    pub tool: String,
    /// The app's HTML, from its `ui://` resource.
    pub html: String,
    /// The `tools/call` result the app is told about.
    pub result: Value,
    pub server: ServerConfig,
    pub dark: bool,
}

/// The host page: the iframe, and the host half of the protocol. The app
/// is `sandbox`ed without `allow-same-origin`, so it cannot reach this page
/// and can only talk through `postMessage`.
const HOST_PAGE: &str = include_str!("viewer.html");

enum Reply {
    Result(Value, Value),
    Error(Value, String),
}

/// Write `spec` where the viewer process can read it.
pub fn write_spec(spec: &Spec) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = dir.join(format!("rockhopper-view-{nanos}.json"));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?
        .write_all(&serde_json::to_vec(spec)?)?;
    Ok(path)
}

/// Run the viewer for the spec in `path`, then delete it. Returns when the
/// window closes.
pub fn run(path: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    _ = std::fs::remove_file(path);
    let spec: Spec = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;

    let event_loop = EventLoopBuilder::<Reply>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title(&spec.title)
        .with_inner_size(LogicalSize::new(900.0, 640.0))
        .build(&event_loop)
        .map_err(|e| e.to_string())?;
    let proxy = event_loop.create_proxy();
    let server = spec.server.clone();
    let boot = format!(
        "window.__spec = {};",
        json!({
            "html": spec.html,
            "result": spec.result,
            "tool": spec.tool,
            "title": spec.title,
            "dark": spec.dark,
            "version": env!("CARGO_PKG_VERSION"),
        })
    );
    let vbox = window.default_vbox().ok_or("no window container")?;
    let webview = WebViewBuilder::new()
        .with_initialization_script(boot)
        .with_html(HOST_PAGE)
        .with_ipc_handler(move |request| {
            let Ok(message) = serde_json::from_str::<Value>(request.body()) else {
                return;
            };
            match message["kind"].as_str() {
                Some("open") => open_link(message["url"].as_str().unwrap_or_default()),
                Some("log") => eprintln!("rockhopper: mcp app: {}", message["data"]),
                Some("proxy") => {
                    let (id, params) = (message["id"].clone(), message["params"].clone());
                    let method = message["method"].as_str().unwrap_or_default().to_owned();
                    let (server, proxy) = (server.clone(), proxy.clone());
                    // Off the UI thread: starting the server takes a moment.
                    std::thread::spawn(move || {
                        let reply = match crate::mcp::proxy(&server, &method, params) {
                            Ok(result) => Reply::Result(id, result),
                            Err(err) => Reply::Error(id, err),
                        };
                        _ = proxy.send_event(reply);
                    });
                }
                _ => {}
            }
        })
        .build_gtk(vbox)
        .map_err(|e| e.to_string())?;

    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *flow = ControlFlow::Exit,
            Event::UserEvent(reply) => {
                let call = match reply {
                    Reply::Result(id, result) => format!("window.__reply({id}, {result}, null)"),
                    Reply::Error(id, err) => {
                        format!("window.__reply({id}, null, {})", json!(err))
                    }
                };
                _ = webview.evaluate_script(&call);
            }
            _ => {}
        }
    });
}

/// Hand a link the app asked to open to the browser; only web links.
fn open_link(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        eprintln!("rockhopper: mcp app: not opening {url}");
        return;
    }
    if let Err(err) = crate::system::spawn(&["xdg-open".into(), url.into()]) {
        eprintln!("rockhopper: mcp app: {err}");
    }
}
