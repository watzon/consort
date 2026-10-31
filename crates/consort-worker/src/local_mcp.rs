//! A small client for the local MCP servers of a worker. Each runs on this
//! machine over streamable HTTP, and a call starts a new short exchange, so a
//! server that restarted works again at the next call.

use serde_json::{Value, json};

use crate::tools::ToolError;

const PROTOCOL_VERSION: &str = "2025-06-18";

fn failed(server: &str, err: impl std::fmt::Display) -> ToolError {
    ToolError::new(
        "local_mcp_failed",
        format!("The local MCP server `{server}` failed: {err}"),
    )
}

/// The JSON-RPC messages in a reply, in plain JSON or as server-sent events.
fn messages(body: &str, is_sse: bool) -> Vec<Value> {
    if !is_sse {
        return serde_json::from_str(body).map_or_else(|_| Vec::new(), |v| vec![v]);
    }
    let mut out = Vec::new();
    for event in body.split("\n\n") {
        let data: Vec<&str> = event
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(str::trim_start)
            .collect();
        if let Ok(value) = serde_json::from_str(&data.join("\n")) {
            out.push(value);
        }
    }
    out
}

struct Exchange {
    http: reqwest::Client,
    url: String,
    server: String,
    session: Option<String>,
}

impl Exchange {
    async fn post(&mut self, message: &Value) -> Result<Vec<Value>, ToolError> {
        let mut request = self
            .http
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .json(message);
        if let Some(session) = &self.session {
            request = request.header("Mcp-Session-Id", session);
        }
        let response = request.send().await.map_err(|e| failed(&self.server, e))?;
        if let Some(id) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            self.session = Some(id.to_owned());
        }
        let status = response.status();
        if !status.is_success() {
            return Err(failed(&self.server, format!("it answered HTTP {status}")));
        }
        let is_sse = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        let body = response.text().await.map_err(|e| failed(&self.server, e))?;
        Ok(messages(&body, is_sse))
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, ToolError> {
        let id = uuid::Uuid::now_v7().to_string();
        let replies = self
            .post(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        let reply = replies
            .into_iter()
            .find(|m| m.get("id").and_then(Value::as_str) == Some(id.as_str()))
            .ok_or_else(|| failed(&self.server, "it sent no answer"))?;
        if let Some(error) = reply.get("error") {
            return Err(failed(&self.server, error));
        }
        Ok(reply.get("result").cloned().unwrap_or(Value::Null))
    }
}

async fn open(server: &str, url: &str) -> Result<Exchange, ToolError> {
    // The server is on this machine. A proxy would see what it says.
    let http = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|e| failed(server, e))?;
    let mut exchange = Exchange {
        http,
        url: url.to_owned(),
        server: server.to_owned(),
        session: None,
    };
    exchange
        .call(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "consort-worker", "version": env!("CARGO_PKG_VERSION")},
            }),
        )
        .await?;
    exchange
        .post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await?;
    Ok(exchange)
}

/// The tools of one server.
pub async fn list_tools(server: &str, url: &str) -> Result<Value, ToolError> {
    let mut exchange = open(server, url).await?;
    let result = exchange.call("tools/list", json!({})).await?;
    Ok(result.get("tools").cloned().unwrap_or_else(|| json!([])))
}

/// Calls one tool. An unknown tool or a failed call is the server's answer.
pub async fn call_tool(
    server: &str,
    url: &str,
    tool: &str,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let mut exchange = open(server, url).await?;
    exchange
        .call("tools/call", json!({"name": tool, "arguments": arguments}))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_reply_forms_are_read() {
        assert_eq!(messages(r#"{"id":1}"#, false), vec![json!({"id": 1})]);
        assert_eq!(
            messages("event: message\ndata: {\"id\":2}\n\n: ping\n\n", true),
            vec![json!({"id": 2})]
        );
        assert!(messages("nope", false).is_empty());
    }
}
