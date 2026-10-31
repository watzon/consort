//! The egress proxy of one session.
//!
//! The boundary denies all network access except for the session's MCP
//! endpoint and this proxy. The proxy takes HTTP `CONNECT` requests for the
//! hosts in its allow list. It denies everything else, and logs each denied
//! host. A plain HTTP request is always denied. See
//! `docs/research/hub-boundary-experiment.md`.

use std::collections::BTreeSet;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::HarnessKind;

/// The most bytes of a request head.
const MAX_HEAD_BYTES: usize = 16 * 1024;
/// How long a client may take to send its request head.
const HEAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The hosts that a session of this harness may reach.
///
/// Claude Code uses `api.anthropic.com` for the model and
/// `platform.claude.com` for a token refresh (the token URL in Claude Code
/// 2.1.314). A harness that is not proven has no hosts.
pub fn provider_hosts(kind: HarnessKind) -> Vec<String> {
    match kind {
        HarnessKind::ClaudeCode => vec![
            "api.anthropic.com".to_owned(),
            "platform.claude.com".to_owned(),
        ],
        // The model, and the token refresh of a ChatGPT sign-in.
        HarnessKind::Codex => vec![
            "chatgpt.com".to_owned(),
            "api.openai.com".to_owned(),
            "auth.openai.com".to_owned(),
        ],
        _ => Vec::new(),
    }
}

/// A running proxy. It stops when this value drops.
#[derive(Debug)]
pub struct EgressProxy {
    port: u16,
    task: JoinHandle<()>,
}

impl EgressProxy {
    /// Starts a proxy on `127.0.0.1` with a random port.
    pub async fn start(allow: Vec<String>) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let allow: Arc<BTreeSet<String>> = Arc::new(
            allow
                .into_iter()
                .map(|host| host.to_ascii_lowercase())
                .collect(),
        );
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let allow = allow.clone();
                tokio::spawn(async move {
                    handle(stream, &allow).await;
                });
            }
        });
        Ok(Self { port, task })
    }

    /// The port of the proxy.
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for EgressProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The host and port of a `CONNECT` request head that the allow list lets
/// through. `Err` has the target to log.
fn allowed_target(head: &str, allow: &BTreeSet<String>) -> Result<(String, u16), String> {
    let first = head.lines().next().unwrap_or_default();
    let mut parts = first.split_ascii_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if method != "CONNECT" {
        return Err(format!("{method} {target}"));
    }
    let Some((host, port)) = target.rsplit_once(':') else {
        return Err(target.to_owned());
    };
    let host = host.to_ascii_lowercase();
    match port.parse::<u16>() {
        Ok(port) if port == 443 && allow.contains(&host) => Ok((host, port)),
        _ => Err(target.to_owned()),
    }
}

/// Whether a refused request asks the proxy itself: its target is a path, not
/// a host or a full address.
fn to_the_proxy(refused: &str) -> bool {
    refused
        .split_ascii_whitespace()
        .nth(1)
        .is_some_and(|target| target.starts_with('/'))
}

async fn handle(mut client: TcpStream, allow: &BTreeSet<String>) {
    let mut buffer = Vec::with_capacity(1024);
    let read_head = async {
        let mut chunk = [0_u8; 1024];
        while !buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            if buffer.len() > MAX_HEAD_BYTES {
                return false;
            }
            match client.read(&mut chunk).await {
                Ok(0) | Err(_) => return false,
                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            }
        }
        true
    };
    if !matches!(
        tokio::time::timeout(HEAD_TIMEOUT, read_head).await,
        Ok(true)
    ) {
        return;
    }
    let end = buffer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map_or(buffer.len(), |at| at + 4);
    let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
    let (host, port) = match allowed_target(&head, allow) {
        Ok(target) => target,
        Err(target) => {
            // Claude Code asks the proxy itself for `GET /` every few seconds,
            // to see that it is there. That request goes nowhere. It is not a
            // try to reach the outside.
            if to_the_proxy(&target) {
                tracing::debug!(%target, "the egress proxy refused a request to itself");
            } else {
                tracing::warn!(%target, "the egress proxy denied a request");
            }
            let _ = client
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                .await;
            return;
        }
    };
    let mut upstream = match TcpStream::connect((host.as_str(), port)).await {
        Ok(upstream) => upstream,
        Err(err) => {
            tracing::warn!(%host, error = %err, "the egress proxy could not reach a host");
            let _ = client
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .await;
            return;
        }
    };
    if client
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await
        .is_err()
    {
        return;
    }
    if upstream.write_all(&buffer[end..]).await.is_err() {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ask(port: u16, head: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.write_all(head.as_bytes()).await.unwrap();
        let mut out = vec![0_u8; 256];
        let n = stream.read(&mut out).await.unwrap();
        String::from_utf8_lossy(&out[..n]).into_owned()
    }

    #[test]
    fn only_a_request_to_the_proxy_itself_is_not_a_denied_host() {
        assert!(to_the_proxy("GET /"));
        assert!(!to_the_proxy("GET http://api.anthropic.com/"));
        assert!(!to_the_proxy("example.com:443"));
    }

    #[test]
    fn only_listed_hosts_on_443_pass() {
        let allow: BTreeSet<String> = ["api.anthropic.com".to_owned()].into();
        let head = |t: &str| format!("CONNECT {t} HTTP/1.1\r\nHost: {t}\r\n\r\n");
        assert_eq!(
            allowed_target(&head("API.anthropic.com:443"), &allow),
            Ok(("api.anthropic.com".to_owned(), 443))
        );
        assert!(allowed_target(&head("api.anthropic.com:80"), &allow).is_err());
        assert!(allowed_target(&head("example.com:443"), &allow).is_err());
        assert!(allowed_target(&head("127.0.0.1:443"), &allow).is_err());
        assert!(allowed_target("GET http://api.anthropic.com/ HTTP/1.1\r\n\r\n", &allow).is_err());
    }

    #[tokio::test]
    async fn a_denied_host_gets_403_and_loopback_is_denied() {
        // A real service on loopback, which the session must not reach.
        let hub = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let hub_port = hub.local_addr().unwrap().port();
        let proxy = EgressProxy::start(vec!["api.anthropic.com".to_owned()])
            .await
            .unwrap();
        let reply = ask(
            proxy.port(),
            &format!("CONNECT 127.0.0.1:{hub_port} HTTP/1.1\r\n\r\n"),
        )
        .await;
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        let reply = ask(proxy.port(), "GET http://example.com/ HTTP/1.1\r\n\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        let reply = ask(proxy.port(), "CONNECT example.com:443 HTTP/1.1\r\n\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
    }

    #[test]
    fn claude_can_reach_its_model_and_refresh_hosts_and_others_nothing() {
        let hosts = provider_hosts(HarnessKind::ClaudeCode);
        assert!(hosts.contains(&"api.anthropic.com".to_owned()));
        assert!(hosts.contains(&"platform.claude.com".to_owned()));
        assert!(provider_hosts(HarnessKind::Codex).contains(&"chatgpt.com".to_owned()));
        assert!(provider_hosts(HarnessKind::GeminiCli).is_empty());
    }
}
