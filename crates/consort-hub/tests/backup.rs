//! Settings > Hub: `hub.status`, Back up now, the backup schedule, the
//! prune to the keep count, and `consort hub restore` (phase 3).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::common::*;
use consort_hub::admin::{self, RestoreError};
use consort_proto::auth::{AuthHttpToken, blob_path};
use consort_proto::client_api::{
    BackupFrequency, BackupSchedule, ClientList, ClientPairingCode, HubBackup, HubBackupSettings,
    HubStatus, MessageList, MessageSend, Weekday,
};
use consort_proto::model::{PairedBy, Sha256Hash};
use consort_proto::time::Timestamp;
use sha2::{Digest, Sha256};

fn hash_of(bytes: &[u8]) -> Sha256Hash {
    Sha256Hash::from_bytes(Sha256::digest(bytes).into())
}

/// Uploads a blob as the client and returns its hash.
async fn upload(hub: &TestHub, client: &mut FakeClient, bytes: &[u8]) -> Sha256Hash {
    let token = client.request(AuthHttpToken {}).await.unwrap().token;
    let hash = hash_of(bytes);
    let response = reqwest::Client::new()
        .put(format!("{}{}", hub.url, blob_path(&hash)))
        .bearer_auth(&token)
        .body(bytes.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    hash
}

/// Waits until no backup runs and returns the status.
async fn wait_for_backup(
    hub: &TestHub,
    client: &mut FakeClient,
) -> consort_proto::client_api::HubInfo {
    let deadline = tokio::time::Instant::now() + WAIT;
    while hub.handle.backup_running() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the backup did not finish"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    client.request(HubStatus {}).await.unwrap().status
}

fn archives(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("consort-") && n.ends_with(".tar"))
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn status_shows_the_address_versions_folder_and_storage() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    upload(&hub, &mut client, b"a channel file").await;
    let status = client.request(HubStatus {}).await.unwrap().status;
    assert_eq!(status.hub_url, hub.url);
    assert_eq!(status.hub_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(status.protocol_versions, vec![1]);
    assert_eq!(status.data_dir, hub.dir.path().display().to_string());
    assert!(status.storage.database_bytes > 0);
    assert_eq!(status.storage.blobs_bytes, b"a channel file".len() as u64);
    assert_eq!(status.storage.skills_bytes, 0);
    #[cfg(unix)]
    assert!(status.storage.free_bytes.is_some());
    // A new hub backs up to `backups` in its data folder, keeps 7, and has
    // no schedule.
    let settings = &status.backup.settings;
    assert_eq!(settings.schedule.frequency, BackupFrequency::Off);
    assert_eq!(settings.keep, 7);
    assert_eq!(
        settings.folder,
        hub.dir.path().join("backups").display().to_string()
    );
    assert!(status.backup.last.is_none());
    assert!(status.backup.running.is_none());
}

#[tokio::test]
async fn the_client_list_says_who_paired_each_client_and_which_are_online() {
    let hub = TestHub::start().await;
    let mut first = FakeClient::pair_and_connect(&hub).await;
    let code = first.request(ClientPairingCode {}).await.unwrap();
    let second = FakeClient::pair_with_code_and_connect(&hub, &code.code.to_string()).await;
    let list = first.request(ClientList {}).await.unwrap();
    assert_eq!(list.clients.len(), 2);
    assert_eq!(list.clients[0].paired_by, Some(PairedBy::HubMachine));
    assert_eq!(
        list.clients[1].paired_by,
        Some(PairedBy::Client {
            client_id: first.client_id
        })
    );
    assert_eq!(list.clients[1].os, Some(consort_proto::model::Os::Macos));
    assert!(list.online.contains(&first.client_id));
    assert!(list.online.contains(&second.client_id));
    // A client that leaves is no longer online, and it has a last seen
    // time.
    second.peer.close().await;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let list = first.request(ClientList {}).await.unwrap();
        let gone = !list.online.contains(&list.clients[1].id);
        if gone && list.clients[1].last_seen_at.is_some() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn back_up_now_writes_an_archive_and_records_it() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    upload(&hub, &mut client, b"photo bytes").await;
    let folder = hub.dir.path().join("my-backups");
    let changed = client
        .request(HubBackupSettings {
            folder: Some(folder.display().to_string()),
            ..HubBackupSettings::default()
        })
        .await
        .unwrap();
    assert_eq!(changed.settings.folder, folder.display().to_string());
    let started = client.request(HubBackup {}).await.unwrap();
    assert!(started.progress.bytes_total > 0);
    let status = wait_for_backup(&hub, &mut client).await;
    let last = status.backup.last.expect("a last backup");
    assert!(last.error.is_none(), "{:?}", last.error);
    assert!(last.file_name.starts_with("consort-"));
    assert_eq!(archives(&folder), vec![last.file_name.clone()]);
    assert_eq!(
        std::fs::metadata(folder.join(&last.file_name))
            .unwrap()
            .len(),
        last.size_bytes
    );
    // The archive starts with the manifest and holds the database and the
    // blob.
    let mut tar = tar::Archive::new(std::fs::File::open(folder.join(&last.file_name)).unwrap());
    let names: Vec<String> = tar
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names[0], "consort-backup.json");
    assert!(names.contains(&"hub.db".to_owned()));
    assert!(names.contains(&format!("blobs/{}", hash_of(b"photo bytes"))));
    // No temp files stay in the folder.
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1);
}

