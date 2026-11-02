//! Review 3, S5: the hub data folder, `hub.db` with its `-wal` and `-shm`
//! files, and `blobs/` had the modes of the umask (755, 644, 755), so other
//! users of the machine could read every conversation.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

async fn start(data_dir: &Path) -> consort_hub::HubHandle {
    consort_hub::serve(consort_hub::HubConfig {
        data_dir: data_dir.to_path_buf(),
        listen: "127.0.0.1:0".parse().unwrap(),
        public_url: None,
        install_kind: consort_proto::release::InstallKind::Manual,
        dev: false,
        first_greeting: false,
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn a_new_data_folder_is_private() {
    let parent = tempfile::tempdir().unwrap();
    let data_dir = parent.path().join("consort").join("hub");
    let handle = start(&data_dir).await;
    assert_eq!(mode(&data_dir), 0o700);
    assert_eq!(mode(&data_dir.join("hub.db")), 0o600);
    assert_eq!(mode(&data_dir.join("blobs")), 0o700);
    for suffix in ["hub.db-wal", "hub.db-shm"] {
        let path = data_dir.join(suffix);
        if path.exists() {
            assert_eq!(mode(&path), 0o600, "{suffix}");
        }
    }
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_old_data_folder_becomes_private() {
    let parent = tempfile::tempdir().unwrap();
    let data_dir = parent.path().join("hub");
    let set = |path: &Path, mode: u32| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    // A running hub keeps `hub.db-wal` and `hub.db-shm`. Give everything
    // the modes of the usual umask, as a hub from before this fix did.
    let handle = start(&data_dir).await;
    let (db, wal, shm) = (
        data_dir.join("hub.db"),
        data_dir.join("hub.db-wal"),
        data_dir.join("hub.db-shm"),
    );
    assert!(wal.exists() && shm.exists());
    set(&data_dir, 0o755);
    for file in [&db, &wal, &shm] {
        set(file, 0o644);
    }
    // A command on the hub machine opens the database too.
    consort_hub::create_setup_code(&data_dir).unwrap();
    assert_eq!(mode(&data_dir), 0o700);
    for file in [&db, &wal, &shm] {
        assert_eq!(mode(file), 0o600, "{}", file.display());
    }
    handle.shutdown().await.unwrap();

    set(&data_dir, 0o755);
    set(&db, 0o644);
    set(&data_dir.join("blobs"), 0o755);
    let handle = start(&data_dir).await;
    assert_eq!(mode(&data_dir), 0o700);
    assert_eq!(mode(&db), 0o600);
    assert_eq!(mode(&data_dir.join("blobs")), 0o700);
    handle.shutdown().await.unwrap();
}

#[test]
fn a_setup_code_on_the_hub_machine_makes_the_folder_private_too() {
    let parent = tempfile::tempdir().unwrap();
    let data_dir = parent.path().join("hub");
    consort_hub::create_setup_code(&data_dir).unwrap();
    assert_eq!(mode(&data_dir), 0o700);
    assert_eq!(mode(&data_dir.join("hub.db")), 0o600);
}
