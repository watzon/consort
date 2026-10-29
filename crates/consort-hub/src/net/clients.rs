//! Connected clients and the short-lived bearer tokens for blob requests.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use consort_proto::auth::{HTTP_TOKEN_TTL_MS, HttpToken};
use consort_proto::ids::ClientId;
use consort_proto::time::Timestamp;
use tokio_util::sync::CancellationToken;

use crate::util::random_token;

#[derive(Default)]
pub struct ClientRegistry {
    tokens: Mutex<HashMap<String, (ClientId, Timestamp)>>,
    connections: Mutex<HashMap<u64, (ClientId, CancellationToken)>>,
    next_connection: AtomicU64,
}

impl ClientRegistry {
    pub fn issue_token(&self, client: ClientId, now: Timestamp) -> HttpToken {
        let token = random_token();
        let expires_at = now.plus_millis(HTTP_TOKEN_TTL_MS);
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.retain(|_, (_, expires)| *expires > now);
            tokens.insert(token.clone(), (client, expires_at));
        }
        HttpToken { token, expires_at }
    }

    pub fn check_token(&self, token: &str, now: Timestamp) -> Option<ClientId> {
        let tokens = self.tokens.lock().ok()?;
        tokens
            .get(token)
            .filter(|(_, expires)| *expires > now)
            .map(|(client, _)| *client)
    }

    /// Registers a live connection. Returns an ID for [`ClientRegistry::unregister`].
    pub fn register(&self, client: ClientId, cancel: CancellationToken) -> u64 {
        let id = self.next_connection.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut connections) = self.connections.lock() {
            connections.insert(id, (client, cancel));
        }
        id
    }

    pub fn unregister(&self, id: u64) {
        if let Ok(mut connections) = self.connections.lock() {
            connections.remove(&id);
        }
    }

    /// Closes every connection of a client and forgets its tokens.
    pub fn disconnect(&self, client: ClientId) {
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.retain(|_, (id, _)| *id != client);
        }
        if let Ok(connections) = self.connections.lock() {
            for (id, cancel) in connections.values() {
                if *id == client {
                    cancel.cancel();
                }
            }
        }
    }
}
