use super::*;
use crate::bridge::McpHttpClient;
use consort_proto::tools::HubTool;
use serde_json::json;

#[derive(Default)]
struct MockBackend {
    calls: std::sync::Mutex<Vec<ToolCall>>,
    uploads: std::sync::Mutex<Vec<(Sha256Hash, usize)>>,
}

#[async_trait]
impl ToolBackend for MockBackend {
    async fn list_tools(&self, _session_id: SessionId) -> Result<Vec<ToolDefinition>, ErrorBody> {
        Ok(vec![
            HubTool::PostUpdate.definition(),
            HubTool::PublishArtifact.definition(),
        ])
    }

    async fn call_tool(&self, call: ToolCall) -> Result<ToolCallResult, ErrorBody> {
        self.calls.lock().unwrap().push(call.clone());
        Ok(ToolCallResult::json(
            &json!({"ok": true, "tool": call.name}),
        ))
    }

    async fn upload_blob(&self, sha256: &Sha256Hash, bytes: Vec<u8>) -> Result<(), ErrorBody> {
        self.uploads
            .lock()
            .unwrap()
            .push((sha256.clone(), bytes.len()));
        Ok(())
    }
}

fn initialize() -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}
    }})
}

async fn status(url: &str, token: Option<&str>) -> u16 {
    let mut request = reqwest::Client::new()
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .json(&initialize());
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    request.send().await.unwrap().status().as_u16()
}

#[tokio::test]
async fn missing_or_wrong_token_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MockBackend::default());
    let endpoint = start(SessionId::new(), dir.path().to_path_buf(), backend)
        .await
        .unwrap();
    assert!(endpoint.url.starts_with("http://127.0.0.1:"));
    assert_eq!(endpoint.token.len(), 64);
    assert_eq!(status(&endpoint.url, None).await, 401);
    assert_eq!(status(&endpoint.url, Some("wrong")).await, 401);
    let wrong_same_length = "0".repeat(64);
    assert_eq!(status(&endpoint.url, Some(&wrong_same_length)).await, 401);
    assert_eq!(status(&endpoint.url, Some(&endpoint.token)).await, 200);
    endpoint.stop().await;
}

#[tokio::test]
async fn lists_and_forwards_tools_and_uploads_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("report.md"), "# Report\n").unwrap();
    let backend = Arc::new(MockBackend::default());
    let session = SessionId::new();
    let endpoint = start(session, dir.path().to_path_buf(), backend.clone())
        .await
        .unwrap();
    let client = McpHttpClient::new(&endpoint.url, &endpoint.token);
    let init = client.post(&initialize()).await.unwrap();
    assert_eq!(init[0]["result"]["serverInfo"]["name"], "consort-hub");
    client
        .post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await
        .unwrap();

    let list = client
        .post(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
        .await
        .unwrap();
    let names: Vec<&str> = list[0]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["post_update", "publish_artifact"]);
    assert_eq!(
        list[0]["result"]["tools"][0]["inputSchema"]["type"],
        "object"
    );

    let call = client.post(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "post_update", "arguments": {"body": "Halfway."}}})).await.unwrap();
    assert_eq!(call[0]["result"]["isError"], false);
    assert_eq!(
        call[0]["result"]["structuredContent"]["tool"],
        "post_update"
    );

    let publish = client.post(&json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "publish_artifact", "arguments": {"path": "report.md", "title": "Report"}}})).await.unwrap();
    assert_eq!(publish[0]["result"]["isError"], false, "{publish:?}");
    let escape = client.post(&json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "publish_artifact", "arguments": {"path": "../../etc/passwd", "title": "x"}}})).await.unwrap();
    assert_eq!(escape[0]["result"]["isError"], true);

    let calls = backend.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].session_id, session);
    assert_eq!(calls[0].arguments["body"], "Halfway.");
    let args: PublishArtifactArgs = serde_json::from_value(calls[1].arguments.clone()).unwrap();
    assert_eq!(args.size, Some(9));
    assert_eq!(args.mime.as_deref(), Some("text/markdown"));
    let uploads = backend.uploads.lock().unwrap().clone();
    assert_eq!(uploads.len(), 1);
    assert_eq!(Some(uploads[0].0.clone()), args.blob_sha256);
    endpoint.stop().await;
}

#[test]
fn workspace_paths_stay_inside() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "x").unwrap();
    assert!(resolve_in_workspace(dir.path(), "a.txt").is_ok());
    assert!(resolve_in_workspace(dir.path(), "../a.txt").is_err());
    assert!(resolve_in_workspace(dir.path(), "missing.txt").is_err());
    let outside = tempfile::NamedTempFile::new().unwrap();
    assert!(resolve_in_workspace(dir.path(), &outside.path().to_string_lossy()).is_err());
}
