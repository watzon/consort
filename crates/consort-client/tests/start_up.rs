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

fn write_saved(dir: &std::path::Path, hub_url: &str) {
    let config = CONFIG.replace("http://127.0.0.1:1", hub_url);
    std::fs::write(dir.join("client.json"), config).unwrap();
    std::fs::write(dir.join("client.key"), "11".repeat(32)).unwrap();
}

/// Finding 4a of review cycle 2: the `http` rule ran only when the user
/// paired. A saved `client.json` with `http` to a public host was used as it
/// was. The saved URL gets the same checks as a typed one.
#[test]
fn a_saved_http_url_to_a_public_host_is_refused_with_a_next_step() {
    let dir = tempfile::tempdir().unwrap();
    write_saved(dir.path(), "http://me:secret@hub.example.com/v1/client");
    let client =
        Client::start_with(Some(Store::new(dir.path())), ClientOptions::desktop()).unwrap();
    let reason = refused_reason(&client);
    assert!(reason.contains("hub.example.com"), "{reason}");
    assert!(reason.contains("https"), "{reason}");
    assert!(reason.contains("Pair this app again"), "{reason}");
    assert!(
        !reason.contains("secret"),
        "no saved secret in the text: {reason}"
    );
}

/// A saved URL with a path and a password works, but only as its origin.
#[test]
fn a_saved_url_with_a_path_and_a_password_is_cut_to_its_origin() {
    let dir = tempfile::tempdir().unwrap();
    write_saved(dir.path(), "http://me:secret@127.0.0.1:1/v1/client?x=1");
    let client =
        Client::start_with(Some(Store::new(dir.path())), ClientOptions::desktop()).unwrap();
    assert!(
        !matches!(client.state().connection, Connection::Refused { .. }),
        "{:?}",
        client.state().connection
    );
}
