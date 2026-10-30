//! Review 3, S3: a stolen client key could remove every other client, the
//! hub had no list or revoke of its own, and the codes did not say which
//! client made them. So the user could not get back in. The hub machine
//! can now list and remove clients and workers, and a removed client's
//! codes stop working.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use consort_hub::admin::{self, Origin};
use consort_proto::client_api::{
    ClientPairingCode, ClientRevoke, EntityPayload, WorkerPairingCode,
};
use consort_proto::model::{ItemStatus, SessionState};
use consort_proto::status::TaskStatus;

#[tokio::test]
async fn the_hub_machine_removes_a_thief_and_its_codes_while_the_hub_runs() {
    let hub = TestHub::start().await;
    let mut user = FakeClient::pair_and_connect(&hub).await;
    let snapshot = user
        .snapshot(consort_proto::client_api::SnapshotScope::All)
        .await;
    user.subscribe(snapshot.seq).await.unwrap();
    // The thief paired with a code that the user's client made.
    let code = user.request(ClientPairingCode {}).await.unwrap();
    let mut thief = FakeClient::pair_with_code_and_connect(&hub, &code.code.to_string()).await;
    // The thief makes codes for later.
    let thief_client_code = thief.request(ClientPairingCode {}).await.unwrap();
    let thief_worker_code = thief.request(WorkerPairingCode {}).await.unwrap();

    // The list says who paired whom.
    let records = admin::list_clients(hub.dir.path()).unwrap();
    let paired_by = |id| {
        records
            .iter()
            .find(|r| r.client.id == id)
            .unwrap()
            .paired_by
    };
    assert_eq!(paired_by(user.client_id), Some(Origin::HubMachine));
    assert_eq!(
        paired_by(thief.client_id),
        Some(Origin::Client(user.client_id))
    );

    // The user removes the thief on the hub machine.
    let removed = admin::revoke_client(hub.dir.path(), thief.client_id).unwrap();
    assert!(!removed.already);
    assert!(thief.peer.closed(WAIT).await, "the thief stayed connected");
    assert!(
        admin::revoke_client(hub.dir.path(), thief.client_id)
            .unwrap()
            .already
    );

    // The codes that the thief made do not work.
    let (status, body) =
        pair_client_http(&hub, &thief_client_code.code.to_string(), &new_key()).await;
    assert_eq!(status, 401, "{body}");
    let (status, body) =
        pair_worker_http(&hub, &thief_worker_code.code.to_string(), &new_key()).await;
    assert_eq!(status, 401, "{body}");

    // The user's client sees the change live, and stays connected.
    let thief_id = thief.client_id;
    user.wait_feed(|e| {
        matches!(upserted(e), Some(EntityPayload::Client(c)) if c.id == thief_id && c.revoked_at.is_some())
    })
    .await;
    let list: consort_proto::client_api::ClientListResult = user
        .request(consort_proto::client_api::ClientList {})
        .await
        .unwrap();
    assert!(
        list.clients
            .iter()
            .any(|c| c.id == thief_id && c.revoked_at.is_some())
    );

    // The record says who removed it. A new setup code works.
    let records = admin::list_clients(hub.dir.path()).unwrap();
    let thief_record = records.iter().find(|r| r.client.id == thief_id).unwrap();
    assert_eq!(thief_record.revoked_by, Some(Origin::HubMachine));
    let setup = consort_hub::create_setup_code(hub.dir.path()).unwrap();
    FakeClient::pair_with_code_and_connect(&hub, &setup).await;
}

#[tokio::test]
async fn a_client_that_removes_another_is_recorded_and_the_codes_of_that_client_go() {
    let hub = TestHub::start().await;
    let mut user = FakeClient::pair_and_connect(&hub).await;
    let code = user.request(ClientPairingCode {}).await.unwrap();
    let mut phone = FakeClient::pair_with_code_and_connect(&hub, &code.code.to_string()).await;
    let phone_code = phone.request(ClientPairingCode {}).await.unwrap();

    user.request(ClientRevoke {
        client_id: phone.client_id,
    })
    .await
    .unwrap();
    assert!(phone.peer.closed(WAIT).await);
    let records = admin::list_clients(hub.dir.path()).unwrap();
    let phone_record = records
        .iter()
        .find(|r| r.client.id == phone.client_id)
        .unwrap();
    assert_eq!(
        phone_record.revoked_by,
        Some(Origin::Client(user.client_id))
    );
    let (status, body) = pair_client_http(&hub, &phone_code.code.to_string(), &new_key()).await;
    assert_eq!(status, 401, "{body}");
}

#[tokio::test]
async fn the_hub_machine_removes_a_worker_and_its_running_task_fails() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Fix it.").await;
    world
        .worker
        .send_events(
            start.session_id,
            vec![state(SessionState::Active), turn_started("t1")],
        )
        .await;
    let worker_id = world.worker.worker_id;
    let records = admin::list_workers(world.hub.dir.path()).unwrap();
    assert_eq!(
        records
            .iter()
            .find(|r| r.worker.id == worker_id)
            .unwrap()
            .paired_by,
        Some(Origin::HubMachine)
    );

    let removed = admin::revoke_worker(world.hub.dir.path(), worker_id).unwrap();
    assert!(!removed.already);
    assert!(
        world.worker.peer.closed(WAIT).await,
        "the worker stayed connected"
    );
    let task = world.wait_task(task_id, TaskStatus::Failed).await;
    assert!(
        task.status_reason
            .as_deref()
            .unwrap_or_default()
            .contains("Retry"),
        "{:?}",
        task.status_reason
    );
    let records = admin::list_workers(world.hub.dir.path()).unwrap();
    let record = records.iter().find(|r| r.worker.id == worker_id).unwrap();
    assert!(record.worker.revoked_at.is_some());
    assert_eq!(record.revoked_by, Some(Origin::HubMachine));
}

#[tokio::test]
async fn a_worker_removed_while_the_hub_is_stopped_loses_its_sessions_at_the_next_start() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Fix it.").await;
    world
        .worker
        .send_events(
            start.session_id,
            vec![
                state(SessionState::Active),
                turn_started("t1"),
                assistant_text(start.session_id, "Working.", ItemStatus::InProgress),
            ],
        )
        .await;
    let World { hub, worker, .. } = world;
    let worker_id = worker.worker_id;
    worker.peer.close().await;
    let TestHub { handle, dir, .. } = hub;
    handle.shutdown().await.unwrap();
    // The hub is stopped. The user removes the worker on the hub machine.
    admin::revoke_worker(dir.path(), worker_id).unwrap();
    let hub = TestHub::start_in(dir).await;
    let setup = consort_hub::create_setup_code(hub.dir.path()).unwrap();
    let mut client = FakeClient::pair_with_code_and_connect(&hub, &setup).await;
    let tutti = client.tutti().await;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let snapshot = client
            .snapshot(consort_proto::client_api::SnapshotScope::Channel(tutti))
            .await;
        let task = snapshot
            .tasks
            .into_iter()
            .find(|t| t.id == task_id)
            .unwrap();
        if task.status == TaskStatus::Failed {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the task stayed {:?}",
            task.status
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
