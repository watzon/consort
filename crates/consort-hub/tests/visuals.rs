//! Visuals: `show_visual` from a session host, and `visual.render` from a
//! client. The fake session host uploads the page and the image the way the
//! real one does, with a signed request.

// Test code may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use crate::common::*;
use consort_proto::auth::{
    HEADER_SIGNATURE, HEADER_TIMESTAMP, HEADER_WORKER_ID, blob_path, http_signing_payload,
};
use consort_proto::client_api::{MessageList, VisualRender};
use consort_proto::error::ErrorCode;
use consort_proto::frame::{Frame, FrameId, MessageFamily, RequestFrame};
use consort_proto::ids::{MessageId, SessionId};
use consort_proto::model::{
    BlobRef, CssVariable, Message, MessageKind, Sha256Hash, VISUAL_PAGE_MIME, VisualImage,
    VisualMode, VisualNotice, VisualStyle,
};
use consort_proto::session_host_link::HubToSessionHost;
use consort_proto::time::Timestamp;
use consort_proto::tools::ToolDefinition;
use ed25519_dalek::Signer;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

// Helpers -----------------------------------------------------------------------

fn hash_of(bytes: &[u8]) -> Sha256Hash {
    Sha256Hash::from_bytes(Sha256::digest(bytes).into())
}

