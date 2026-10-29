//! A JSON-RPC client over the stdio of `codex app-server`.
//!
//! Codex sends one JSON object per line and leaves out the `"jsonrpc"`
//! field. A message with `id` and `result` or `error` is a response, one with
//! `id` and `method` is a server request, and one with only `method` is a
//! notification.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use crate::HarnessError;

/// A message from Codex that is not a response.
#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    /// A notification.
    Notification {
        /// The method.
        method: String,
        /// The params.
        params: Value,
    },
    /// A server request that needs an answer.
    Request {
        /// The request ID, echoed in the answer.
        id: Value,
        /// The method.
        method: String,
        /// The params.
        params: Value,
    },
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, HarnessError>>>>>;

/// The client half of the connection.
#[derive(Debug, Clone)]
pub struct RpcClient {
    writer: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: Arc<AtomicI64>,
}

/// Sorts one line from Codex. Responses go to `pending`.
fn dispatch(line: &str, pending: &Pending, inbound: &mpsc::UnboundedSender<Inbound>) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        tracing::debug!(line, "codex app-server wrote a line that is not JSON");
        return;
    };
    crate::record::record("codex", &value);
    let id = value.get("id").cloned();
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    match (id, method) {
        (Some(id), Some(method)) => {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            let _ = inbound.send(Inbound::Request { id, method, params });
        }
        (None, Some(method)) => {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            let _ = inbound.send(Inbound::Notification { method, params });
        }
        (Some(id), None) => {
            let Some(id) = id.as_i64() else {
                return;
            };
            let sender = pending.lock().ok().and_then(|mut p| p.remove(&id));
            let Some(sender) = sender else {
                return;
            };
            let result = if let Some(error) = value.get("error") {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
                    .to_owned();
                Err(HarnessError::Protocol(message))
            } else {
                Ok(value.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = sender.send(result);
        }
        (None, None) => {}
    }
}

impl RpcClient {
    /// Starts the reader and writer tasks. The receiver gets every
    /// notification and server request, and closes when Codex closes its
    /// stdout.
    pub fn start(
        stdin: impl AsyncWrite + Send + Unpin + 'static,
        stdout: impl AsyncRead + Send + Unpin + 'static,
    ) -> (Self, mpsc::UnboundedReceiver<Inbound>) {
        let (writer, mut lines) = mpsc::unbounded_channel::<String>();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(mut line) = lines.recv().await {
                line.push('\n');
                if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });
        let reader_pending = pending.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                dispatch(&line, &reader_pending, &inbound_tx);
            }
            // Codex closed its stdout: fail every request that waits.
            if let Ok(mut pending) = reader_pending.lock() {
                for (_, sender) in pending.drain() {
                    let _ = sender.send(Err(HarnessError::Closed));
                }
            }
        });
        (
            Self {
                writer,
                pending,
                next_id: Arc::new(AtomicI64::new(1)),
            },
            inbound_rx,
        )
    }

    fn send(&self, value: &Value) -> Result<(), HarnessError> {
        self.writer
            .send(value.to_string())
            .map_err(|_| HarnessError::Closed)
    }

    /// Sends a request and waits for its result.
    pub async fn request<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<R, HarnessError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(id, tx);
        }
        let params =
            serde_json::to_value(params).map_err(|e| HarnessError::Protocol(e.to_string()))?;
        self.send(&json!({"id": id, "method": method, "params": params}))?;
        let value = rx.await.map_err(|_| HarnessError::Closed)??;
        serde_json::from_value(value).map_err(|e| {
            HarnessError::Protocol(format!("`{method}` returned an unexpected result: {e}"))
        })
    }

    /// Sends a notification.
    pub fn notify<P: Serialize>(&self, method: &str, params: &P) -> Result<(), HarnessError> {
        let params =
            serde_json::to_value(params).map_err(|e| HarnessError::Protocol(e.to_string()))?;
        if params.is_null() {
            self.send(&json!({"method": method}))
        } else {
            self.send(&json!({"method": method, "params": params}))
        }
    }

    /// Answers a server request.
    pub fn respond<R: Serialize>(&self, id: &Value, result: &R) -> Result<(), HarnessError> {
        let result =
            serde_json::to_value(result).map_err(|e| HarnessError::Protocol(e.to_string()))?;
        self.send(&json!({"id": id, "result": result}))
    }

    /// Answers a server request with an error.
    pub fn respond_error(&self, id: &Value, code: i64, message: &str) -> Result<(), HarnessError> {
        self.send(&json!({"id": id, "error": {"code": code, "message": message}}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_response_and_inbound_messages() {
        let (client_side, mut server_side) = tokio::io::duplex(64 * 1024);
        let (server_out, client_in) = tokio::io::duplex(64 * 1024);
        let (client, mut inbound) = RpcClient::start(client_side, client_in);
        let server = tokio::spawn(async move {
            let mut lines = BufReader::new(&mut server_side).lines();
            let line = lines.next_line().await.unwrap().unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "account/read");
            assert!(request.get("jsonrpc").is_none());
            let mut out = server_out;
            let text = format!(
                "{}\n{}\n{}\n",
                json!({"method": "turn/started", "params": {"x": 1}}),
                json!({"id": 7, "method": "item/tool/requestUserInput", "params": {}}),
                json!({"id": request["id"], "result": {"ok": true}})
            );
            out.write_all(text.as_bytes()).await.unwrap();
            out
        });
        let result: Value = client.request("account/read", &json!({})).await.unwrap();
        assert_eq!(result, json!({"ok": true}));
        assert_eq!(
            inbound.recv().await.unwrap(),
            Inbound::Notification {
                method: "turn/started".into(),
                params: json!({"x": 1})
            }
        );
        assert!(
            matches!(inbound.recv().await.unwrap(), Inbound::Request { id, .. } if id == json!(7))
        );
        let out = server.await.unwrap();
        drop(out);
        // The reader ends, so a new request fails.
        let err = client
            .request::<_, Value>("x", &json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, HarnessError::Closed));
    }
}
