//! How other devices reach the hub: a real TLS listener with a pin, and the
//! list of origins that a peer may sign. A fake stands in for the `tailscale`
//! program, so these tests never touch a tailnet.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use consort_client::blob::BlobTransfer;
use consort_client::hub::{ClientOptions, pair};
use consort_client::store::Store;
use consort_client::{Client, ClientError, Connection, ViewState};
use consort_hub::reach::{ProgramError, ProgramOutput, TailscaleProgram, cli};
use consort_hub::{HubConfig, serve_with_tailscale};
use consort_link::http::{HubRef, download_blob, pair_machine, upload_blob};
use consort_link::identity::Identity;
use consort_link::link::{Link, LinkHandler, maintain};
use consort_proto::auth::{
    AuthHttpToken, HEADER_SIGNATURE, HEADER_TIMESTAMP, HEADER_WORKER_ID, PAIR_WORKER_PATH,
    PairWorkerRequest, PairingInvite, WORKER_WS_PATH, WorkerHello, WorkerHelloResult, blob_path,
    http_signing_payload,
};
use consort_proto::client_api::{
    ClientPairingCode, HubBackup, HubBackupSettings, HubReachSet, HubReachStatus, HubStatus,
    NetworkState, ReachMethod, TailscaleState, WorkerPairingCode,
};
use consort_proto::frame::SUPPORTED_PROTOCOL_VERSIONS;
use consort_proto::ids::WorkerId;
use consort_proto::model::{Os, Sha256Hash};
use consort_proto::release::InstallKind;
use consort_proto::time::Timestamp;
use ed25519_dalek::Signer as _;
use sha2::{Digest, Sha256};

use crate::common::*;

const TAILNET_NAME: &str = "mac.tail1234.ts.net";
const TAILNET_ORIGIN: &str = "https://mac.tail1234.ts.net";

/// The `tailscale` program, with its Serve entries in memory.
#[derive(Default)]
struct FakeTailscale {
    entries: Mutex<Vec<(u16, String)>>,
}

fn answer(stdout: String) -> Result<ProgramOutput, ProgramError> {
    Ok(ProgramOutput {
        success: true,
        stdout,
        ..ProgramOutput::default()
    })
}