/// Uploads a blob as the session host, with a signed request.
async fn upload(world: &World, bytes: &[u8]) -> Sha256Hash {
    let hash = hash_of(bytes);
    let path = blob_path(&hash);
    let now = Timestamp::now();
    let payload = http_signing_payload(
        "PUT",
        &world.hub.origin(),
        &path,
        now,
        world.worker.worker_id,
    );
    let signature =
        consort_proto::auth::Signature::from_bytes(world.worker.key.sign(&payload).to_bytes())
            .to_hex();
    let response = reqwest::Client::new()
        .put(format!("{}{path}", world.hub.url))
        .header(HEADER_WORKER_ID, world.worker.worker_id.to_string())
        .header(HEADER_TIMESTAMP, now.as_millis().to_string())
        .header(HEADER_SIGNATURE, signature)
        .body(bytes.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    hash
}

/// The arguments that the session host sends after it prepared the page.
async fn prepared(world: &World, title: &str, html: &str) -> Value {
    let sha256 = upload(world, html.as_bytes()).await;
    json!({
        "html": "",
        "title": title,
        "height": 300,
        "page": BlobRef {
            sha256,
            name: format!("{title}.html"),
            mime: VISUAL_PAGE_MIME.to_owned(),
            size: html.len() as u64,
        },
        "heights": [
            {"width": 720, "height": 400},
            {"width": 320, "height": 800},
        ],
    })
}

async fn show(world: &mut World, session_id: SessionId, title: &str) -> MessageId {
    let arguments = prepared(world, title, "<p>Hello</p>").await;
    let result = world
        .worker
        .tool_json(session_id, "show_visual", arguments)
        .await;
    assert!(
        result["message"]
            .as_str()
            .unwrap()
            .contains("Do not describe"),
        "{result}"
    );
    result["message_id"].as_str().unwrap().parse().unwrap()
}

async fn messages(world: &mut World, thread: Option<MessageId>) -> Vec<Message> {
    world
        .client
        .request(MessageList {
            channel_id: world.tutti,
            thread_root_id: thread,
            before: None,
            limit: Some(200),
        })
        .await
        .unwrap()
        .messages
}

fn style() -> VisualStyle {
    VisualStyle {
        mode: VisualMode::Light,
        variables: vec![CssVariable {
            name: "--background".to_owned(),
            value: "#FFFFFF".to_owned(),
        }],
    }
}

fn render(message_id: MessageId) -> VisualRender {
    VisualRender {
        message_id,
        width: 720,
        scale: 2,
        style: style(),
    }
}

async fn tool_names(world: &mut World, session_id: SessionId) -> Vec<String> {
    let value = world
        .worker
        .peer
        .call_raw("tool.list", json!({"session_id": session_id}))
        .await
        .unwrap();
    let tools: Vec<ToolDefinition> = serde_json::from_value(value["tools"].clone()).unwrap();
    tools.into_iter().map(|t| t.name).collect()
}

// Tests --------------------------------------------------------------------------

#[tokio::test]
async fn a_coordinator_posts_a_visual_in_its_channel_and_a_client_gets_an_image_of_it() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Show me a chart").await;
    let id = show(&mut world, start.session_id, "Tokens by week").await;

    let timeline = messages(&mut world, None).await;
    let visual = timeline.iter().find(|m| m.id == id).expect("the visual");
    assert_eq!(visual.kind, MessageKind::Notice);
    assert_eq!(visual.thread_root_id, None);
    assert_eq!(visual.attachments.len(), 1);
    assert_eq!(visual.attachments[0].mime, VISUAL_PAGE_MIME);
    assert_eq!(visual.attachments[0].name, "Tokens by week.html");
    let notice = VisualNotice::parse(&visual.body).expect("a visual notice");
    assert_eq!(notice.title, "Tokens by week");
    assert_eq!(notice.heights.len(), 2);
    assert_eq!(notice.heights[0].width, 320);

    // The first request asks the session host, which uploads the PNG.
    let page = visual.attachments[0].clone();
    let png = b"\x89PNG not really".to_vec();
    let png_hash = upload(&world, &png).await;
    let World { worker, client, .. } = &mut world;
    let (image, ()) = tokio::join!(client.request(render(id)), async {
        let (request, command) = worker.next_command().await;
        let HubToSessionHost::VisualRender(command) = command else {
            panic!("expected visual.render, got {}", command.type_name());
        };
        assert_eq!(command.page, page);
        assert_eq!((command.width, command.scale), (720, 2));
        assert_eq!(command.style, style());
        worker
            .respond_ok(
                &request,
                &VisualImage {
                    image: BlobRef {
                        sha256: png_hash.clone(),
                        name: "page.png".to_owned(),
                        mime: "image/png".to_owned(),
                        size: png.len() as u64,
                    },
                    height: 412,
                },
            )
            .await;
    });
    let image = image.unwrap();
    assert_eq!(image.height, 412);
    assert_eq!(image.image.sha256, png_hash);
    assert_eq!(image.image.mime, "image/png");

    // The second request is answered from the cache: no command goes out.
    let again = world.client.request(render(id)).await.unwrap();
    assert_eq!(again, image);
    assert!(
        world
            .worker
            .try_next_command(Duration::from_millis(300))
            .await
            .is_none(),
        "the cache answered"
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_task_session_posts_a_visual_in_the_thread_of_its_task() {
    let mut world = World::start().await;
    let (task_id, start) = world.start_task("Make a chart.").await;
    let id = show(&mut world, start.session_id, "Totals").await;
    let task = world.task(task_id).await;

    let thread = messages(&mut world, Some(task.card_message_id)).await;
    let visual = thread.iter().find(|m| m.id == id).expect("in the thread");
    assert_eq!(visual.thread_root_id, Some(task.card_message_id));
    assert!(VisualNotice::parse(&visual.body).is_some());
    let timeline = messages(&mut world, None).await;
    assert!(
        timeline.iter().all(|m| m.id != id),
        "not in the timeline of the channel"
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn both_visual_tools_are_listed_for_a_coordinator_and_a_task_session() {
    let mut world = World::start().await;
    let (_, task) = world.start_task("Make a chart.").await;
    let coordinator = world.coordinator_session.unwrap();
    for session in [coordinator, task.session_id] {
        let names = tool_names(&mut world, session).await;
        for tool in ["show_visual", "preview_visual"] {
            assert!(names.contains(&tool.to_owned()), "{tool} in {names:?}");
        }
    }
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_hub_refuses_a_visual_without_a_page_and_a_preview_with_a_hint_to_update() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Show me a chart").await;
    let result = world
        .worker
        .tool_call(
            start.session_id,
            "show_visual",
            json!({"html": "<p>Hi</p>", "title": "Chart"}),
        )
        .await;
    assert!(result.is_error);
    let text = serde_json::to_string(&result).unwrap();
    assert!(text.contains("too old"), "{text}");
    let result = world
        .worker
        .tool_call(
            start.session_id,
            "preview_visual",
            json!({"html": "<p>Hi</p>"}),
        )
        .await;
    assert!(result.is_error);
    let text = serde_json::to_string(&result).unwrap();
    assert!(text.contains("too old"), "{text}");
    // Nothing was posted.
    let timeline = messages(&mut world, None).await;
    assert!(timeline.iter().all(|m| m.kind != MessageKind::Notice));
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn visual_render_refuses_a_bad_request_and_a_message_that_is_not_a_visual() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Show me a chart").await;
    let id = show(&mut world, start.session_id, "Chart").await;

    // The message of the user that started the session is a plain message.
    let timeline = messages(&mut world, None).await;
    let text = timeline
        .iter()
        .find(|m| m.kind == MessageKind::Text)
        .expect("a text message");
    let error = world.client.request(render(text.id)).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
    let error = world
        .client
        .request(render(MessageId::new()))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);

    let error = world
        .client
        .request(VisualRender {
            width: 10_000,
            ..render(id)
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Invalid);
    let error = world
        .client
        .request(VisualRender {
            scale: 9,
            ..render(id)
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Invalid);
    assert!(
        world
            .worker
            .try_next_command(Duration::from_millis(300))
            .await
            .is_none(),
        "the session host was not asked"
    );
    world.hub.handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_render_that_waits_for_the_session_host_does_not_block_the_other_requests() {
    let mut world = World::start().await;
    let start = world.start_coordinator("Show me a chart").await;
    let id = show(&mut world, start.session_id, "Chart").await;

    // The client sends the request and does not wait. The session host gets
    // the command and does not answer yet.
    let frame_id = FrameId::generate();
    world
        .client
        .peer
        .send(&Frame::Request(RequestFrame {
            id: frame_id.clone(),
            kind: "visual.render".to_owned(),
            body: serde_json::to_value(render(id)).unwrap(),
        }))
        .await;
    let (request, _) = world.worker.next_command().await;

    // Another request on the same connection gets its answer at once.
    let timeline = messages(&mut world, None).await;
    assert!(timeline.iter().any(|m| m.id == id));

    // Now the session host answers, and the client gets its image.
    let png_hash = upload(&world, b"a png").await;
    world
        .worker
        .respond_ok(
            &request,
            &VisualImage {
                image: BlobRef {
                    sha256: png_hash.clone(),
                    name: "page.png".to_owned(),
                    mime: "image/png".to_owned(),
                    size: 5,
                },
                height: 250,
            },
        )
        .await;
    let answer = loop {
        match world.client.peer.next_frame(WAIT).await {
            Some(Frame::Response(response)) if response.re == frame_id => break response.result,
            Some(_) => {}
            None => panic!("no answer to visual.render"),
        }
    };
    let image: VisualImage = serde_json::from_value(answer.unwrap()).unwrap();
    assert_eq!(image.height, 250);
    assert_eq!(image.image.sha256, png_hash);
    world.hub.handle.shutdown().await.unwrap();
}
