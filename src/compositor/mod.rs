//! The compositor Rockhopper runs under. Each backend implements
//! [`Compositor`]; [`detect`] picks one from the environment, and the
//! launcher asks it for windows, outputs and window actions.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::sync::Arc;

pub(crate) mod niri;
pub(crate) mod umbriel;

/// A window's id as its compositor reports it. Niri uses numbers and Umbriel
/// uses strings, so ids are kept as text and each backend parses its own.
pub type WindowId = String;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub id: WindowId,
    pub title: String,
    pub app_id: Option<String>,
    /// The workspace's name, else its position, for display.
    pub workspace: Option<String>,
    pub is_focused: bool,
    pub is_floating: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    /// What [`Compositor::move_window_to_workspace`] takes back.
    pub id: String,
    /// The workspace's position, from 1.
    pub position: u64,
    pub name: Option<String>,
    pub output: Option<String>,
    pub is_focused: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusedWindow {
    pub id: WindowId,
    pub is_floating: bool,
}

/// A window or column operation, named for what it does rather than for any
/// one compositor's request.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    ToggleFullscreen,
    ToggleWindowedFullscreen,
    ToggleFloating,
    Center,
    /// Width as a fraction of the output.
    SetWidth(f64),
    MaximizeColumn,
    CenterColumn,
    ToggleTabbedColumn,
    MoveToNextMonitor,
    MoveToWorkspaceUp,
    MoveToWorkspaceDown,
    Screenshot,
    Close,
    OpenOverview,
}

/// What a built-in command acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The window the launcher was opened for.
    Window,
    /// Whatever has focus; the launcher waits for focus to return first.
    Focused,
    /// Opens the overview; needs no window.
    Overview,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Label {
    Static(&'static str),
    /// "Float Window" or "Tile Window", by the focused window's state.
    FloatingToggle,
}

/// A command the launcher lists for every compositor that supports its op.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuiltinCommand {
    /// Also the history key.
    pub key: &'static str,
    /// The `commands:` config entry that can override this one's keywords.
    pub config_key: Option<&'static str>,
    pub label: Label,
    pub keywords: &'static [&'static str],
    pub scope: Scope,
    pub op: Op,
}

impl BuiltinCommand {
    pub fn name(self, is_floating: bool) -> &'static str {
        match self.label {
            Label::Static(name) => name,
            Label::FloatingToggle if is_floating => "Tile Window",
            Label::FloatingToggle => "Float Window",
        }
    }
}

pub trait Compositor: Send + Sync {
    /// Short name for messages and config defaults, such as `niri`.
    fn name(&self) -> &'static str;

    /// Whether this compositor can perform `op`. Unsupported operations are
    /// left out of menus.
    fn supports(&self, op: Op) -> bool;

    /// Open windows, most recently focused first when the compositor says so.
    fn windows(&self) -> Vec<Window>;
    fn workspaces(&self) -> Vec<Workspace>;
    fn outputs(&self) -> Vec<Output>;

    /// The window that has focus, or `None` when no window does.
    fn focused_window(&self) -> io::Result<Option<FocusedWindow>>;
    /// The output with keyboard focus, such as `DP-1`.
    fn focused_output(&self) -> Option<String>;

    fn focus_window(&self, id: &str) -> io::Result<()>;
    /// Perform `op` on the window `id`, focusing it first where the
    /// compositor cannot address it directly.
    fn window_op(&self, id: &str, op: Op) -> io::Result<()>;
    /// Perform `op` on the focused window or column.
    fn focused_op(&self, op: Op) -> io::Result<()>;
    fn move_window_to_workspace(&self, id: &str, workspace: &str) -> io::Result<()>;
    fn move_window_to_monitor(&self, id: &str, output: &str) -> io::Result<()>;
    /// End the session without a second confirmation. `None` when the
    /// compositor has no quit action of its own.
    fn log_out(&self) -> Option<io::Result<()>> {
        None
    }

    /// The commands listed in the launcher, before support filtering.
    fn builtin_commands(&self) -> &'static [BuiltinCommand];

    /// Whether the window's workspace can be chosen by name from a picker.
    fn can_move_to_workspace(&self) -> bool {
        true
    }

    /// Whether the window can be sent to a named output from a picker.
    fn can_move_to_output(&self) -> bool {
        true
    }

    fn close_window(&self, id: &str) -> io::Result<()> {
        self.window_op(id, Op::Close)
    }