#[async_trait]
impl TailscaleProgram for FakeTailscale {
    async fn run(&self, args: &[String]) -> Result<ProgramOutput, ProgramError> {
        let line = args.join(" ");
        if line == "status --json" {
            return answer(format!(
                r#"{{"BackendState":"Running","Self":{{"DNSName":"{TAILNET_NAME}.","Capabilities":["https"]}},"CurrentTailnet":{{"MagicDNSEnabled":true}}}}"#
            ));
        }
        if line == "serve status --json" {
            let entries = self.entries.lock().unwrap();
            let tcp: Vec<String> = entries
                .iter()
                .map(|(port, _)| format!(r#""{port}":{{"HTTPS":true}}"#))
                .collect();
            let web: Vec<String> = entries
                .iter()
                .map(|(port, proxy)| {
                    format!(
                        r#""{TAILNET_NAME}:{port}":{{"Handlers":{{"/":{{"Proxy":"{proxy}"}}}}}}"#
                    )
                })
                .collect();
            return answer(format!(
                r#"{{"TCP":{{{}}},"Web":{{{}}}}}"#,
                tcp.join(","),
                web.join(",")
            ));
        }
        if args[0] == "serve" && args[1] == "--bg" {
            let port: u16 = args[3].trim_start_matches("--https=").parse().unwrap();
            self.entries.lock().unwrap().push((port, args[4].clone()));
            return answer(String::new());
        }
        if args[0] == "serve" && args.last().map(String::as_str) == Some("off") {
            let port: u16 = args[2].trim_start_matches("--https=").parse().unwrap();
            self.entries.lock().unwrap().retain(|(p, _)| *p != port);
            return answer(String::new());
        }
        panic!("a command that Consort must not run: {line}");
    }
}

/// A hub that may change its reach, on this folder, with a setup code for the
/// test. A real local hub makes its codes only when `consort local` asks.
async fn local_hub_in(dir: tempfile::TempDir, fake: Arc<FakeTailscale>) -> TestHub {
    let hub = local_hub_with_no_code(dir, fake).await;
    hub.handle.new_setup_code().await.unwrap();
    hub
}

async fn local_hub_with_no_code(dir: tempfile::TempDir, fake: Arc<FakeTailscale>) -> TestHub {
    let handle = serve_with_tailscale(
        HubConfig {
            data_dir: dir.path().to_path_buf(),
            listen: "127.0.0.1:0".parse().unwrap(),
            public_url: None,
            install_kind: InstallKind::Local,
            dev: false,
            first_greeting: false,
        },
        fake,
    )
    .await
    .unwrap();
    // The certificate does not check the name, so a test can use this address.
    handle.set_network_host("127.0.0.1");
    TestHub {
        url: handle.public_url(),
        addr: handle.local_addr(),
        handle,
        dir,
    }
}

async fn local_hub(fake: Arc<FakeTailscale>) -> TestHub {
    local_hub_in(tempfile::tempdir().unwrap(), fake).await
}

#[tokio::test]
async fn a_local_hub_makes_no_setup_code_at_its_start() {
    let hub = local_hub_with_no_code(
        tempfile::tempdir().unwrap(),
        Arc::new(FakeTailscale::default()),
    )
    .await;
    assert_eq!(hub.handle.setup_code(), None);
    let code = hub.handle.new_setup_code().await.unwrap();
    assert_eq!(hub.handle.setup_code(), Some(code));
}

/// Turns on This network, and returns its address and pin.
async fn network_on(hub: &TestHub) -> (String, Sha256Hash) {
    let info = hub
        .handle
        .set_reach(ReachMethod::ThisNetwork, true)
        .await
        .unwrap();
    let NetworkState::On {
        address,
        tls_sha256,
    } = info.this_network
    else {
        panic!("{:?}", info.this_network)
    };
    (address, tls_sha256)
}

fn invite(address: &str, code: &str, pin: Option<Sha256Hash>) -> String {
    PairingInvite {
        hub_url: address.to_owned(),
        code: code.parse().unwrap(),
        tls_sha256: pin,
    }
    .to_link()
}

fn test_options() -> ClientOptions {
    ClientOptions::desktop()
}

async fn wait_state(client: &Client, what: &str, pred: impl Fn(&ViewState) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if pred(&client.state()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "timed out waiting for {what}; last state: {:?}",
        client.state().connection
    );
}

// The pin of the client ------------------------------------------------------

#[tokio::test]
async fn a_client_accepts_only_the_pinned_certificate() {
    let hub = local_hub(Arc::default()).await;
    let code = hub.handle.setup_code().unwrap();
    let (address, pin) = network_on(&hub).await;

    // The address alone: a clear message, and the code stays good.
    let err = pair(&address, &code, &test_options()).await.unwrap_err();
    assert!(matches!(err, ClientError::UntrustedHub { .. }), "{err}");
    assert!(err.to_string().contains("Scan the code"), "{err}");

    // Another pin.
    let wrong = Sha256Hash::from_bytes([9; 32]);
    let err = pair(
        &invite(&address, &code, Some(wrong.clone())),
        "",
        &test_options(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ClientError::PinMismatch { .. }), "{err}");

    // The right pin, from a pasted link.
    let identity = pair(
        &invite(&address, &code, Some(pin.clone())),
        "",
        &test_options(),
    )
    .await
    .unwrap();
    assert_eq!(identity.config.hub_url, address);
    assert_eq!(identity.config.tls_sha256, Some(pin.clone()));

    // The saved identity connects over the WebSocket with its pin.
    let good = tempfile::tempdir().unwrap();
    Store::new(good.path()).save(&identity).unwrap();
    let client = Client::start_with(Some(Store::new(good.path())), test_options()).unwrap();
    wait_state(&client, "the link", |s| s.connection == Connection::Online).await;

    // The same identity with another pin does not.
    let bad = tempfile::tempdir().unwrap();
    let mut other = identity.clone();
    other.config.tls_sha256 = Some(wrong);
    Store::new(bad.path()).save(&other).unwrap();
    let refused = Client::start_with(Some(Store::new(bad.path())), test_options()).unwrap();
    wait_state(&refused, "the certificate error", |s| {
        matches!(&s.connection, Connection::Offline { reason, .. }
            if reason.contains("another certificate"))
    })
    .await;
    assert_ne!(refused.state().connection, Connection::Online);

    drop(client);
    drop(refused);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_unpinned_link_to_a_self_signed_hub_fails_with_no_panic() {
    let hub = local_hub(Arc::default()).await;
    let code = hub.handle.setup_code().unwrap();
    let (address, pin) = network_on(&hub).await;
    let mut identity = pair(&invite(&address, &code, Some(pin)), "", &test_options())
        .await
        .unwrap();
    identity.config.tls_sha256 = None;
    let dir = tempfile::tempdir().unwrap();
    Store::new(dir.path()).save(&identity).unwrap();
    let client = Client::start_with(Some(Store::new(dir.path())), test_options()).unwrap();
    wait_state(&client, "the error", |s| {
        matches!(&s.connection, Connection::Offline { reason, .. }
            if reason.contains("no authority"))
    })
    .await;
    drop(client);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_blob_client_accepts_only_the_pinned_certificate() {
    let hub = local_hub(Arc::default()).await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let (address, pin) = network_on(&hub).await;
    let token = client.request(AuthHttpToken {}).await.unwrap().token;
    let bytes = b"over the pin".to_vec();
    let sha = BlobTransfer::hash_bytes(&bytes);

    let good = BlobTransfer::pinned(&address, Some(&pin), None).unwrap();
    good.upload_bytes(&token, bytes.clone(), &sha, |_| {})
        .await
        .unwrap();
    assert!(good.exists(&token, &sha).await.unwrap());

    let wrong = Sha256Hash::from_bytes([9; 32]);
    let bad = BlobTransfer::pinned(&address, Some(&wrong), None).unwrap();
    assert!(bad.exists(&token, &sha).await.is_err());
    let none = BlobTransfer::new(&address, None).unwrap();
    assert!(none.exists(&token, &sha).await.is_err());
    hub.handle.shutdown().await.unwrap();
}

// The pin of the link of a machine -------------------------------------------

struct Machine {
    url: String,
    pin: Option<Sha256Hash>,
    identity: Identity,
    worker_id: WorkerId,
    connected: tokio::sync::mpsc::UnboundedSender<()>,
}

impl LinkHandler for Machine {
    fn worker_id(&self) -> WorkerId {
        self.worker_id
    }
    fn ws_path(&self) -> &str {
        WORKER_WS_PATH
    }
    fn hub_url(&self) -> &str {
        &self.url
    }
    fn tls_sha256(&self) -> Option<&Sha256Hash> {
        self.pin.as_ref()
    }
    fn sign(&self, payload: &[u8]) -> consort_proto::auth::Signature {
        self.identity.sign(payload)
    }
    fn hello(&self) -> WorkerHello {
        WorkerHello {
            worker_id: self.worker_id,
            version: "test".into(),
            protocol_versions: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
            sessions: vec![],
        }
    }
    fn connected(self: Arc<Self>, _result: WorkerHelloResult) {
        let _ = self.connected.send(());
    }
    fn command(self: Arc<Self>, _request: consort_proto::frame::RequestFrame) {}
}

/// True when the link reaches the hub in a short time.
async fn link_connects(
    machine: Machine,
    mut connected: tokio::sync::mpsc::UnboundedReceiver<()>,
) -> bool {
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let run = tokio::spawn(maintain(Link::new(), Arc::new(machine), stopped));
    let reached = tokio::time::timeout(Duration::from_secs(3), connected.recv())
        .await
        .is_ok();
    let _ = stop.send(true);
    run.abort();
    reached
}

#[tokio::test]
async fn the_link_of_a_machine_accepts_only_the_pinned_certificate() {
    let hub = local_hub(Arc::default()).await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let (address, pin) = network_on(&hub).await;

    // The code says where a worker goes, with the pin for the install command.
    let result = client.request(WorkerPairingCode {}).await.unwrap();
    assert_eq!(result.hub_url.as_deref(), Some(address.as_str()));
    assert_eq!(result.tls_sha256.as_ref(), Some(&pin));

    let identity = Identity::generate();
    let request = PairWorkerRequest {
        code: result.code,
        public_key: identity.public_key(),
        hostname: "test".into(),
        os: Os::Linux,
        arch: "arm64".into(),
        name: None,
        protocol_versions: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
    };
    let wrong = Sha256Hash::from_bytes([9; 32]);
    let refused = pair_machine(
        HubRef {
            url: &address,
            tls_sha256: Some(&wrong),
        },
        PAIR_WORKER_PATH,
        &request,
    )
    .await
    .unwrap_err();
    assert!(refused.to_string().contains("not the one"), "{refused}");
    let unpinned = pair_machine(address.as_str(), PAIR_WORKER_PATH, &request)
        .await
        .unwrap_err();
    assert!(unpinned.to_string().contains("no authority"), "{unpinned}");

    let good = HubRef {
        url: &address,
        tls_sha256: Some(&pin),
    };
    let worker_id = pair_machine(good, PAIR_WORKER_PATH, &request)
        .await
        .unwrap();

    // Blobs.
    let bytes = b"from the machine".to_vec();
    let sha = Sha256Hash::from_bytes(Sha256::digest(&bytes).into());
    upload_blob(good, worker_id, &identity, &sha, bytes.clone())
        .await
        .unwrap();
    assert_eq!(
        download_blob(good, worker_id, &identity, &sha, 1024)
            .await
            .unwrap(),
        bytes
    );
    let bad = HubRef {
        url: &address,
        tls_sha256: Some(&wrong),
    };
    assert!(
        download_blob(bad, worker_id, &identity, &sha, 1024)
            .await
            .is_err()
    );

    // The WebSocket.
    let saved = hub.dir.path().join("machine.key");
    identity.save(&saved).unwrap();
    let machine = |pin: Option<Sha256Hash>| {
        let (connected, rx) = tokio::sync::mpsc::unbounded_channel();
        (
            Machine {
                url: address.clone(),
                pin,
                identity: Identity::load(&saved).unwrap(),
                worker_id,
                connected,
            },
            rx,
        )
    };
    let (with_wrong, rx) = machine(Some(wrong));
    assert!(
        !link_connects(with_wrong, rx).await,
        "the wrong pin connected"
    );
    let (with_right, rx) = machine(Some(pin));
    assert!(
        link_connects(with_right, rx).await,
        "the right pin did not connect"
    );
    hub.handle.shutdown().await.unwrap();
}

// The list of origins --------------------------------------------------------

/// True when the hub takes this origin in the WebSocket signature.
async fn socket_accepts(hub: &TestHub, client: &FakeClient, origin: &str) -> bool {
    let mut peer = Peer::connect(&hub.ws_url(consort_proto::auth::CLIENT_WS_PATH)).await;
    authenticate(
        &mut peer,
        origin,
        consort_proto::auth::PeerId::Client(client.client_id),
        &client.key,
    )
    .await
    .is_ok()
}

/// True when the hub takes this origin in the signature of a blob request.
async fn blob_accepts(hub: &TestHub, worker: &FakeWorker, origin: &str) -> bool {
    let sha = Sha256Hash::from_bytes([3; 32]);
    let path = blob_path(&sha);
    let now = Timestamp::now();
    let payload = http_signing_payload("GET", origin, &path, now, worker.worker_id);
    let signature =
        consort_proto::auth::Signature::from_bytes(worker.key.sign(&payload).to_bytes());
    let status = reqwest::Client::new()
        .get(format!("{}{path}", hub.url))
        .header(HEADER_WORKER_ID, worker.worker_id.to_string())
        .header(HEADER_TIMESTAMP, now.as_millis().to_string())
        .header(HEADER_SIGNATURE, signature.to_hex())
        .send()
        .await
        .unwrap()
        .status();
    // A good signature for a blob that is not there gets 404.
    status != reqwest::StatusCode::UNAUTHORIZED
}

#[tokio::test]
async fn a_signature_counts_only_for_an_origin_on_the_list() {
    let hub = local_hub(Arc::default()).await;
    let client = FakeClient::pair_and_connect(&hub).await;
    let worker = FakeWorker::pair_and_connect(&hub).await;
    let main = hub.origin();
    let off_the_list = "https://other.example.com";

    assert!(socket_accepts(&hub, &client, &main).await);
    assert!(blob_accepts(&hub, &worker, &main).await);
    for origin in [TAILNET_ORIGIN, off_the_list] {
        assert!(!socket_accepts(&hub, &client, origin).await, "{origin}");
        assert!(!blob_accepts(&hub, &worker, origin).await, "{origin}");
    }

    // A reach adds its address. The main address stays, so an existing client
    // keeps its hub URL.
    let info = hub
        .handle
        .set_reach(ReachMethod::Tailscale, true)
        .await
        .unwrap();
    assert!(
        matches!(info.tailscale, TailscaleState::On { .. }),
        "{info:?}"
    );
    assert!(socket_accepts(&hub, &client, TAILNET_ORIGIN).await);
    assert!(blob_accepts(&hub, &worker, TAILNET_ORIGIN).await);
    assert!(socket_accepts(&hub, &client, &main).await);
    assert!(!socket_accepts(&hub, &client, off_the_list).await);
    assert!(!blob_accepts(&hub, &worker, off_the_list).await);

    // Turning it off removes the address.
    hub.handle
        .set_reach(ReachMethod::Tailscale, false)
        .await
        .unwrap();
    assert!(!socket_accepts(&hub, &client, TAILNET_ORIGIN).await);
    assert!(!blob_accepts(&hub, &worker, TAILNET_ORIGIN).await);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_change_on_the_command_line_counts_with_no_restart() {
    let fake = Arc::new(FakeTailscale::default());
    let hub = local_hub(fake.clone()).await;
    let client = FakeClient::pair_and_connect(&hub).await;
    assert!(!socket_accepts(&hub, &client, TAILNET_ORIGIN).await);

    let data = hub.dir.path().to_path_buf();
    cli::tailscale_with(&data, fake.as_ref()).await.unwrap();
    assert!(socket_accepts(&hub, &client, TAILNET_ORIGIN).await);
    let status = cli::status_with(&data, fake.as_ref())
        .await
        .unwrap()
        .join("\n");
    assert!(status.contains(TAILNET_NAME), "{status}");

    cli::off_with(&data, None, fake.as_ref()).await.unwrap();
    assert!(!socket_accepts(&hub, &client, TAILNET_ORIGIN).await);
    assert!(fake.entries.lock().unwrap().is_empty());
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_restart_finds_a_removed_entry_and_keeps_the_origin_until_the_user_decides() {
    let fake = Arc::new(FakeTailscale::default());
    let hub = local_hub(fake.clone()).await;
    let client = FakeClient::pair_and_connect(&hub).await;
    hub.handle
        .set_reach(ReachMethod::Tailscale, true)
        .await
        .unwrap();
    let (client_id, key) = (client.client_id, client.key.clone());
    drop(client);
    let dir = hub.dir;
    hub.handle.shutdown().await.unwrap();

    // Someone ran `tailscale serve reset` while the hub was down.
    fake.entries.lock().unwrap().clear();
    let hub = local_hub_in(dir, fake.clone()).await;
    let info = hub.handle.reach_status().await.unwrap();
    assert_eq!(
        info.tailscale,
        TailscaleState::Removed {
            address: TAILNET_ORIGIN.to_owned()
        }
    );
    let client = FakeClient::connect(&hub, client_id, key).await;
    assert!(socket_accepts(&hub, &client, TAILNET_ORIGIN).await);

    // The user decides.
    hub.handle
        .set_reach(ReachMethod::Tailscale, false)
        .await
        .unwrap();
    assert!(!socket_accepts(&hub, &client, TAILNET_ORIGIN).await);
    hub.handle.shutdown().await.unwrap();
}

// This network ---------------------------------------------------------------

#[tokio::test]
async fn the_network_port_survives_a_restart_and_keeps_its_certificate() {
    let fake = Arc::new(FakeTailscale::default());
    let hub = local_hub(fake.clone()).await;
    let code = hub.handle.setup_code().unwrap();
    let (address, pin) = network_on(&hub).await;
    let dir = hub.dir;
    hub.handle.shutdown().await.unwrap();

    let hub = local_hub_in(dir, fake).await;
    let info = hub.handle.reach_status().await.unwrap();
    assert_eq!(
        info.this_network,
        NetworkState::On {
            address: address.clone(),
            tls_sha256: pin.clone()
        }
    );
    pair(&invite(&address, &code, Some(pin)), "", &test_options())
        .await
        .unwrap();
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn turning_this_network_off_closes_its_links_and_its_port() {
    let hub = local_hub(Arc::default()).await;
    let code = hub.handle.setup_code().unwrap();
    let (address, pin) = network_on(&hub).await;
    let identity = pair(&invite(&address, &code, Some(pin)), "", &test_options())
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    Store::new(dir.path()).save(&identity).unwrap();
    let client = Client::start_with(Some(Store::new(dir.path())), test_options()).unwrap();
    wait_state(&client, "the link", |s| s.connection == Connection::Online).await;

    hub.handle
        .set_reach(ReachMethod::ThisNetwork, false)
        .await
        .unwrap();
    wait_state(&client, "the link to end", |s| {
        s.connection != Connection::Online
    })
    .await;
    let port: u16 = address.rsplit(':').next().unwrap().parse().unwrap();
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err(),
        "the port is still open"
    );
    drop(client);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_client_api_gives_the_state_and_the_pairing_address() {
    let hub = local_hub(Arc::default()).await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let start = client.request(HubReachStatus {}).await.unwrap().reach;
    assert!(start.can_change);
    assert_eq!(start.pairing.hub_url, hub.url);
    assert_eq!(start.this_network, NetworkState::Off);

    let on = client
        .request(HubReachSet {
            method: ReachMethod::ThisNetwork,
            on: true,
        })
        .await
        .unwrap()
        .reach;
    let NetworkState::On {
        address,
        tls_sha256,
    } = on.this_network
    else {
        panic!()
    };
    assert_eq!(on.pairing.hub_url, address);
    let pairing = client.request(ClientPairingCode {}).await.unwrap();
    assert_eq!(pairing.hub_url, address);
    assert_eq!(pairing.tls_sha256, Some(tls_sha256));
    // The address of the hub itself does not change.
    let status = client.request(HubStatus {}).await.unwrap().status;
    assert_eq!(status.hub_url, hub.url);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_server_hub_answers_with_state_only() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    // The hub has no way on, so this runs no change.
    let reach = client
        .request(HubReachSet {
            method: ReachMethod::ThisNetwork,
            on: true,
        })
        .await
        .unwrap()
        .reach;
    assert!(!reach.can_change);
    assert_eq!(reach.this_network, NetworkState::Off);
    hub.handle.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn the_private_key_is_private_and_a_backup_holds_it() {
    use std::os::unix::fs::PermissionsExt;

    let hub = local_hub(Arc::default()).await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let (_, pin) = network_on(&hub).await;
    let key_file = hub.dir.path().join("reach-tls.key");
    let mode = std::fs::metadata(&key_file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    let folder = tempfile::tempdir().unwrap();
    client
        .request(HubBackupSettings {
            folder: Some(folder.path().display().to_string()),
            ..HubBackupSettings::default()
        })
        .await
        .unwrap();
    client.request(HubBackup {}).await.unwrap();
    let deadline = tokio::time::Instant::now() + WAIT;
    while hub.handle.backup_running() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the backup did not end"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let status = client.request(HubStatus {}).await.unwrap().status;
    let archive = std::path::PathBuf::from(status.backup.last.unwrap().path);

    let restored = tempfile::tempdir().unwrap();
    let target = restored.path().join("hub");
    consort_hub::admin::restore(&archive, &target, false).unwrap();
    let key = std::fs::read(target.join("reach-tls.key")).unwrap();
    assert_eq!(key, std::fs::read(&key_file).unwrap());
    let mode = std::fs::metadata(target.join("reach-tls.key"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    let cert = std::fs::read(target.join("reach-tls.crt")).unwrap();
    assert_eq!(consort_tls::certificate_sha256(&cert), pin);
    hub.handle.shutdown().await.unwrap();
}

// The logs -------------------------------------------------------------------

static LOGS: Mutex<Vec<u8>> = Mutex::new(Vec::new());

struct Capture;

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        LOGS.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn no_code_or_key_of_reach_goes_in_a_log() {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(|| Capture)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);

    let hub = local_hub(Arc::default()).await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let (address, pin) = network_on(&hub).await;
    hub.handle
        .set_reach(ReachMethod::Tailscale, true)
        .await
        .unwrap();
    let client_code = client.request(ClientPairingCode {}).await.unwrap().code;
    let worker_code = client.request(WorkerPairingCode {}).await.unwrap().code;
    let identity = pair(
        &invite(&address, &client_code.to_string(), Some(pin)),
        "",
        &test_options(),
    )
    .await
    .unwrap();
    let key = std::fs::read(hub.dir.path().join("reach-tls.key")).unwrap();
    hub.handle
        .set_reach(ReachMethod::ThisNetwork, false)
        .await
        .unwrap();

    let logs = String::from_utf8_lossy(&LOGS.lock().unwrap()).into_owned();
    assert!(!logs.contains(&client_code.to_string()));
    assert!(!logs.contains(&worker_code.to_string()));
    let key_hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    assert!(!logs.contains(&key_hex));
    assert!(!logs.contains(&format!("{:?}", identity.key.to_bytes())));
    hub.handle.shutdown().await.unwrap();
}