#[tokio::test]
async fn bad_settings_are_refused() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let relative = client
        .request(HubBackupSettings {
            folder: Some("backups".into()),
            ..HubBackupSettings::default()
        })
        .await
        .unwrap_err();
    assert_eq!(relative.code, consort_proto::error::ErrorCode::Invalid);
    let zone = client
        .request(HubBackupSettings {
            schedule: Some(BackupSchedule {
                frequency: BackupFrequency::Daily,
                time: "03:00".into(),
                weekday: Weekday::Sunday,
                time_zone: "Mars/Olympus".into(),
            }),
            ..HubBackupSettings::default()
        })
        .await
        .unwrap_err();
    assert!(zone.message.contains("time zone"), "{}", zone.message);
    let keep = client
        .request(HubBackupSettings {
            keep: Some(0),
            ..HubBackupSettings::default()
        })
        .await
        .unwrap_err();
    assert_eq!(keep.code, consort_proto::error::ErrorCode::Invalid);
}

#[tokio::test]
async fn the_schedule_runs_once_per_slot_and_old_archives_go() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let folder = hub.dir.path().join("scheduled");
    client
        .request(HubBackupSettings {
            schedule: Some(BackupSchedule {
                frequency: BackupFrequency::Daily,
                time: "03:00".into(),
                weekday: Weekday::Sunday,
                time_zone: "UTC".into(),
            }),
            folder: Some(folder.display().to_string()),
            keep: Some(2),
        })
        .await
        .unwrap();
    // The settings survive a restart: they live in backup.json.
    let hub = hub.restart().await;
    let mut client = FakeClient::connect(&hub, client.client_id, client.key).await;
    let read = client
        .request(HubBackupSettings::default())
        .await
        .unwrap()
        .settings;
    assert_eq!(read.keep, 2);
    assert_eq!(read.schedule.frequency, BackupFrequency::Daily);

    // Three days of slots, each run a little after 3:00 UTC. The clock of
    // the slots is in the future, so the schedule change is before them.
    let day_ms = 86_400_000_i64;
    let first_slot = (Timestamp::now().as_millis() / day_ms + 2) * day_ms + 3 * 3_600_000;
    for day in 0..3 {
        let at = Timestamp::from_millis(first_slot + day * day_ms + 30_000);
        assert!(hub.handle.backup_if_due_at(at).unwrap(), "day {day}");
        wait_for_backup(&hub, &mut client).await;
        // The same slot does not run twice.
        assert!(!hub.handle.backup_if_due_at(at.plus_millis(60_000)).unwrap());
    }
    let status = client.request(HubStatus {}).await.unwrap().status;
    let last = status.backup.last.unwrap();
    assert_eq!(
        last.trigger,
        consort_proto::client_api::BackupTrigger::Scheduled
    );
    // Three backups ran, and the hub kept the newest two.
    assert_eq!(archives(&folder).len(), 2);
    // A slot that is more than an hour late does not run.
    let late = Timestamp::from_millis(first_slot + 3 * day_ms + 2 * 3_600_000);
    assert!(!hub.handle.backup_if_due_at(late).unwrap());
}

/// A hub with a message and a blob that was backed up and stopped.
struct BackedUp {
    dir: tempfile::TempDir,
    archive: PathBuf,
    hash: Sha256Hash,
    client_id: consort_proto::ids::ClientId,
    key: ed25519_dalek::SigningKey,
}

