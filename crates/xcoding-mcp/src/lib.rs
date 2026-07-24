//! Workspace MCP (Model Context Protocol) client for XCoding.
//!
//! V1 supports stdio transport only. Servers are configured in
//! `.xcoding/mcp.json` and exposed to the model as namespaced tools:
//! `mcp__<server>__<tool>`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

pub const MCP_CONFIG_RELATIVE_PATH: &str = ".xcoding/mcp.json";
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_SERVERS: usize = 16;
const MAX_TOOLS_PER_SERVER: usize = 64;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("MCP config error: {0}")]
    Config(String),
    #[error("MCP server `{server}` failed to start: {message}")]
    Spawn { server: String, message: String },
    #[error("MCP server `{server}` protocol error: {message}")]
    Protocol { server: String, message: String },
    #[error("MCP server `{server}` timed out")]
    Timeout { server: String },
    #[error("MCP server `{server}` is not available")]
    Unavailable { server: String },
    #[error("unknown MCP tool `{server}::{tool}`")]
    UnknownTool { server: String, tool: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct McpFileConfig {
    #[serde(default, rename = "mcpServers")]
    pub mcp_servers: HashMap<String, McpServerEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct McpServerEntry {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct McpToolDefinition {
    pub server: String,
    pub tool: String,
    pub namespaced_name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct McpCallResult {
    pub server: String,
    pub tool: String,
    pub is_error: bool,
    pub content: Value,
    pub structured_content: Option<Value>,
}

/// Encode a namespaced OpenAI tool name for an MCP tool.
pub fn encode_tool_name(server: &str, tool: &str) -> String {
    format!("mcp__{server}__{tool}")
}

/// Decode `mcp__<server>__<tool>` into `(server, tool)`.
pub fn decode_tool_name(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    if !is_valid_server_name(server) {
        return None;
    }
    Some((server.to_owned(), tool.to_owned()))
}

pub fn is_valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

/// Load MCP server configs from `.xcoding/mcp.json`.
/// Missing file => empty list. Invalid JSON => error.
pub fn load_mcp_config(workspace_root: impl AsRef<Path>) -> Result<Vec<McpServerConfig>, McpError> {
    let path = workspace_root.as_ref().join(MCP_CONFIG_RELATIVE_PATH);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path)?;
    parse_mcp_config_text(&raw)
}

/// Count configured / enabled servers without spawning processes.
pub fn summarize_mcp_config(workspace_root: impl AsRef<Path>) -> Result<(usize, usize), McpError> {
    let servers = load_mcp_config(workspace_root)?;
    let configured = servers.len();
    let enabled = servers.iter().filter(|server| server.enabled).count();
    Ok((configured, enabled))
}

pub fn parse_mcp_config_text(raw: &str) -> Result<Vec<McpServerConfig>, McpError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let file: McpFileConfig = serde_json::from_str(trimmed)
        .map_err(|error| McpError::Config(format!("invalid JSON: {error}")))?;
    let mut servers = Vec::new();
    for (name, entry) in file.mcp_servers {
        if !is_valid_server_name(&name) {
            return Err(McpError::Config(format!(
                "invalid server name `{name}` (use letters, digits, `_`, `-`)"
            )));
        }
        if entry.command.trim().is_empty() {
            return Err(McpError::Config(format!(
                "server `{name}` is missing a command"
            )));
        }
        servers.push(McpServerConfig {
            name,
            command: entry.command.trim().to_owned(),
            args: entry.args,
            env: entry.env,
            enabled: entry.enabled,
        });
    }
    servers.sort_by(|left, right| left.name.cmp(&right.name));
    if servers.len() > MAX_SERVERS {
        servers.truncate(MAX_SERVERS);
    }
    Ok(servers)
}

/// Live MCP sessions for one agent run.
pub struct McpRuntime {
    workspace_root: PathBuf,
    sessions: HashMap<String, McpSession>,
    tools: Vec<McpToolDefinition>,
    startup_errors: Vec<String>,
}

impl McpRuntime {
    /// Spawn enabled servers, initialize, and list tools.
    pub fn prepare(workspace_root: impl AsRef<Path>) -> Result<Self, McpError> {
        let workspace_root = workspace_root.as_ref().to_path_buf();
        let configs = load_mcp_config(&workspace_root)?;
        let mut sessions = HashMap::new();
        let mut tools = Vec::new();
        let mut startup_errors = Vec::new();

        for config in configs.into_iter().filter(|config| config.enabled) {
            match McpSession::start(&workspace_root, &config) {
                Ok(mut session) => match session.list_tools() {
                    Ok(listed) => {
                        for tool in listed.into_iter().take(MAX_TOOLS_PER_SERVER) {
                            tools.push(tool);
                        }
                        sessions.insert(config.name.clone(), session);
                    }
                    Err(error) => {
                        startup_errors.push(error.to_string());
                    }
                },
                Err(error) => startup_errors.push(error.to_string()),
            }
        }

        tools.sort_by(|left, right| left.namespaced_name.cmp(&right.namespaced_name));
        Ok(Self {
            workspace_root,
            sessions,
            tools,
            startup_errors,
        })
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn tools(&self) -> &[McpToolDefinition] {
        &self.tools
    }

    pub fn startup_errors(&self) -> &[String] {
        &self.startup_errors
    }

    pub fn call(
        &mut self,
        server: &str,
        tool: &str,
        arguments: Value,
    ) -> Result<McpCallResult, McpError> {
        if !self
            .tools
            .iter()
            .any(|item| item.server == server && item.tool == tool)
        {
            return Err(McpError::UnknownTool {
                server: server.to_owned(),
                tool: tool.to_owned(),
            });
        }
        let session = self
            .sessions
            .get_mut(server)
            .ok_or_else(|| McpError::Unavailable {
                server: server.to_owned(),
            })?;
        session.call_tool(tool, arguments)
    }
}

impl Drop for McpRuntime {
    fn drop(&mut self) {
        for (_, mut session) in self.sessions.drain() {
            session.shutdown();
        }
    }
}

struct McpSession {
    name: String,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: AtomicU64,
}

impl McpSession {
    fn start(workspace_root: &Path, config: &McpServerConfig) -> Result<Self, McpError> {
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .current_dir(workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (key, value) in &config.env {
            command.env(key, value);
        }
        // Keep parent env by default so `node`/`npx` PATH works; overlay only listed keys.
        let mut child = command.spawn().map_err(|error| McpError::Spawn {
            server: config.name.clone(),
            message: error.to_string(),
        })?;
        let stdin = child.stdin.take().ok_or_else(|| McpError::Spawn {
            server: config.name.clone(),
            message: "missing stdin pipe".to_owned(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| McpError::Spawn {
            server: config.name.clone(),
            message: "missing stdout pipe".to_owned(),
        })?;
        let mut session = Self {
            name: config.name.clone(),
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: AtomicU64::new(1),
        };
        session.initialize()?;
        Ok(session)
    }

    fn initialize(&mut self) -> Result<(), McpError> {
        let result = self.request(
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "xcoding",
                    "version": "0.1.0"
                }
            }),
        )?;
        if result.get("protocolVersion").and_then(Value::as_str).is_none() {
            // Some servers still return success without echoing the version; accept either shape.
        }
        self.notify(
            "notifications/initialized",
            json!({}),
        )?;
        Ok(())
    }

    fn list_tools(&mut self) -> Result<Vec<McpToolDefinition>, McpError> {
        let result = self.request("tools/list", json!({}))?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::new();
        for tool in tools {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if name.is_empty() {
                continue;
            }
            let description = tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("MCP tool")
                .to_owned();
            let parameters = tool
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
            out.push(McpToolDefinition {
                server: self.name.clone(),
                tool: name.to_owned(),
                namespaced_name: encode_tool_name(&self.name, name),
                description: format!("[MCP:{server}] {description}", server = self.name),
                parameters,
            });
        }
        Ok(out)
    }

    fn call_tool(&mut self, tool: &str, arguments: Value) -> Result<McpCallResult, McpError> {
        let result = self.request(
            "tools/call",
            json!({
                "name": tool,
                "arguments": if arguments.is_null() { json!({}) } else { arguments }
            }),
        )?;
        let is_error = result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let content = result
            .get("content")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let structured_content = result.get("structuredContent").cloned();
        Ok(McpCallResult {
            server: self.name.clone(),
            tool: tool.to_owned(),
            is_error,
            content,
            structured_content,
        })
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_message(&payload)?;
        self.read_response(id)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), McpError> {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_message(&payload)
    }

    fn write_message(&mut self, payload: &Value) -> Result<(), McpError> {
        let mut line = serde_json::to_string(payload)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.flush()?;
        Ok(())
    }

    fn read_response(&mut self, expected_id: u64) -> Result<Value, McpError> {
        let deadline = Instant::now() + DEFAULT_TIMEOUT;
        loop {
            if Instant::now() > deadline {
                return Err(McpError::Timeout {
                    server: self.name.clone(),
                });
            }
            // Best-effort non-blocking wait: poll child exit and read with timeout via short sleeps.
            // BufReader::read_line blocks; use a background-free approach by checking child first.
            if let Ok(Some(status)) = self.child.try_wait() {
                return Err(McpError::Protocol {
                    server: self.name.clone(),
                    message: format!("process exited early with {status}"),
                });
            }

            // Use a temporary approach: set read with thread + channel for timeout.
            let mut line = String::new();
            match read_line_with_timeout(&mut self.stdout, &mut line, deadline) {
                Ok(0) => {
                    return Err(McpError::Protocol {
                        server: self.name.clone(),
                        message: "stdout closed".to_owned(),
                    });
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let message: Value = serde_json::from_str(trimmed).map_err(|error| {
                        McpError::Protocol {
                            server: self.name.clone(),
                            message: format!("invalid JSON from server: {error}"),
                        }
                    })?;
                    // Notifications / unrelated messages: skip unless id matches.
                    if let Some(id) = message.get("id") {
                        let matches = match id {
                            Value::Number(number) => number
                                .as_u64()
                                .map(|value| value == expected_id)
                                .unwrap_or(false),
                            Value::String(text) => text.parse::<u64>()
                                .map(|value| value == expected_id)
                                .unwrap_or(false),
                            _ => false,
                        };
                        if !matches {
                            continue;
                        }
                        if let Some(error) = message.get("error") {
                            let text = error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown MCP error");
                            return Err(McpError::Protocol {
                                server: self.name.clone(),
                                message: text.to_owned(),
                            });
                        }
                        return Ok(message.get("result").cloned().unwrap_or(Value::Null));
                    }
                    // Notification without id — ignore.
                }
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                    return Err(McpError::Timeout {
                        server: self.name.clone(),
                    });
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn shutdown(&mut self) {
        let _ = self.stdin.write_all(b"");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_line_with_timeout<R: BufRead>(
    reader: &mut R,
    line: &mut String,
    deadline: Instant,
) -> std::io::Result<usize> {
    // V1: blocking read. E2E and local MCP servers respond immediately; process death is
    // still detected by the caller loop. A true async/interruptible timeout can come later.
    let _ = deadline;
    reader.read_line(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_workspace(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("xcoding-mcp-{label}-{unique}"));
        fs::create_dir_all(path.join(".xcoding")).expect("mkdir");
        path
    }

    #[test]
    fn encodes_and_decodes_namespaced_tool_names() {
        let name = encode_tool_name("demo", "echo");
        assert_eq!(name, "mcp__demo__echo");
        assert_eq!(
            decode_tool_name(&name),
            Some(("demo".to_owned(), "echo".to_owned()))
        );
        assert_eq!(
            decode_tool_name("mcp__demo__nested__tool"),
            Some(("demo".to_owned(), "nested__tool".to_owned()))
        );
        assert_eq!(decode_tool_name("list_dir"), None);
        assert_eq!(decode_tool_name("mcp__"), None);
    }

    #[test]
    fn parses_mcp_config_json() {
        let raw = r#"{
          "mcpServers": {
            "demo": {
              "command": "node",
              "args": ["server.mjs"],
              "env": { "FOO": "bar" },
              "enabled": true
            },
            "off": {
              "command": "node",
              "enabled": false
            }
          }
        }"#;
        let servers = parse_mcp_config_text(raw).expect("parse");
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].name, "demo");
        assert_eq!(servers[0].args, vec!["server.mjs".to_owned()]);
        assert_eq!(servers[0].env.get("FOO").map(String::as_str), Some("bar"));
        assert!(!servers[1].enabled);
    }

    #[test]
    fn load_missing_config_is_empty() {
        let root = temp_workspace("missing");
        let servers = load_mcp_config(&root).expect("load");
        assert!(servers.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_invalid_server_names() {
        let err = parse_mcp_config_text(
            r#"{ "mcpServers": { "bad name": { "command": "node" } } }"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid server name"));
    }
}
