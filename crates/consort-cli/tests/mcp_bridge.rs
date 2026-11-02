//! `consort mcp-bridge` forwards stdio to a session's local MCP endpoint.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use consort_proto::error::ErrorBody;
use consort_proto::ids::SessionId;
use consort_proto::model::{Sha256Hash, VisualHeight, VisualMode};
use consort_proto::session_host_link::ToolCall;
use consort_proto::tools::{HubTool, ToolCallResult, ToolDefinition};
use consort_session_host::mcp::{self, McpSession, ToolBackend};
use consort_session_host::visual::{RenderError, Rendered, VisualEngine};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Default)]
struct Backend {
    calls: Mutex<Vec<ToolCall>>,
}

#[async_trait]
impl ToolBackend for Backend {
    async fn list_tools(&self, _session_id: SessionId) -> Result<Vec<ToolDefinition>, ErrorBody> {
        Ok(vec![HubTool::PostUpdate.definition()])
    }
    async fn call_tool(&self, call: ToolCall) -> Result<ToolCallResult, ErrorBody> {
        self.calls.lock().unwrap().push(call);
        Ok(ToolCallResult::json(&json!({"ok": true})))
    }
    async fn upload_blob(&self, _sha256: &Sha256Hash, _bytes: Vec<u8>) -> Result<(), ErrorBody> {
        Ok(())
    }
    async fn download_blob(&self, _sha256: &Sha256Hash, _max: u64) -> Result<Vec<u8>, ErrorBody> {
        Ok(Vec::new())
    }
}

/// The bridge tests call no visual tool.
struct NoEngine;

#[async_trait]
impl VisualEngine for NoEngine {
    async fn render(
        &self,
        _document: String,
        _width: u32,
        _scale: u8,
        _mode: VisualMode,
        _max_height: u32,
    ) -> Result<Rendered, RenderError> {
        Err(RenderError::Installing)
    }
    async fn measure(
        &self,
        _document: String,
        _widths: &[u32],
        _mode: VisualMode,
    ) -> Result<Vec<VisualHeight>, RenderError> {
        Err(RenderError::Installing)
    }
}

async fn start(dir: &std::path::Path, backend: Arc<Backend>) -> mcp::McpEndpoint {
    mcp::start(McpSession {
        session_id: SessionId::new(),
        workspace: dir.to_path_buf(),
        backend,
        exec: None,
        worker_root: None,
        visual: Arc::new(NoEngine),
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn bridge_forwards_stdio_to_the_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(Backend::default());
    let endpoint = start(dir.path(), backend.clone()).await;

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_consort"))
        .args(["mcp-bridge", "--url", &endpoint.url])
        .env("CONSORT_MCP_TOKEN", &endpoint.token)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut send = async |v: Value| {
        stdin.write_all(format!("{v}\n").as_bytes()).await.unwrap();
        stdin.flush().await.unwrap();
    };
    send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}})).await;
    send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
    send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).await;
    send(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "post_update", "arguments": {"body": "Hi."}}})).await;

    let mut by_id = std::collections::HashMap::new();
    while by_id.len() < 3 {
        let line = tokio::time::timeout(Duration::from_secs(20), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        by_id.insert(value["id"].as_i64().unwrap(), value);
    }
    assert_eq!(by_id[&1]["result"]["serverInfo"]["name"], "consort");
    assert_eq!(by_id[&2]["result"]["tools"][0]["name"], "post_update");
    assert_eq!(by_id[&3]["result"]["isError"], false);
    assert_eq!(backend.calls.lock().unwrap()[0].arguments["body"], "Hi.");
    drop(stdin);
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    endpoint.stop().await;
}

/// The endpoint is on 127.0.0.1, so the bridge never sends the token
/// through a proxy, also when the proxy variables are set.
#[tokio::test]
async fn bridge_ignores_proxy_variables() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(Backend::default());
    let endpoint = start(dir.path(), backend.clone()).await;
    // A "proxy" that counts the requests that carry the session token.
    // It does not count every connection: other programs on a developer
    // machine sometimes connect to new local ports, and that made this
    // test fail about one run in six.
    let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    let connections = Arc::new(Mutex::new(0_usize));
    let counter = connections.clone();
    let token = endpoint.token.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = proxy.accept().await {
            let counter = counter.clone();
            let token = token.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut buf = vec![0_u8; 8192];
                let read =
                    tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await;
                if let Ok(Ok(n)) = read
                    && String::from_utf8_lossy(&buf[..n]).contains(&token)
                {
                    *counter.lock().unwrap() += 1;
                }
            });
        }
    });

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_consort"));
    command
        .args(["mcp-bridge", "--url", &endpoint.url])
        .env("CONSORT_MCP_TOKEN", &endpoint.token)
        .env_remove("NO_PROXY")
        .env_remove("no_proxy");
    for variable in ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
        command.env(variable, &proxy_url);
    }
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let initialize = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}});
    stdin
        .write_all(format!("{initialize}\n").as_bytes())
        .await
        .unwrap();
    stdin.flush().await.unwrap();
    let line = tokio::time::timeout(Duration::from_secs(20), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["result"]["serverInfo"]["name"], "consort", "{value}");
    assert_eq!(*connections.lock().unwrap(), 0, "nothing went to the proxy");
    drop(stdin);
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    endpoint.stop().await;
}