async fn hub_with_backup() -> BackedUp {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let tutti = client.tutti().await;
    client
        .request(MessageSend {
            channel_id: tutti,
            client_message_id: "cm-restore".to_owned(),
            body: "Keep this message".to_owned(),
            attachments: Vec::new(),
            thread_root_id: None,
            in_timeline: false,
        })
        .await
        .unwrap();
    let hash = upload(&hub, &mut client, b"restored blob").await;
    client.request(HubBackup {}).await.unwrap();
    let status = wait_for_backup(&hub, &mut client).await;
    let archive = PathBuf::from(status.backup.last.unwrap().path);
    let FakeClient { client_id, key, .. } = client;
    let TestHub { handle, dir, .. } = hub;
    handle.shutdown().await.unwrap();
    BackedUp {
        dir,
        archive,
        hash,
        client_id,
        key,
    }
}

#[tokio::test]
async fn a_restore_round_trip_brings_back_messages_and_blobs() {
    let backed = hub_with_backup().await;
    let target = tempfile::tempdir().unwrap();
    let report = admin::restore(&backed.archive, target.path(), false).unwrap();
    assert_eq!(report.blobs, 1);
    assert!(report.moved_aside.is_none());
    assert_eq!(
        std::fs::read(target.path().join("blobs").join(backed.hash.as_str())).unwrap(),
        b"restored blob"
    );

    // A hub on the restored folder knows the old client and its message.
    let hub = TestHub::start_in(target).await;
    assert!(
        hub.handle.setup_code().is_none(),
        "the restored hub has a client"
    );
    let mut client = FakeClient::connect(&hub, backed.client_id, backed.key).await;
    let tutti = client.tutti().await;
    let page = client
        .request(MessageList {
            channel_id: tutti,
            thread_root_id: None,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    assert!(page.messages.iter().any(|m| m.body == "Keep this message"));
    let token = client.request(AuthHttpToken {}).await.unwrap().token;
    let response = reqwest::Client::new()
        .get(format!("{}{}", hub.url, blob_path(&backed.hash)))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.bytes().await.unwrap().as_ref(), b"restored blob");
    drop(backed.dir);
}

#[tokio::test]
async fn a_restore_refuses_a_running_hub_a_wrong_file_and_a_full_folder() {
    let backed = hub_with_backup().await;
    let (old_dir, archive) = (&backed.dir, &backed.archive);

    // A running hub on the folder.
    let running = TestHub::start().await;
    let err = admin::restore(archive, running.dir.path(), true).unwrap_err();
    assert!(matches!(err, RestoreError::HubRunning(_)), "{err}");
    assert!(err.to_string().contains("Stop the hub"));

    // A file that is not a backup.
    let other = old_dir.path().join("notes.tar");
    std::fs::write(&other, b"not a tar at all").unwrap();
    let empty = tempfile::tempdir().unwrap();
    let err = admin::restore(&other, empty.path(), false).unwrap_err();
    assert!(matches!(err, RestoreError::NotABackup { .. }), "{err}");
    let err = admin::restore(&old_dir.path().join("missing.tar"), empty.path(), false).unwrap_err();
    assert!(matches!(err, RestoreError::NotABackup { .. }), "{err}");
    // A tar without the manifest.
    let plain = old_dir.path().join("plain.tar");
    {
        let mut builder = tar::Builder::new(std::fs::File::create(&plain).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_size(2);
        header.set_cksum();
        builder
            .append_data(&mut header, "hub.db", &b"xx"[..])
            .unwrap();
        builder.finish().unwrap();
    }
    let err = admin::restore(&plain, empty.path(), false).unwrap_err();
    assert!(err.to_string().contains("no Consort manifest"), "{err}");

    // A folder with a hub in it needs --force, and --force keeps the old
    // files aside.
    let full = old_dir.path();
    let err = admin::restore(archive, full, false).unwrap_err();
    assert!(matches!(err, RestoreError::NotEmpty(_)), "{err}");
    assert!(err.to_string().contains("--force"));
    let report = admin::restore(archive, full, true).unwrap();
    let aside = report.moved_aside.expect("the old files moved aside");
    assert!(aside.join("hub.db").exists());
    assert!(full.join("hub.db").exists());
    // The backups folder stays where it was.
    assert!(archive.exists());
    running.handle.shutdown().await.unwrap();
}