    /// Run a built-in command. `window` is the window the launcher captured;
    /// it is required for window-scoped commands.
    fn run_builtin(&self, command: BuiltinCommand, window: Option<&str>) -> io::Result<()> {
        match command.scope {
            Scope::Window => {
                let id = window.ok_or_else(|| io::Error::other("no window was captured"))?;
                self.window_op(id, command.op)
            }
            Scope::Focused | Scope::Overview => self.focused_op(command.op),
        }
    }
}

/// Picks the backend from the environment: a niri IPC socket, else an Umbriel
/// IPC socket, else a backend that lists nothing and performs no actions.
pub fn detect() -> Arc<dyn Compositor> {
    detect_with(&|key| std::env::var_os(key), &|path| path.exists())
}

fn detect_with(
    var: &dyn Fn(&str) -> Option<OsString>,
    exists: &dyn Fn(&Path) -> bool,
) -> Arc<dyn Compositor> {
    if var("NIRI_SOCKET").is_some() {
        return Arc::new(niri::Niri);
    }
    if let Some(path) = umbriel::socket_path_with(var).filter(|path| exists(path)) {
        return Arc::new(umbriel::Umbriel::new(path));
    }
    Arc::new(Unknown)
}

/// Used when no supported compositor is running. Queries are empty and every
/// action fails with a clear message.
pub struct Unknown;

impl Compositor for Unknown {
    fn name(&self) -> &'static str {
        "unknown"
    }

    fn supports(&self, _op: Op) -> bool {
        false
    }

    fn windows(&self) -> Vec<Window> {
        Vec::new()
    }

    fn workspaces(&self) -> Vec<Workspace> {
        Vec::new()
    }

    fn outputs(&self) -> Vec<Output> {
        Vec::new()
    }

    fn focused_window(&self) -> io::Result<Option<FocusedWindow>> {
        Ok(None)
    }

    fn focused_output(&self) -> Option<String> {
        None
    }

    fn focus_window(&self, _id: &str) -> io::Result<()> {
        Err(unsupported())
    }

    fn window_op(&self, _id: &str, _op: Op) -> io::Result<()> {
        Err(unsupported())
    }

    fn focused_op(&self, _op: Op) -> io::Result<()> {
        Err(unsupported())
    }

    fn move_window_to_workspace(&self, _id: &str, _workspace: &str) -> io::Result<()> {
        Err(unsupported())
    }

    fn move_window_to_monitor(&self, _id: &str, _output: &str) -> io::Result<()> {
        Err(unsupported())
    }

    fn builtin_commands(&self) -> &'static [BuiltinCommand] {
        &[]
    }
}

fn unsupported() -> io::Error {
    io::Error::other("no supported compositor is running")
}

/// Sends one JSON request line to a UNIX socket and reads one reply line.
pub(crate) fn exchange(path: &Path, request: &serde_json::Value) -> io::Result<serde_json::Value> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_millis(200)))?;
    writeln!(stream, "{request}")?;
    let mut line = String::new();
    if BufReader::new(stream).read_line(&mut line)? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the compositor closed the IPC connection without replying",
        ));
    }
    serde_json::from_str(&line).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A variable lookup over fixed pairs.
    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn niri_wins_when_its_socket_is_set() {
        let vars = [
            ("NIRI_SOCKET", "/run/niri.sock"),
            ("UMBRIEL_SOCKET", "/run/u.sock"),
        ];
        let compositor = detect_with(&env(&vars), &|_| true);
        assert_eq!(compositor.name(), "niri");
    }

    #[test]
    fn umbriel_is_found_from_its_socket_variable() {
        let vars = [("UMBRIEL_SOCKET", "/run/u.sock")];
        assert_eq!(detect_with(&env(&vars), &|_| true).name(), "umbriel");
    }

    #[test]
    fn umbriel_is_found_from_the_runtime_directory_path() {
        let vars = [
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("WAYLAND_DISPLAY", "wayland-1"),
        ];
        let compositor = detect_with(&env(&vars), &|path| {
            path == Path::new("/run/user/1000/umbriel-wayland-1.sock")
        });
        assert_eq!(compositor.name(), "umbriel");
    }

    #[test]
    fn a_stale_umbriel_socket_path_falls_back_to_no_compositor() {
        let vars = [("UMBRIEL_SOCKET", "/run/gone.sock")];
        assert_eq!(detect_with(&env(&vars), &|_| false).name(), "unknown");
        assert_eq!(detect_with(&env(&[]), &|_| true).name(), "unknown");
    }
}
