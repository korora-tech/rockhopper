//! The control socket: `$XDG_RUNTIME_DIR/rockhopper.sock`, one command per
//! connection, as a line of text.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

/// A client that connects and says nothing must not block other requests.
const TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Toggle,
    Show,
    Hide,
    Quit,
    /// Open on the clipboard history.
    Clipboard,
    /// Open on the emoji picker.
    Emoji,
    /// Drive the open launcher as if typed; see [`TestInput`].
    Test(TestInput),
}

/// Input for automated tests, which cannot use the real keyboard without
/// reaching the rest of the session. Accepted only by a daemon started with
/// `ROCKHOPPER_TEST=1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestInput {
    /// Replace the search text.
    Type(String),
    /// Press a key, by name: down, up, left, right, tab, enter, escape, ctrl-k.
    Key(String),
}

/// Whether this daemon accepts [`Request::Test`].
pub fn test_input_enabled() -> bool {
    std::env::var_os("ROCKHOPPER_TEST").is_some_and(|v| v == "1")
}

impl Request {
    pub fn parse(s: &str) -> Option<Self> {
        if let Some(text) = s.strip_prefix("test-type ") {
            return Some(Self::Test(TestInput::Type(text.to_owned())));
        }
        if let Some(key) = s.strip_prefix("test-key ") {
            return Some(Self::Test(TestInput::Key(key.to_owned())));
        }
        Some(match s {
            "toggle" => Self::Toggle,
            "show" => Self::Show,
            "hide" => Self::Hide,
            "quit" => Self::Quit,
            "clipboard" => Self::Clipboard,
            "emoji" => Self::Emoji,
            _ => return None,
        })
    }

    fn line(&self) -> String {
        match self {
            Self::Toggle => "toggle".into(),
            Self::Show => "show".into(),
            Self::Hide => "hide".into(),
            Self::Quit => "quit".into(),
            Self::Clipboard => "clipboard".into(),
            Self::Emoji => "emoji".into(),
            Self::Test(TestInput::Type(text)) => format!("test-type {}", text.replace('\n', " ")),
            Self::Test(TestInput::Key(key)) => format!("test-key {key}"),
        }
    }
}

pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        // Without a runtime dir, fall back to a per-user name in /tmp.
        .unwrap_or_else(|| PathBuf::from(format!("/tmp/rockhopper-{}", unsafe { libc::getuid() })));
    dir.join("rockhopper.sock")
}

/// Send `request` to a running daemon. `Ok(false)` means none is running.
pub fn send(request: &Request) -> std::io::Result<bool> {
    let mut stream = match UnixStream::connect(socket_path()) {
        Ok(s) => s,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(false);
        }
        Err(e) => return Err(e),
    };
    stream.set_read_timeout(Some(TIMEOUT * 2))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    writeln!(stream, "{}", request.line())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    match reply.trim() {
        "ok" => Ok(true),
        other => Err(std::io::Error::other(format!("daemon replied {other:?}"))),
    }
}

/// Bind the socket, replacing a stale one. Fails if another daemon answers.
pub fn bind() -> std::io::Result<UnixListener> {
    let path = socket_path();
    if std::env::var_os("XDG_RUNTIME_DIR").is_none_or(|v| v.is_empty()) {
        private_dir(path.parent().expect("socket path has a parent"))?;
    }
    match UnixListener::bind(&path) {
        Ok(l) => Ok(l),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if UnixStream::connect(&path).is_ok() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    "rockhopper is already running",
                ));
            }
            std::fs::remove_file(&path)?;
            UnixListener::bind(&path)
        }
        Err(e) => Err(e),
    }
}

/// Accept connections on a thread and forward each request.
pub fn serve(listener: UnixListener, tx: async_channel::Sender<Request>) {
    let test_input = test_input_enabled();
    std::thread::Builder::new()
        .name("rockhopper-ipc".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                _ = stream.set_read_timeout(Some(TIMEOUT));
                _ = stream.set_write_timeout(Some(TIMEOUT));
                let mut line = String::new();
                let mut reader = BufReader::new(&stream);
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let reply = match Request::parse(line.trim_end_matches(['\r', '\n'])) {
                    Some(Request::Test(_)) if !test_input => "test input is off",
                    Some(req) => {
                        // So a `rockhopper` run right after `quit` starts a
                        // new daemon rather than reaching this one.
                        if req == Request::Quit {
                            remove_socket();
                        }
                        if tx.send_blocking(req).is_err() {
                            return;
                        }
                        "ok"
                    }
                    None => "unknown command",
                };
                _ = writeln!(&stream, "{reply}");
            }
        })
        .expect("spawn ipc thread");
}

/// The `/tmp` fallback must be ours alone: another user could otherwise
/// create it first and replace the socket.
fn private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } || meta.mode() & 0o077 != 0 {
        return Err(std::io::Error::other(format!(
            "{} must be a directory owned by you with mode 0700",
            dir.display()
        )));
    }
    Ok(())
}

pub fn remove_socket() {
    _ = std::fs::remove_file(socket_path());
}
