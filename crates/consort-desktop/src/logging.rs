//! The log setup of the app.
//!
//! The app writes its log to stderr, at `info` unless `RUST_LOG` says
//! otherwise. The WebSocket libraries are capped at `info` whatever
//! `RUST_LOG` says (`consort_proto::logs`), because their `trace` level logs
//! whole frames, and a frame can hold a key.

use tracing_subscriber::EnvFilter;

/// The filter: `RUST_LOG`, or `info`, with the WebSocket cap added.
fn filter(from_env: Result<EnvFilter, tracing_subscriber::filter::FromEnvError>) -> EnvFilter {
    let mut filter = from_env.unwrap_or_else(|_| EnvFilter::new("info"));
    for directive in consort_proto::logs::WEBSOCKET_LOG_CAP {
        // The directives are constants, and a test parses them.
        if let Ok(parsed) = directive.parse() {
            filter = filter.add_directive(parsed);
        }
    }
    filter
}

/// Installs the subscriber. A second call does nothing.
pub fn init() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter(EnvFilter::try_from_default_env()))
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_caps_the_websocket_libraries_at_info_also_at_trace() {
        let text = filter(Ok(EnvFilter::new("trace"))).to_string();
        assert!(text.contains("tungstenite=info"), "{text}");
        assert!(text.contains("tokio_tungstenite=info"), "{text}");
    }

    #[test]
    fn every_directive_of_the_cap_parses() {
        for directive in consort_proto::logs::WEBSOCKET_LOG_CAP {
            assert!(
                directive
                    .parse::<tracing_subscriber::filter::Directive>()
                    .is_ok()
            );
        }
    }
}
