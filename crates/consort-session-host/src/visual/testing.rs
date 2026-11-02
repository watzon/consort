//! Fakes for the tests of the visual tools and of `visual.render`: an engine
//! with no browser, and a backend that keeps blobs in memory.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::ids::SessionId;
use consort_proto::model::{Sha256Hash, VisualHeight, VisualMode};
use consort_proto::session_host_link::ToolCall;
use consort_proto::tools::{ToolCallResult, ToolDefinition, VisualConsoleMessage};
use serde_json::json;

use super::{RenderError, Rendered, VisualEngine};
use crate::mcp::ToolBackend;

/// A PNG signature and an IHDR chunk: enough for a test to see a PNG.
pub(crate) const FAKE_PNG: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 2,
    0xD0, 0, 0, 0, 0xF0,
];

/// One call of a [`FakeEngine`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EngineCall {
    Render {
        document: String,
        width: u32,
        scale: u8,
        mode: VisualMode,
        max_height: u32,
    },
    Measure {
        document: String,
        widths: Vec<u32>,
        mode: VisualMode,
    },
}

/// An engine with no browser. It gives the same height at each width, or
/// the error that the test set.
pub(crate) struct FakeEngine {
    pub height: u32,
    pub error: Option<RenderError>,
    pub calls: Mutex<Vec<EngineCall>>,
}

impl FakeEngine {
    pub(crate) fn new(height: u32) -> Self {
        Self {
            height,
            error: None,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn failing(error: RenderError) -> Self {
        Self {
            error: Some(error),
            ..Self::new(1)
        }
    }

    pub(crate) fn calls(&self) -> Vec<EngineCall> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl VisualEngine for FakeEngine {
    async fn render(
        &self,
        document: String,
        width: u32,
        scale: u8,
        mode: VisualMode,
        max_height: u32,
    ) -> Result<Rendered, RenderError> {
        self.calls.lock().unwrap().push(EngineCall::Render {
            document,
            width,
            scale,
            mode,
            max_height,
        });
        if let Some(err) = &self.error {
            return Err(err.clone());
        }
        Ok(Rendered {
            png: FAKE_PNG.to_vec(),
            height: self.height,
            console: vec![VisualConsoleMessage {
                level: "log".to_owned(),
                text: "drawn".to_owned(),
            }],
            omitted: 0,
        })
    }

    async fn measure(
        &self,
        document: String,
        widths: &[u32],
        mode: VisualMode,
    ) -> Result<Vec<VisualHeight>, RenderError> {
        self.calls.lock().unwrap().push(EngineCall::Measure {
            document,
            widths: widths.to_vec(),
            mode,
        });
        if let Some(err) = &self.error {
            return Err(err.clone());
        }
        Ok(widths
            .iter()
            .map(|&width| VisualHeight {
                width,
                height: self.height,
            })
            .collect())
    }
}

/// A backend that lists the tools that the test gives, records the calls,
/// and keeps the blobs in memory.
#[derive(Default)]
pub(crate) struct MemoryBackend {
    pub tools: Vec<ToolDefinition>,
    pub calls: Mutex<Vec<ToolCall>>,
    /// The blobs by hash: the ones that the test serves and the uploads.
    pub blobs: Mutex<HashMap<String, Vec<u8>>>,
    /// The hashes of the uploads, in order.
    pub uploads: Mutex<Vec<Sha256Hash>>,
    /// The hashes of the downloads, in order.
    pub downloads: Mutex<Vec<Sha256Hash>>,
}

impl MemoryBackend {
    pub(crate) fn with_tools(tools: Vec<ToolDefinition>) -> Self {
        Self {
            tools,
            ..Self::default()
        }
    }

    /// Serves a blob, and returns its hash.
    pub(crate) fn serve(&self, bytes: &[u8]) -> Sha256Hash {
        let sha = super::command::blob_hash(bytes);
        self.blobs
            .lock()
            .unwrap()
            .insert(sha.to_string(), bytes.to_vec());
        sha
    }

    pub(crate) fn blob(&self, sha: &Sha256Hash) -> Option<Vec<u8>> {
        self.blobs.lock().unwrap().get(sha.as_str()).cloned()
    }

    pub(crate) fn calls(&self) -> Vec<ToolCall> {
        self.calls.lock().unwrap().clone()
    }

    pub(crate) fn uploads(&self) -> Vec<Sha256Hash> {
        self.uploads.lock().unwrap().clone()
    }
}

#[async_trait]
impl ToolBackend for MemoryBackend {
    async fn list_tools(&self, _session_id: SessionId) -> Result<Vec<ToolDefinition>, ErrorBody> {
        Ok(self.tools.clone())
    }

    async fn call_tool(&self, call: ToolCall) -> Result<ToolCallResult, ErrorBody> {
        self.calls.lock().unwrap().push(call.clone());
        Ok(ToolCallResult::json(
            &json!({"ok": true, "tool": call.name}),
        ))
    }

    async fn upload_blob(&self, sha256: &Sha256Hash, bytes: Vec<u8>) -> Result<(), ErrorBody> {
        self.uploads.lock().unwrap().push(sha256.clone());
        self.blobs.lock().unwrap().insert(sha256.to_string(), bytes);
        Ok(())
    }

    async fn download_blob(
        &self,
        sha256: &Sha256Hash,
        max_bytes: u64,
    ) -> Result<Vec<u8>, ErrorBody> {
        self.downloads.lock().unwrap().push(sha256.clone());
        let bytes = self
            .blob(sha256)
            .ok_or_else(|| ErrorBody::new(ErrorCode::NotFound, "no such blob"))?;
        if bytes.len() as u64 > max_bytes {
            return Err(ErrorBody::invalid("the blob is too large"));
        }
        Ok(bytes)
    }
}
