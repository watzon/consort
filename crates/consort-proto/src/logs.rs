//! The log rule that every Consort program shares.
//!
//! At its `trace` level, the WebSocket library logs whole frames. A frame can
//! hold a secret that a client sends to the hub, such as the Composio key. So
//! a program never logs these libraries below `info`, whatever `RUST_LOG` says.
//! A program adds these directives to its filter after it reads `RUST_LOG`.

/// Filter directives that keep the WebSocket libraries at `info`.
pub const WEBSOCKET_LOG_CAP: [&str; 2] = ["tungstenite=info", "tokio_tungstenite=info"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cap_names_both_websocket_libraries_at_info() {
        assert!(WEBSOCKET_LOG_CAP.contains(&"tungstenite=info"));
        assert!(WEBSOCKET_LOG_CAP.contains(&"tokio_tungstenite=info"));
    }
}
