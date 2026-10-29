//! The wake event queue: events that go back to the front of one channel's
//! queue must not collide with the queued events of another channel.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use consort_hub::store::Store;
use consort_hub::store::coordinator::pending_wakes;
use consort_proto::ids::{ChannelId, MessageId};
use consort_proto::time::Timestamp;
use consort_proto::turn_input::WakeEvent;

fn wake(text: &str) -> WakeEvent {
    WakeEvent::UserMessage {
        id: MessageId::new(),
        thread: None,
        at: Timestamp::now(),
        text: text.to_owned(),
    }
}

fn texts(events: Vec<(i64, WakeEvent)>) -> Vec<String> {
    events
        .into_iter()
        .map(|(_, e)| match e {
            WakeEvent::UserMessage { text, .. } => text,
            other => panic!("unexpected event {other:?}"),
        })
        .collect()
}

#[tokio::test]
async fn two_channels_with_empty_queues_can_each_requeue_events() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let (a, b) = (ChannelId::new(), ChannelId::new());
    let e = wake("a");
    store
        .write(move |tx| tx.requeue_wakes_first(a, &[e]))
        .await
        .unwrap();
    let e = wake("b");
    store
        .write(move |tx| tx.requeue_wakes_first(b, &[e]))
        .await
        .unwrap();
    assert_eq!(
        texts(store.read(move |c| pending_wakes(c, a)).await.unwrap()),
        ["a"]
    );
    assert_eq!(
        texts(store.read(move |c| pending_wakes(c, b)).await.unwrap()),
        ["b"]
    );
}

#[tokio::test]
async fn requeued_events_go_first_even_when_another_channel_has_lower_ids() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let (c, d) = (ChannelId::new(), ChannelId::new());
    let (c1, d1) = (wake("c1"), wake("d1"));
    store
        .write(move |tx| {
            tx.queue_wake(c, &c1)?;
            tx.queue_wake(d, &d1)
        })
        .await
        .unwrap();
    let back = [wake("d0a"), wake("d0b")];
    store
        .write(move |tx| tx.requeue_wakes_first(d, &back))
        .await
        .unwrap();
    assert_eq!(
        texts(
            store
                .read(move |conn| pending_wakes(conn, d))
                .await
                .unwrap()
        ),
        ["d0a", "d0b", "d1"]
    );
    assert_eq!(
        texts(
            store
                .read(move |conn| pending_wakes(conn, c))
                .await
                .unwrap()
        ),
        ["c1"]
    );
}
