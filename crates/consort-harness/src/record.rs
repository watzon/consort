//! Optional recording of raw harness messages, for mapper fixtures.
//!
//! Set `CONSORT_HARNESS_RECORD` to a file path. Each adapter then appends
//! every message that it receives from its harness to that file, one JSON
//! object per line. Recordings can hold prompt text, so this is for
//! development only.

use std::io::Write;

use serde_json::Value;

/// The environment variable that turns recording on.
pub const RECORD_VAR: &str = "CONSORT_HARNESS_RECORD";

/// Appends one message to the recording, if recording is on.
pub fn record(source: &str, message: &Value) {
    let Some(path) = std::env::var_os(RECORD_VAR) else {
        return;
    };
    let line = serde_json::json!({"source": source, "message": message});
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}
