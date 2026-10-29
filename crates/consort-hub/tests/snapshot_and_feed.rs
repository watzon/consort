//! Snapshot, subscribe, the feed, resync and idempotent sends.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use consort_proto::client_api::{
    ClientPairingCode, EntityPayload, MessageList, MessageSend, PreferencesSetFavoriteModel,
    SnapshotScope, SyncSubscribe,
};
use consort_proto::error::ErrorCode;
use consort_proto::model::{Author, FavoriteModel};

#[tokio::test]
async fn a_snapshot_has_the_everything_channel_and_the_feed_position() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let snapshot = client.snapshot(SnapshotScope::All).await;
    let everything: Vec<_> = snapshot
        .channels
        .iter()
        .filter(|c| c.is_everything())
        .collect();
    assert_eq!(everything.len(), 1);
    assert!(snapshot.seq >= client.hello.feed_seq);
    assert!(
        snapshot.messages.is_empty(),
        "the all scope has no messages"
    );
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_message_reaches_a_subscribed_client_through_the_feed() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let everything = client.everything().await;
    let snapshot = client.snapshot(SnapshotScope::All).await;
    client.subscribe(snapshot.seq).await.unwrap();

    let sent = client
        .request(MessageSend {
            channel_id: everything,
            client_message_id: "cm-1".to_owned(),
            body: "Hello hub".to_owned(),
            attachments: Vec::new(),
            thread_root_id: None,
            in_timeline: false,
        })
        .await
        .unwrap()
        .message;
    assert_eq!(sent.author, Author::User);

    let entry = client
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Message(m)) if m.id == sent.id))
        .await;
    assert!(entry.seq > snapshot.seq);
    assert_eq!(entry.channel_id, Some(everything));

    // The channel snapshot has the message.
    let channel_snapshot = client.snapshot(SnapshotScope::Channel(everything)).await;
    assert!(channel_snapshot.messages.iter().any(|m| m.id == sent.id));
    assert_eq!(channel_snapshot.channel_files.len(), 0);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn sending_the_same_client_message_id_twice_stores_one_message() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let everything = client.everything().await;
    let request = || MessageSend {
        channel_id: everything,
        client_message_id: "cm-same".to_owned(),
        body: "Once".to_owned(),
        attachments: Vec::new(),
        thread_root_id: None,
        in_timeline: false,
    };
    let first = client.request(request()).await.unwrap().message;
    let second = client.request(request()).await.unwrap().message;
    assert_eq!(first.id, second.id);

    let page = client
        .request(MessageList {
            channel_id: everything,
            thread_root_id: None,
            before: None,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(
        page.messages
            .iter()
            .filter(|m| m.author == Author::User)
            .count(),
        1
    );
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn subscribing_from_an_older_position_replays_the_missed_entries() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let everything = client.everything().await;
    let before = client.snapshot(SnapshotScope::All).await.seq;

    // Two messages arrive while the client is not subscribed.
    for i in 0..2 {
        client
            .request(MessageSend {
                channel_id: everything,
                client_message_id: format!("cm-{i}"),
                body: format!("missed {i}"),
                attachments: Vec::new(),
                thread_root_id: None,
                in_timeline: false,
            })
            .await
            .unwrap();
    }
    client.subscribe(before).await.unwrap();
    let mut seen = Vec::new();
    for _ in 0..2 {
        let entry = client
            .wait_feed(|e| {
                matches!(upserted(e), Some(EntityPayload::Message(m)) if m.body.starts_with("missed"))
                    && !seen.contains(&e.seq)
            })
            .await;
        seen.push(entry.seq);
    }
    // The replay keeps the order of the sequence numbers.
    assert!(seen[0] < seen[1]);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_position_the_hub_does_not_have_needs_a_new_snapshot() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let error = client
        .request(SyncSubscribe {
            after_seq: 1_000_000,
            channel_ids: None,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ResyncRequired);
    hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_channel_filter_hides_the_entries_of_other_channels() {
    let hub = TestHub::start().await;
    let mut client = FakeClient::pair_and_connect(&hub).await;
    let everything = client.everything().await;
    let other = client
        .request(consort_proto::client_api::ChannelCreate {
            name: "Other".to_owned(),
            purpose: String::new(),
            instructions: String::new(),
            icon: None,
            color: None,
            group_id: None,
            coordinator_selection: None,
            task_selection: None,
            repo_ids: Vec::new(),
            default_isolation: None,
            default_approval_mode: None,
            memory_enabled: None,
            tags: Vec::new(),
        })
        .await
        .unwrap()
        .channel;
    let head = client.snapshot(SnapshotScope::All).await.seq;
    client
        .request(SyncSubscribe {
            after_seq: head,
            channel_ids: Some(vec![other.id]),
        })
        .await
        .unwrap();

    // A message in Everything first, then one in the other channel.
    for (channel, text) in [(everything, "in everything"), (other.id, "in other")] {
        client
            .request(MessageSend {
                channel_id: channel,
                client_message_id: text.to_owned(),
                body: text.to_owned(),
                attachments: Vec::new(),
                thread_root_id: None,
                in_timeline: false,
            })
            .await
            .unwrap();
    }
    client
        .wait_feed(
            |e| matches!(upserted(e), Some(EntityPayload::Message(m)) if m.body == "in other"),
        )
        .await;
    assert!(!client.feed.iter().any(
        |e| matches!(upserted(e), Some(EntityPayload::Message(m)) if m.body == "in everything")
    ));
    hub.handle.shutdown().await.unwrap();
}

/// A star from one client reaches another client through the feed, and the
/// next snapshot has it.
#[tokio::test]
async fn a_favorite_model_reaches_every_client() {
    let hub = TestHub::start().await;
    let mut desktop = FakeClient::pair_and_connect(&hub).await;
    let code = desktop.request(ClientPairingCode {}).await.unwrap();
    let mut phone = FakeClient::pair_with_code_and_connect(&hub, &code.code.to_string()).await;
    let snapshot = phone.snapshot(SnapshotScope::All).await;
    assert_eq!(
        snapshot.preferences.map(|p| p.favorite_models.len()),
        Some(0)
    );
    phone.subscribe(snapshot.seq).await.unwrap();

    let opus = FavoriteModel {
        account_label: "Claude Personal".into(),
        model: "claude-opus-5-5".into(),
    };
    desktop
        .request(PreferencesSetFavoriteModel {
            model: opus.clone(),
            favorite: true,
        })
        .await
        .unwrap();
    let entry = phone
        .wait_feed(|e| matches!(upserted(e), Some(EntityPayload::Preferences(_))))
        .await;
    assert_eq!(entry.entity_id, "preferences");
    let Some(EntityPayload::Preferences(preferences)) = upserted(&entry) else {
        panic!("a preferences upsert");
    };
    assert_eq!(preferences.favorite_models, std::slice::from_ref(&opus));
    let again = desktop.snapshot(SnapshotScope::All).await;
    assert_eq!(again.preferences.unwrap().favorite_models, [opus]);
    hub.handle.shutdown().await.unwrap();
}
