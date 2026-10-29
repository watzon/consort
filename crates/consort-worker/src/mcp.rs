//! The local MCP endpoint of one session.
//!
//! Each session gets its own streamable HTTP MCP server on `127.0.0.1` with
//! a random port and a random 32-byte bearer token. Only that session's
//! harness gets the token. The worker is the MCP server: it gets the tool
//! list from the hub with `tool.list` and forwards each call as `tool.call`.
//! For `publish_artifact`, the worker uploads the file first and adds the
//! blob hash to the call.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::ids::SessionId;
use consort_proto::model::Sha256Hash;
use consort_proto::tools::{PublishArtifactArgs, ToolCallResult, ToolContent, ToolDefinition};
use consort_proto::worker_link::ToolCall;
use rand::RngCore;
use rmcp::ErrorData as McpError;
use rmcp::RoleServer;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::WorkerError;

/// The most bytes that `publish_artifact` uploads (64 MiB).
pub const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;

/// The hub tools of a session, as `tool.list` gave them. Empty until the
/// harness lists the tools for the first time.
pub type ToolCache = Arc<Mutex<Option<Vec<ToolDefinition>>>>;

/// What the endpoint needs from the rest of the worker.
#[async_trait]
pub trait ToolBackend: Send + Sync {
    /// The hub tools of a session (`tool.list`).
    async fn list_tools(&self, session_id: SessionId) -> Result<Vec<ToolDefinition>, ErrorBody>;
    /// Runs a hub tool (`tool.call`).
    async fn call_tool(&self, call: ToolCall) -> Result<ToolCallResult, ErrorBody>;
    /// Uploads a blob to the hub.
    async fn upload_blob(&self, sha256: &Sha256Hash, bytes: Vec<u8>) -> Result<(), ErrorBody>;
}

/// A running MCP endpoint.
#[derive(Debug)]
pub struct McpEndpoint {
    /// The URL, `http://127.0.0.1:<port>/mcp/<session_id>`.
    pub url: String,
    /// The bearer token.
    pub token: String,
    tools: ToolCache,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl McpEndpoint {
    /// The hub tools that this endpoint gave the harness. The worker allows
    /// an approval for a hub tool only when the tool is in this list.
    pub fn tools(&self) -> ToolCache {
        self.tools.clone()
    }

    /// Stops the server.
    pub async fn stop(self) {
        self.cancel.cancel();
        let mut task = self.task;
        if tokio::time::timeout(std::time::Duration::from_secs(2), &mut task)
            .await
            .is_err()
        {
            task.abort();
        }
    }
}

/// A random 32-byte token as hex.
pub fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compares two strings in time that does not depend on where they differ.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0_u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn require_bearer(
    State(token): State<Arc<String>>,
    request: Request,
    next: Next,
) -> Response {
    let ok = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|given| constant_time_eq(given.trim().as_bytes(), token.as_bytes()));
    if ok {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            "A valid bearer token is required.",
        )
            .into_response()
    }
}

/// Starts the endpoint of a session.
pub async fn start(
    session_id: SessionId,
    workspace: PathBuf,
    backend: Arc<dyn ToolBackend>,
) -> Result<McpEndpoint, WorkerError> {
    let token = random_token();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| WorkerError::io("could not open the local MCP endpoint", e))?;
    let addr = listener
        .local_addr()
        .map_err(|e| WorkerError::io("could not read the MCP endpoint address", e))?;
    let cancel = CancellationToken::new();
    let tools: ToolCache = Arc::new(Mutex::new(None));
    let server = HubToolsServer {
        session_id,
        workspace,
        backend,
        tools: tools.clone(),
        calls: Arc::new(AtomicU64::new(0)),
    };
    let mut config = StreamableHttpServerConfig::default();
    config.cancellation_token = cancel.child_token();
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let path = format!("/mcp/{session_id}");
    let router = axum::Router::new().nest_service(&path, service).layer(
        axum::middleware::from_fn_with_state(Arc::new(token.clone()), require_bearer),
    );
    let stop = cancel.clone();
    let task = tokio::spawn(async move {
        let result = axum::serve(listener, router)
            .with_graceful_shutdown(async move { stop.cancelled().await })
            .await;
        if let Err(err) = result {
            tracing::warn!(error = %err, "the local MCP endpoint stopped");
        }
    });
    Ok(McpEndpoint {
        url: format!("http://{addr}{path}"),
        token,
        tools,
        cancel,
        task,
    })
}

#[derive(Clone)]
struct HubToolsServer {
    session_id: SessionId,
    workspace: PathBuf,
    backend: Arc<dyn ToolBackend>,
    tools: ToolCache,
    calls: Arc<AtomicU64>,
}

