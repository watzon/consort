//! A small client for the local MCP endpoint, and the stdio bridge that
//! `consort mcp-bridge` runs.
//!
//! A harness that cannot take an HTTP MCP server gets a stdio server
//! instead: `consort mcp-bridge --url <endpoint>`, with the token in
//! `CONSORT_MCP_TOKEN`. The bridge reads JSON-RPC messages from stdin, posts
//! each one to the endpoint, and writes the answers to stdout.
//!
//! The harness starts the bridge from its MCP configuration, so the token
//! must be in that configuration: in an argument, a variable or a file
//! name. Each of these is visible to other processes of the same OS user,
//! so the bridge cannot keep the token from them (see DECISIONS.md). The
//! adapters use the bridge only for an agent that has no HTTP MCP support.

use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use crate::HostError;

/// The environment variable that holds the bearer token for the bridge.
pub const TOKEN_VAR: &str = "CONSORT_MCP_TOKEN";

/// Reads the JSON messages of a `text/event-stream` body.
pub fn parse_sse(body: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut data = String::new();
    for line in body.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if !data.is_empty() {
                if let Ok(value) = serde_json::from_str::<Value>(&data) {
                    out.push(value);
                }
                data.clear();
            }
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    out
}

/// A client for one streamable HTTP MCP endpoint.
#[derive(Debug, Clone)]
pub struct McpHttpClient {
    url: String,
    token: String,
    http: reqwest::Client,
    session: Arc<Mutex<Option<String>>>,
    protocol: Arc<Mutex<Option<String>>>,
}

impl McpHttpClient {
    /// A client for `url` with a bearer token. The client ignores the
    /// proxy variables (`HTTP_PROXY` and the others), because the endpoint
    /// is on 127.0.0.1 and a proxy would see the token.
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Result<Self, HostError> {
        let http = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|e| HostError::Network(format!("could not make an HTTP client: {e}")))?;
        Ok(Self {
            url: url.into(),
            token: token.into(),
            http,
            session: Arc::new(Mutex::new(None)),
            protocol: Arc::new(Mutex::new(None)),
        })
    }

    /// Posts one JSON-RPC message and returns the messages that came back.
    /// Returns the HTTP status as an error when it is not a success.
    pub async fn post(&self, message: &Value) -> Result<Vec<Value>, HostError> {
        let mut request = self
            .http
            .post(&self.url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/json, text/event-stream")
            .json(message);
        if let Some(session) = self.session.lock().await.clone() {
            request = request.header("Mcp-Session-Id", session);
        }
        if let Some(version) = self.protocol.lock().await.clone() {
            request = request.header("MCP-Protocol-Version", version);
        }
        let response = request
            .send()
            .await
            .map_err(|e| HostError::Network(e.to_string()))?;
        let status = response.status();
        if let Some(id) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            *self.session.lock().await = Some(id.to_owned());
        }
        if !status.is_success() {
            return Err(HostError::Network(format!(
                "the MCP endpoint answered HTTP {status}"
            )));
        }
        let is_sse = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        let body = response
            .text()
            .await
            .map_err(|e| HostError::Network(e.to_string()))?;
        let values = if body.trim().is_empty() {
            Vec::new()
        } else if is_sse {
            parse_sse(&body)
        } else {
            match serde_json::from_str::<Value>(&body) {
                Ok(Value::Array(items)) => items,
                Ok(value) => vec![value],
                Err(_) => Vec::new(),
            }
        };
        for value in &values {
            if let Some(version) = value
                .get("result")
                .and_then(|r| r.get("protocolVersion"))
                .and_then(Value::as_str)
            {
                *self.protocol.lock().await = Some(version.to_owned());
            }
        }
        Ok(values)
    }
}

/// Runs the stdio bridge until stdin closes.
pub async fn run_stdio(url: &str, token: &str) -> Result<(), HostError> {
    let client = McpHttpClient::new(url, token)?;
    let stdout = Arc::new(Mutex::new(tokio::io::stdout()));
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut first = true;
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| HostError::io("could not read stdin", e))?
    {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let client = client.clone();
        let stdout = stdout.clone();
        let work = async move {
            let answers = match client.post(&message).await {
                Ok(a) => a,
                Err(err) => match message.get("id") {
                    Some(id) => vec![serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": -32603, "message": err.to_string()}
                    })],
                    None => Vec::new(),
                },
            };
            let mut out = stdout.lock().await;
            for answer in answers {
                let _ = out.write_all(format!("{answer}\n").as_bytes()).await;
            }
            let _ = out.flush().await;
        };
        // The first message is `initialize`. Wait for it, so later messages
        // carry the session ID.
        if first {
            work.await;
            first = false;
        } else {
            tokio::spawn(work);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_parsing() {
        let body = ": ping\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\nid: 2\ndata: {\"a\":\ndata: 1}\n";
        let values = parse_sse(body);
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["id"], 1);
        assert_eq!(values[1]["a"], 1);
    }
}
