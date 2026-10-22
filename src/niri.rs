//! niri IPC over `$NIRI_SOCKET`: one JSON request line, one JSON reply line.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde_json::Value;

fn request(request: &str) -> Option<Value> {
    let path = std::env::var_os("NIRI_SOCKET")?;
    let mut stream = UnixStream::connect(path).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(200))).ok()?;
    writeln!(stream, "{request}").ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

/// The name of the output with keyboard focus, such as `DP-1`.
pub fn focused_output() -> Option<String> {
    let reply = request(r#""FocusedOutput""#)?;
    reply["Ok"]["FocusedOutput"]["name"].as_str().map(str::to_owned)
}