fn to_mcp_tool(def: &ToolDefinition) -> Tool {
    let schema = match &def.input_schema {
        Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    Tool::new(def.name.clone(), def.description.clone(), Arc::new(schema))
}

fn to_mcp_result(result: ToolCallResult) -> CallToolResult {
    let content: Vec<ContentBlock> = result
        .content
        .into_iter()
        .map(|c| match c {
            ToolContent::Text { text } => ContentBlock::text(text),
        })
        .collect();
    let mut out = if result.is_error {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    };
    out.structured_content = result.structured_content;
    out
}

/// A media type from a file name.
pub fn guess_mime(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "txt" | "log" | "rs" | "ts" | "js" | "py" | "toml" | "yaml" | "yml" => "text/plain",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// Resolves a path that the model gave against the workspace. The result
/// must stay inside the workspace.
pub fn resolve_in_workspace(workspace: &Path, given: &str) -> Result<PathBuf, ErrorBody> {
    let candidate = Path::new(given);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        if candidate
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(ErrorBody::invalid("`path` must not leave the workspace"));
        }
        workspace.join(candidate)
    };
    let real = std::fs::canonicalize(&joined).map_err(|_| {
        ErrorBody::new(
            ErrorCode::NotFound,
            format!("`{given}` does not exist in the workspace"),
        )
    })?;
    let root = std::fs::canonicalize(workspace).map_err(|_| ErrorBody::internal())?;
    if !real.starts_with(&root) {
        return Err(ErrorBody::invalid("`path` must be inside the workspace"));
    }
    if !real.is_file() {
        return Err(ErrorBody::invalid(format!("`{given}` is not a file")));
    }
    Ok(real)
}

impl HubToolsServer {
    async fn tools(&self) -> Result<Vec<ToolDefinition>, ErrorBody> {
        let mut cache = self.tools.lock().await;
        if let Some(tools) = cache.as_ref() {
            return Ok(tools.clone());
        }
        let tools = self.backend.list_tools(self.session_id).await?;
        *cache = Some(tools.clone());
        Ok(tools)
    }

    async fn publish_arguments(&self, arguments: Value) -> Result<Value, ErrorBody> {
        let mut args: PublishArtifactArgs = serde_json::from_value(arguments).map_err(|e| {
            ErrorBody::invalid(format!("arguments of `publish_artifact` are invalid: {e}"))
        })?;
        let path = resolve_in_workspace(&self.workspace, &args.path)?;
        let size = std::fs::metadata(&path)
            .map(|m| m.len())
            .map_err(|_| ErrorBody::internal())?;
        if size > MAX_ARTIFACT_BYTES {
            return Err(ErrorBody::invalid(format!(
                "the file is larger than {MAX_ARTIFACT_BYTES} bytes"
            )));
        }
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|_| ErrorBody::internal())?;
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        let sha = Sha256Hash::from_bytes(digest);
        self.backend.upload_blob(&sha, bytes).await?;
        args.blob_sha256 = Some(sha);
        args.size = Some(size);
        args.mime = Some(guess_mime(&path).to_owned());
        serde_json::to_value(&args).map_err(|_| ErrorBody::internal())
    }

    async fn call(&self, name: String, arguments: Value) -> ToolCallResult {
        let arguments = if name == "publish_artifact" {
            match self.publish_arguments(arguments).await {
                Ok(a) => a,
                Err(e) => return ToolCallResult::error(&e),
            }
        } else {
            arguments
        };
        let call_id = format!(
            "{}-{}",
            self.session_id,
            self.calls.fetch_add(1, Ordering::Relaxed) + 1
        );
        let call = ToolCall {
            session_id: self.session_id,
            call_id,
            name,
            arguments,
        };
        match self.backend.call_tool(call).await {
            Ok(result) => result,
            Err(error) => ToolCallResult::error(&error),
        }
    }
}

impl ServerHandler for HubToolsServer {
    fn get_info(&self) -> ServerConfig {
        let mut info = Implementation::from_build_env();
        info.name = "consort-hub".to_owned();
        info.title = Some("Consort hub tools".to_owned());
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(info)
            .with_instructions(
                "The hub tools let you post messages, start tasks, ask decisions and use memory.",
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let tools = self.tools().await.map_err(|e| {
            McpError::internal_error(format!("could not get the hub tools: {}", e.message), None)
        })?;
        Ok(ListToolsResult::with_all_items(
            tools.iter().map(to_mcp_tool).collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let arguments = request
            .arguments
            .map(Value::Object)
            .unwrap_or(Value::Object(Default::default()));
        let result = self.call(request.name.into_owned(), arguments).await;
        Ok(CallToolResponse::Complete(to_mcp_result(result)))
    }
}

#[cfg(test)]
mod tests;
