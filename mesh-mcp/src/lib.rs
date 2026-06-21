//! # mesh-mcp — make any mesh app AI-agent-accessible
//!
//! A tiny, dependency-light MCP (Model Context Protocol) host. An app implements
//! the [`McpApp`] trait (declare your tools + handle calls), and the app's data and
//! actions become reachable by ANY MCP client (Claude Desktop, Cursor, …) over a
//! local stdio server — with **no server in the loop**: the tool handlers run
//! against the app's own mesh replica on the user's machine.
//!
//! MCP is just JSON-RPC 2.0 with a defined method set, so it's hand-rolled here
//! rather than pulled from a fast-moving external SDK — keeping the volatile surface
//! behind this one crate (the family's "wrap volatile deps" rule). The protocol
//! logic ([`handle_line`]) is separated from the transport ([`serve_stdio`]) so it's
//! unit-testable without real stdio.

use serde_json::{json, Value};

/// A tool the app exposes to agents. `input_schema` is a JSON Schema object.
#[derive(Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl ToolDef {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

/// Implement this on your app to expose it over MCP. One concrete app per server
/// process, so the trait stays simple (no `dyn`, no object-safety gymnastics).
///
/// `async fn` in a trait is intentional here: we only ever use it generically (the
/// host is monomorphized per app), so the absent auto-trait bound is a non-issue.
#[allow(async_fn_in_trait)]
pub trait McpApp {
    /// Human-readable server name reported during `initialize`.
    fn server_name(&self) -> String {
        "mesh app".to_string()
    }
    /// The tools this app offers.
    fn tools(&self) -> Vec<ToolDef>;
    /// Handle a tool call. `Ok(value)` is returned to the agent as text content;
    /// `Err(msg)` is surfaced as an MCP tool error (isError = true).
    async fn call_tool(&self, name: &str, args: Value) -> Result<Value, String>;
}

/// The MCP protocol version we advertise (broadly supported).
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Handle ONE newline-delimited JSON-RPC message. Returns the response line to
/// write back, or `None` for notifications / unparseable input.
pub async fn handle_line<A: McpApp>(app: &A, line: &str) -> Option<String> {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        // Malformed JSON: per JSON-RPC, reply with a parse error (id null) rather than
        // silently dropping it (which hangs a client awaiting a response).
        Err(_) => {
            return Some(
                json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "parse error" } })
                    .to_string(),
            )
        }
    };
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    // No `id` => a notification: act if needed, but never respond.
    let id = req.get("id").cloned()?;

    let resp = match dispatch(app, method, params).await {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        }
    };
    Some(resp.to_string())
}

/// Core method dispatch. `Err((code, msg))` is a JSON-RPC protocol error; tool-level
/// failures are returned as `Ok` with `isError: true` (MCP convention).
async fn dispatch<A: McpApp>(app: &A, method: &str, params: Value) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": app.server_name(), "version": env!("CARGO_PKG_VERSION") },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let tools: Vec<Value> = app
                .tools()
                .into_iter()
                .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.input_schema }))
                .collect();
            Ok(json!({ "tools": tools }))
        }
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match app.call_tool(name, args).await {
                Ok(value) => {
                    let text = value
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| value.to_string());
                    Ok(json!({ "content": [ { "type": "text", "text": text } ], "isError": false }))
                }
                Err(message) => Ok(
                    json!({ "content": [ { "type": "text", "text": message } ], "isError": true }),
                ),
            }
        }
        "resources/list" => Ok(json!({ "resources": [] })),
        other => Err((-32601, format!("method not found: {other}"))),
    }
}

/// Run the MCP server over any newline-delimited byte stream: read JSON-RPC requests
/// from `reader`, write responses to `writer`. Blocks until the reader closes. This is
/// the transport-agnostic core; [`serve_stdio`] wraps stdin/stdout and a local TCP
/// bridge wraps a socket.
pub async fn serve_stream<A, R, W>(app: A, reader: R, mut writer: W) -> anyhow::Result<()>
where
    A: McpApp,
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(resp) = handle_line(&app, &line).await {
            writer.write_all(resp.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
    }
    Ok(())
}

/// Run the MCP server over stdio (the standard MCP local transport): read
/// newline-delimited JSON-RPC from stdin, write responses to stdout. Blocks until
/// stdin closes.
pub async fn serve_stdio<A: McpApp>(app: A) -> anyhow::Result<()> {
    serve_stream(app, tokio::io::stdin(), tokio::io::stdout()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy;
    impl McpApp for Dummy {
        fn server_name(&self) -> String {
            "dummy".to_string()
        }
        fn tools(&self) -> Vec<ToolDef> {
            vec![ToolDef::new(
                "echo",
                "echoes the msg argument",
                json!({ "type": "object", "properties": { "msg": { "type": "string" } } }),
            )]
        }
        async fn call_tool(&self, name: &str, args: Value) -> Result<Value, String> {
            match name {
                "echo" => Ok(args.get("msg").cloned().unwrap_or(json!(""))),
                other => Err(format!("unknown tool: {other}")),
            }
        }
    }

    #[tokio::test]
    async fn initialize_list_call_and_notify() {
        let app = Dummy;

        let r = handle_line(
            &app,
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize"}).to_string(),
        )
        .await
        .unwrap();
        assert!(r.contains("serverInfo") && r.contains("dummy"));

        let r = handle_line(
            &app,
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}).to_string(),
        )
        .await
        .unwrap();
        assert!(r.contains("echo"));

        let call = json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"echo","arguments":{"msg":"hi there"}}});
        let r = handle_line(&app, &call.to_string()).await.unwrap();
        assert!(r.contains("hi there") && r.contains("\"isError\":false"));

        // A notification (no id) yields no response.
        let n = handle_line(
            &app,
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
        )
        .await;
        assert!(n.is_none());

        // Unknown method is a JSON-RPC error.
        let r = handle_line(
            &app,
            &json!({"jsonrpc":"2.0","id":4,"method":"bogus"}).to_string(),
        )
        .await
        .unwrap();
        assert!(r.contains("-32601"));
    }

    #[tokio::test]
    async fn malformed_json_returns_parse_error() {
        let app = Dummy;
        // A malformed line must get a JSON-RPC parse error reply, not silence (which
        // would hang a client awaiting a response).
        let r = handle_line(&app, "this is not json {").await.unwrap();
        assert!(r.contains("-32700"), "malformed JSON yields a parse error");
    }
}
