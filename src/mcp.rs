//! MCP servers from the config: a minimal client for the stdio transport.
//! Their tools are listed in the launcher and run on `enter`. Each
//! discovery or call starts the server, talks to it, and stops it again, so
//! nothing stays running while the launcher is closed.
//!
//! Only tools that need no arguments are offered. A tool result that carries
//! an MCP-UI resource (`ui://…` with `text/html`) is opened in the browser.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command as Process, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const PROTOCOL_VERSION: &str = "2025-06-18";
const TIMEOUT: Duration = Duration::from_secs(20);

/// One entry under `mcp:` in the config.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub enabled: Option<bool>,
    /// Program and arguments that start the server on stdio.
    pub command: Vec<String>,
    /// Added to Rockhopper's own environment.
    pub env: BTreeMap<String, String>,
}

/// A tool a server offers that can run without arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct Tool {
    /// The config key of its server.
    pub server: String,
    pub name: String,
    pub title: String,
    pub description: Option<String>,
    /// The `ui://` resource that renders it, from the tool's `_meta`.
    pub ui_resource: Option<String>,
}

impl Tool {
    pub fn history_key(&self) -> String {
        format!("mcp:{}:{}", self.server, self.name)
    }
}

/// What a tool call produced.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub text: String,
    pub html: Option<String>,
    /// The `tools/call` result as the server sent it.
    pub raw: Value,
    pub is_error: bool,
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
}

impl Drop for Session {
    fn drop(&mut self) {
        _ = self.child.kill();
        _ = self.child.wait();
    }
}

impl Session {
    fn start(config: &ServerConfig) -> Result<Self, String> {
        let (program, args) = config.command.split_first().ok_or("no command")?;
        let mut child = Process::new(program)
            .args(args)
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("{program}: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut session = Self {
            child,
            stdin,
            lines,
            next_id: 1,
        };
        session.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "rockhopper", "version": env!("CARGO_PKG_VERSION")},
            }),
        )?;
        session.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(session)
    }

    fn send(&mut self, message: &Value) -> Result<(), String> {
        writeln!(self.stdin, "{message}")
            .and_then(|()| self.stdin.flush())
            .map_err(|e| e.to_string())
    }

    /// Send a request and wait for its response; notifications and requests
    /// from the server in between are ignored.
    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        loop {
            let line = self
                .lines
                .recv_timeout(TIMEOUT)
                .map_err(|_| format!("{method}: no response"))?;
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("id") != Some(&json!(id)) || message.get("method").is_some() {
                continue;
            }
            if let Some(error) = message.get("error") {
                let text = error["message"].as_str().unwrap_or("error");
                return Err(format!("{method}: {text}"));
            }
            return Ok(message["result"].clone());
        }
    }
}

/// Every runnable tool of every enabled server. A server that fails is
/// reported and skipped.
pub fn discover(servers: &BTreeMap<String, ServerConfig>) -> Vec<Tool> {
    let handles: Vec<_> = servers
        .iter()
        .filter(|(_, s)| s.enabled != Some(false))
        .map(|(key, server)| {
            let (key, server) = (key.clone(), server.clone());
            std::thread::spawn(move || {
                list_tools(&key, &server).unwrap_or_else(|err| {
                    eprintln!("rockhopper: mcp {key}: {err}");
                    Vec::new()
                })
            })
        })
        .collect();
    handles
        .into_iter()
        .flat_map(|h| h.join().unwrap_or_default())
        .collect()
}

