//! `consort mcp-bridge` forwards stdio to a session's local MCP endpoint.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use consort_proto::error::ErrorBody;
use consort_proto::ids::SessionId;
use consort_proto::model::Sha256Hash;
use consort_proto::tools::{HubTool, ToolCallResult, ToolDefinition};
use consort_proto::worker_link::ToolCall;
use consort_worker::mcp::{self, ToolBackend};
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
}

#[tokio::test]
async fn bridge_forwards_stdio_to_the_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(Backend::default());
    let endpoint = mcp::start(SessionId::new(), dir.path().to_path_buf(), backend.clone())
        .await
        .unwrap();

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
    assert_eq!(by_id[&1]["result"]["serverInfo"]["name"], "consort-hub");
    assert_eq!(by_id[&2]["result"]["tools"][0]["name"], "post_update");
    assert_eq!(by_id[&3]["result"]["isError"], false);
    assert_eq!(backend.calls.lock().unwrap()[0].arguments["body"], "Hi.");
    drop(stdin);
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    endpoint.stop().await;
}
