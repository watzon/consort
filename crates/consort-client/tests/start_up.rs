//! Starting the client with files that cannot be read.

// Tests may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use consort_client::{Client, Connection, hub::ClientOptions, store::Store};

const CONFIG: &str = r#"{"hub_url":"http://127.0.0.1:1","client_id":"cli_00000000-0000-0000-0000-000000000001","name":"Test"}"#;

fn refused_reason(client: &Client) -> String {
    match client.state().connection.clone() {
        Connection::Refused { reason } => reason,
        other => panic!("expected Refused, got {other:?}"),
    }
}

/// Finding 15: a damaged key made `Client::start` fail, and the app left with
/// no window. It starts unpaired and says what to do.
#[test]
fn a_damaged_key_starts_the_client_with_a_reason_and_a_next_step() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("client.json"), CONFIG).unwrap();
    std::fs::write(dir.path().join("client.key"), "not a key").unwrap();
    let client =
        Client::start_with(Some(Store::new(dir.path())), ClientOptions::desktop()).unwrap();
    let reason = refused_reason(&client);
    assert!(reason.contains("client.key"), "{reason}");
    assert!(reason.contains("Pair this app again"), "{reason}");
    assert!(!reason.contains("Cannot save"), "{reason}");
}

/// With only one of the two files, the app looked unpaired and said nothing.
#[test]
fn one_missing_file_starts_the_client_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("client.json"), CONFIG).unwrap();
    let client =
        Client::start_with(Some(Store::new(dir.path())), ClientOptions::desktop()).unwrap();
    let reason = refused_reason(&client);
    assert!(reason.contains("client.key"), "{reason}");
    assert!(reason.contains("Pair this app again"), "{reason}");
}

#[test]
fn no_files_means_not_paired() {
    let dir = tempfile::tempdir().unwrap();
    let client =
        Client::start_with(Some(Store::new(dir.path())), ClientOptions::desktop()).unwrap();
    assert_eq!(client.state().connection, Connection::NotPaired);
}