fn list_tools(key: &str, config: &ServerConfig) -> Result<Vec<Tool>, String> {
    let mut session = Session::start(config)?;
    let mut tools = Vec::new();
    let mut cursor = None;
    loop {
        let params = cursor.map_or(json!({}), |c| json!({"cursor": c}));
        let page = session.request("tools/list", params)?;
        tools.extend(
            page["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| parse_tool(key, t)),
        );
        match page["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => return Ok(tools),
        }
    }
}

/// `None` for a tool that has required arguments.
fn parse_tool(server: &str, tool: &Value) -> Option<Tool> {
    let name = tool["name"].as_str()?;
    if tool["inputSchema"]["required"]
        .as_array()
        .is_some_and(|r| !r.is_empty())
    {
        return None;
    }
    let meta = &tool["_meta"];
    let ui_resource = meta["ui"]["resourceUri"]
        .as_str()
        .or_else(|| meta["ui/resourceUri"].as_str())
        .map(str::to_owned);
    Some(Tool {
        server: server.to_owned(),
        name: name.to_owned(),
        title: tool["title"]
            .as_str()
            .or_else(|| tool["annotations"]["title"].as_str())
            .unwrap_or(name)
            .to_owned(),
        description: tool["description"]
            .as_str()
            .filter(|d| !d.is_empty())
            .map(|d| d.lines().next().unwrap_or(d).to_owned()),
        ui_resource,
    })
}

/// Run `tool` on its server.
pub fn call(config: &ServerConfig, tool: &Tool) -> Result<Outcome, String> {
    let mut session = Session::start(config)?;
    let result = session.request("tools/call", json!({"name": tool.name, "arguments": {}}))?;
    let mut outcome = parse_result(&result);
    if outcome.html.is_none()
        && let Some(uri) = &tool.ui_resource
    {
        let resource = session.request("resources/read", json!({"uri": uri}))?;
        outcome.html = resource["contents"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(html_of);
    }
    Ok(outcome)
}

/// Forward one request from an MCP App to its server. Only the calls an app
/// may make are passed on.
pub fn proxy(config: &ServerConfig, method: &str, params: Value) -> Result<Value, String> {
    if !matches!(method, "tools/call" | "resources/read") {
        return Err(format!("{method}: not allowed"));
    }
    Session::start(config)?.request(method, params)
}

fn html_of(resource: &Value) -> Option<String> {
    let html = resource["mimeType"]
        .as_str()
        .is_some_and(|m| m.starts_with("text/html"));
    if !html {
        return None;
    }
    resource["text"].as_str().map(str::to_owned)
}

fn parse_result(result: &Value) -> Outcome {
    let mut outcome = Outcome {
        is_error: result["isError"].as_bool().unwrap_or(false),
        raw: result.clone(),
        ..Outcome::default()
    };
    let mut texts = Vec::new();
    for block in result["content"].as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") => texts.extend(block["text"].as_str()),
            Some("resource") => {
                let resource = &block["resource"];
                let ui = resource["uri"]
                    .as_str()
                    .is_some_and(|u| u.starts_with("ui://"));
                match html_of(resource) {
                    Some(html) if ui => outcome.html = Some(html),
                    _ => texts.extend(resource["text"].as_str()),
                }
            }
            _ => {}
        }
    }
    outcome.text = texts.join("\n");
    outcome
}

/// Write MCP-UI `html` where a browser can open it.
pub fn write_page(html: &str) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = dir.join(format!("rockhopper-mcp-{nanos}.html"));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?
        .write_all(html.as_bytes())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server that answers requests 1 to 3 with canned responses.
    fn fake() -> ServerConfig {
        let script = r#"
read l; echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{}}}'
read l
read l; echo '{"jsonrpc":"2.0","method":"notifications/message"}'
case "$l" in
*tools/list*) echo '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"ping","description":"Say pong","inputSchema":{"type":"object"}},{"name":"echo","inputSchema":{"type":"object","required":["text"]}},{"name":"dash","title":"Dashboard","_meta":{"ui":{"resourceUri":"ui://dash"}},"inputSchema":{"type":"object"}}]}}' ;;
*tools/call*) echo '{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"pong"},{"type":"resource","resource":{"uri":"ui://x","mimeType":"text/html","text":"<b>hi</b>"}}]}}' ;;
esac
read l
"#;
        ServerConfig {
            command: vec!["sh".into(), "-c".into(), script.into()],
            ..ServerConfig::default()
        }
    }

    #[test]
    fn lists_runnable_tools() {
        let tools = list_tools("fake", &fake()).unwrap();
        let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["ping", "dash"]);
        assert_eq!(tools[0].description.as_deref(), Some("Say pong"));
        assert_eq!(tools[1].title, "Dashboard");
        assert_eq!(tools[1].ui_resource.as_deref(), Some("ui://dash"));
    }

    #[test]
    fn description_is_its_first_line() {
        let tool = json!({"name": "t", "description": "One\nTwo", "inputSchema": {}});
        assert_eq!(
            parse_tool("s", &tool).unwrap().description.as_deref(),
            Some("One")
        );
    }

    #[test]
    fn calls_a_tool() {
        let tool = list_tools("fake", &fake()).unwrap().remove(0);
        let outcome = call(&fake(), &tool).unwrap();
        assert_eq!(outcome.text, "pong");
        assert_eq!(outcome.html.as_deref(), Some("<b>hi</b>"));
        assert!(!outcome.is_error);
    }

    #[test]
    fn missing_server_is_an_error() {
        let config = ServerConfig {
            command: vec!["/nonexistent/mcp".into()],
            ..ServerConfig::default()
        };
        assert!(list_tools("x", &config).is_err());
        assert!(discover(&BTreeMap::from([("x".to_string(), config)])).is_empty());
    }
}
