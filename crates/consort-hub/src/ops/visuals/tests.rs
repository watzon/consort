//! Tests of `show_visual`, `visual.render` and the image cache.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::hub::{test_hub, test_task_session};
use crate::net::ws::Outbox;
use consort_proto::client_api::ChannelCreate;
use consort_proto::frame::RequestFrame;
use consort_proto::ids::WorkerId;
use consort_proto::model::{
    CssVariable, MachineKind, Message, Os, SessionRole, VisualMode, VisualStyle, Worker,
    WorkerState,
};
use consort_proto::time::Timestamp;
use consort_proto::tools::ToolAudience;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// Helpers ----------------------------------------------------------------

async fn task_ctx(hub: &Hub) -> ToolCtx {
    let (task, session) = test_task_session(hub).await;
    let channel = hub.store.read(conversation::tutti_channel).await.unwrap();
    ToolCtx {
        session,
        channel,
        task: Some(task),
        audience: ToolAudience::TaskSession,
    }
}

fn coordinator_ctx(mut ctx: ToolCtx) -> ToolCtx {
    ctx.task = None;
    ctx.session.role = SessionRole::Coordinator;
    ctx.audience = ToolAudience::TuttiCoordinator;
    ctx
}

/// Stores the session of `ctx` as a coordinator session, so that
/// `call_tool` finds it, and opens a turn with these events.
async fn coordinator_turn(
    hub: &Hub,
    ctx: &ToolCtx,
    events: Vec<consort_proto::turn_input::WakeEvent>,
) -> consort_proto::ids::SessionId {
    let (channel_id, session_id) = (ctx.channel.id, ctx.session.id);
    let session = coordinator_ctx(ctx.clone()).session;
    hub.store
        .write(move |tx| {
            let mut stored = session;
            stored.task_id = None;
            tx.save_session(&stored)?;
            tx.save_coordinator(&crate::store::coordinator::CoordinatorRow {
                channel_id,
                session_id: Some(session_id),
                turn_open: true,
                turn_started_at: Some(tx.now()),
                turn_events: events,
            })
        })
        .await
        .unwrap();
    session_id
}

async fn put_blob(hub: &Hub, bytes: &[u8]) -> Sha256Hash {
    let hash = Sha256Hash::from_bytes(Sha256::digest(bytes).into());
    tokio::fs::write(hub.store.blobs_dir().join(hash.as_str()), bytes)
        .await
        .unwrap();
    hash
}

async fn page_ref(hub: &Hub, html: &str) -> BlobRef {
    let sha256 = put_blob(hub, html.as_bytes()).await;
    BlobRef {
        sha256,
        name: "Tokens by week.html".to_owned(),
        mime: VISUAL_PAGE_MIME.to_owned(),
        size: html.len() as u64,
    }
}

fn args(title: &str, page: Option<BlobRef>) -> ShowVisualArgs {
    ShowVisualArgs {
        html: String::new(),
        title: title.to_owned(),
        height: Some(300),
        page,
        heights: Some(vec![
            VisualHeight {
                width: 720,
                height: 400,
            },
            VisualHeight {
                width: 320,
                height: 800,
            },
        ]),
    }
}

async fn show(hub: &Hub, ctx: &ToolCtx) -> MessageId {
    show_page(hub, ctx, "<p>Hello</p>").await
}

/// Posts a visual with this page. Two visuals with one page share their
/// images, so a test that needs two images gives two pages.
async fn show_page(hub: &Hub, ctx: &ToolCtx, html: &str) -> MessageId {
    let page = page_ref(hub, html).await;
    let value = hub
        .tool_show_visual(ctx, args("Tokens by week", Some(page)))
        .await
        .unwrap();
    serde_json::from_value::<ShowVisualResult>(value)
        .unwrap()
        .message_id
}

async fn message(hub: &Hub, id: MessageId) -> Message {
    hub.store
        .read(move |conn| conversation::message_or_404(conn, id))
        .await
        .unwrap()
}

fn style(mode: VisualMode, accent: &str) -> VisualStyle {
    VisualStyle {
        mode,
        variables: vec![
            CssVariable {
                name: "--background".to_owned(),
                value: "#131E1A".to_owned(),
            },
            CssVariable {
                name: "--primary".to_owned(),
                value: accent.to_owned(),
            },
        ],
    }
}

fn render(message_id: MessageId, width: u32) -> VisualRender {
    VisualRender {
        message_id,
        width,
        scale: 2,
        style: style(VisualMode::Dark, "#77B4ED"),
    }
}

/// A session host with a link that the test reads and answers.
struct FakeHost {
    id: WorkerId,
    link: Arc<WorkerLink>,
    rx: mpsc::Receiver<String>,
}

impl FakeHost {
    async fn online(hub: &Hub, seed: u8, paired_at: i64) -> Self {
        let id = WorkerId::new();
        let worker = Worker {
            kind: MachineKind::SessionHost,
            id,
            name: format!("Host {seed}"),
            hostname: "hub".to_owned(),
            os: Os::Macos,
            arch: "aarch64".to_owned(),
            public_key: consort_proto::auth::PublicKey::from_bytes([seed; 32]),
            version: String::new(),
            protocol_version: 1,
            tags: Vec::new(),
            paused: false,
            state: WorkerState::Online,
            last_seen_at: None,
            inventory: None,
            paired_at: Timestamp::from_millis(paired_at),
            revoked_at: None,
        };
        hub.store
            .write(move |tx| tx.save_worker(&worker))
            .await
            .unwrap();
        let (outbox, rx) = Outbox::new(CancellationToken::new());
        let link = Arc::new(WorkerLink::new(hub.links.next_conn_id(), id, outbox));
        hub.links.register(link.clone());
        Self { id, link, rx }
    }

    /// The next command that the hub sent to this host.
    async fn command(&mut self) -> (RequestFrame, VisualRenderCommand) {
        let text = tokio::time::timeout(Duration::from_secs(10), self.rx.recv())
            .await
            .expect("a command in time")
            .expect("an open link");
        let Frame::Request(request) = Frame::parse(&text).unwrap() else {
            panic!("expected a request");
        };
        let HubToSessionHost::VisualRender(command) = request.decode().unwrap() else {
            panic!("expected visual.render");
        };
        (request, command)
    }

    fn answer(&self, request: &RequestFrame, result: std::result::Result<Value, ErrorBody>) {
        self.link.resolve(&request.id, result);
    }

    /// Answers with an image whose PNG the hub holds.
    async fn draw(&self, hub: &Hub, request: &RequestFrame, png: &[u8], height: u32) {
        let sha256 = put_blob(hub, png).await;
        let image = VisualImage {
            image: BlobRef {
                sha256,
                name: "page.png".to_owned(),
                mime: "image/png".to_owned(),
                size: png.len() as u64,
            },
            height,
        };
        self.answer(request, Ok(serde_json::to_value(image).unwrap()));
    }

    fn sent_nothing(&mut self) -> bool {
        self.rx.try_recv().is_err()
    }
}

// show_visual ---------------------------------------------------------------

#[tokio::test]
async fn show_visual_posts_a_notice_in_the_task_thread() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let page = page_ref(&hub, "<p>Hello</p>").await;
    let value = hub
        .tool_show_visual(&ctx, args("  Tokens by week ", Some(page.clone())))
        .await
        .unwrap();
    let result: ShowVisualResult = serde_json::from_value(value).unwrap();
    assert_eq!(result.message, SHOW_VISUAL_SHOWN);
    let posted = message(&hub, result.message_id).await;
    assert_eq!(posted.kind, MessageKind::Notice);
    assert_eq!(posted.author, Author::Assistant);
    assert_eq!(
        posted.thread_root_id,
        ctx.task.as_ref().map(|t| t.card_message_id)
    );
    assert_eq!(posted.attachments, vec![page], "the page keeps its name");
    let notice = VisualNotice::parse(&posted.body).expect("a visual notice");
    assert_eq!(notice.title, "Tokens by week");
    assert_eq!(notice.text, "Tokens by week");
    assert_eq!(notice.height, 300);
    let widths: Vec<u32> = notice.heights.iter().map(|h| h.width).collect();
    assert_eq!(widths, [320, 720], "the table is sorted");
}

#[tokio::test]
async fn show_visual_in_a_coordinator_session_posts_in_the_channel() {
    let (hub, _dir) = test_hub().await;
    let ctx = coordinator_ctx(task_ctx(&hub).await);
    let id = show(&hub, &ctx).await;
    let posted = message(&hub, id).await;
    assert_eq!(posted.channel_id, ctx.channel.id);
    assert_eq!(posted.thread_root_id, None);
    assert_eq!(posted.attachments.len(), 1);
}

#[tokio::test]
async fn show_visual_with_no_height_table_keeps_the_height_that_was_given() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let mut call = args("Chart", Some(page_ref(&hub, "<p>x</p>").await));
    call.heights = None;
    call.height = None;
    let value = hub.tool_show_visual(&ctx, call).await.unwrap();
    let id = serde_json::from_value::<ShowVisualResult>(value)
        .unwrap()
        .message_id;
    let notice = VisualNotice::parse(&message(&hub, id).await.body).unwrap();
    assert!(notice.heights.is_empty());
    assert_eq!(notice.height, consort_proto::model::VISUAL_HEIGHT_DEFAULT);
}

#[tokio::test]
async fn show_visual_in_a_proactive_turn_marks_the_visual() {
    use consort_proto::model::ProactiveKind;
    let (hub, _dir) = test_hub().await;
    let ctx = coordinator_ctx(task_ctx(&hub).await);
    coordinator_turn(
        &hub,
        &ctx,
        vec![consort_proto::turn_input::WakeEvent::FollowUp {
            routine: consort_proto::ids::RoutineId::new(),
            reason: "Show the totals.".to_owned(),
            set_at: Timestamp::from_millis(1),
            log: 1,
        }],
    )
    .await;
    let id = show(&hub, &ctx).await;
    let mark = message(&hub, id).await.proactive.expect("a proactive mark");
    assert_eq!(mark.kind, ProactiveKind::FollowUp);
}

#[tokio::test]
async fn show_visual_is_refused_in_the_research_pass_and_posts_nothing() {
    let (hub, _dir) = test_hub().await;
    let ctx = coordinator_ctx(task_ctx(&hub).await);
    coordinator_turn(
        &hub,
        &ctx,
        vec![consort_proto::turn_input::WakeEvent::Research {
            routine: consort_proto::ids::RoutineId::new(),
            log: 1,
        }],
    )
    .await;
    let page = page_ref(&hub, "<p>x</p>").await;
    let error = hub
        .tool_show_visual(&ctx, args("Chart", Some(page)))
        .await
        .unwrap_err();
    assert!(error.to_error_body().message.contains("read-only"));
    let channel_id = ctx.channel.id;
    let (posted, _) = hub
        .store
        .read(move |conn| conversation::messages_page(conn, channel_id, None, None, 50, 1 << 20))
        .await
        .unwrap();
    assert!(posted.iter().all(|m| m.kind != MessageKind::Notice));
}

#[tokio::test]
async fn show_visual_refuses_a_call_the_session_host_did_not_prepare() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let good = page_ref(&hub, "<p>x</p>").await;

    // No page: the session host is too old.
    let error = hub
        .tool_show_visual(&ctx, args("Chart", None))
        .await
        .unwrap_err()
        .to_error_body();
    assert_eq!(error.code, ErrorCode::Unsupported);
    assert!(error.message.contains("too old"), "{}", error.message);
    assert!(error.message.contains("Update"), "{}", error.message);

    // A page that is not in the store.
    let mut missing = good.clone();
    missing.sha256 = Sha256Hash::from_bytes([7; 32]);
    let error = hub
        .tool_show_visual(&ctx, args("Chart", Some(missing)))
        .await
        .unwrap_err()
        .to_error_body();
    assert_eq!(error.code, ErrorCode::Invalid);
    assert!(error.message.contains("no blob"), "{}", error.message);

    // A page of the wrong type.
    let mut wrong = good.clone();
    wrong.mime = "text/plain".to_owned();
    let error = hub
        .tool_show_visual(&ctx, args("Chart", Some(wrong)))
        .await
        .unwrap_err()
        .to_error_body();
    assert_eq!(error.code, ErrorCode::Invalid);
    assert!(error.message.contains(VISUAL_PAGE_MIME));

    // A page that claims to be too large.
    let mut big = good.clone();
    big.size = VISUAL_PAGE_MAX_BYTES + 1;
    assert!(
        hub.tool_show_visual(&ctx, args("Chart", Some(big)))
            .await
            .is_err()
    );

    // A bad title.
    for title in ["", "   ", &"t".repeat(VISUAL_TITLE_MAX_CHARS + 1)] {
        let error = hub
            .tool_show_visual(&ctx, args(title, Some(good.clone())))
            .await
            .unwrap_err()
            .to_error_body();
        assert_eq!(error.code, ErrorCode::Invalid, "{title:?}");
    }

    // A bad height table.
    let mut many = args("Chart", Some(good.clone()));
    many.heights = Some(
        (0..=HEIGHTS_MAX as u32)
            .map(|i| VisualHeight {
                width: 320 + i,
                height: 100,
            })
            .collect(),
    );
    assert!(hub.tool_show_visual(&ctx, many).await.is_err());
    for width in [VISUAL_RENDER_WIDTH_MIN - 1, VISUAL_RENDER_WIDTH_MAX + 1] {
        let mut bad = args("Chart", Some(good.clone()));
        bad.heights = Some(vec![VisualHeight { width, height: 100 }]);
        assert!(hub.tool_show_visual(&ctx, bad).await.is_err(), "{width}");
    }

    // Nothing was posted in the thread.
    let root = ctx.task.as_ref().unwrap().card_message_id;
    let thread = hub
        .store
        .read(move |conn| conversation::thread_replies(conn, root))
        .await
        .unwrap();
    assert!(thread.iter().all(|m| m.kind != MessageKind::Notice));
}

#[tokio::test]
async fn show_visual_takes_the_size_from_the_stored_blob() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let mut page = page_ref(&hub, "<p>twelve</p>").await;
    page.size = 3;
    let value = hub
        .tool_show_visual(&ctx, args("Chart", Some(page)))
        .await
        .unwrap();
    let id = serde_json::from_value::<ShowVisualResult>(value)
        .unwrap()
        .message_id;
    assert_eq!(message(&hub, id).await.attachments[0].size, 13);
}

#[tokio::test]
async fn preview_visual_sent_to_the_hub_is_refused_with_a_hint_to_update() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let result = hub
        .call_tool(
            ctx.session.id,
            "preview_visual",
            json!({"html": "<p>x</p>"}),
        )
        .await;
    assert!(result.is_error);
    let text = serde_json::to_string(&result).unwrap();
    assert!(text.contains("too old"), "{text}");
    assert!(text.contains("unsupported"), "{text}");
}

// The turn gates --------------------------------------------------------------

#[tokio::test]
async fn research_suggestion_and_voice_turns_refuse_both_visual_tools() {
    use consort_proto::ids::{RoutineId, TaskId};
    use consort_proto::turn_input::{SuggestionSource, WakeEvent};
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let page = page_ref(&hub, "<p>x</p>").await;
    let show_args = json!({
        "html": "",
        "title": "Chart",
        "page": page,
    });
    let turns = [
        (
            WakeEvent::Research {
                routine: RoutineId::new(),
                log: 1,
            },
            crate::proactivity::turn::RESEARCH_REFUSAL,
        ),
        (
            WakeEvent::MemorySuggestion {
                from: SuggestionSource::Task(TaskId::new()),
                reason: "Keep it.".to_owned(),
                note: "A note.".to_owned(),
            },
            crate::proactivity::turn::SUGGESTION_REFUSAL,
        ),
        (
            WakeEvent::VoiceSample {
                sample: 1,
                request: "Write to Ana.".to_owned(),
            },
            crate::proactivity::turn::VOICE_TURN_REFUSAL,
        ),
    ];
    for (event, refusal) in turns {
        let session_id = coordinator_turn(&hub, &ctx, vec![event]).await;
        for (tool, arguments) in [
            ("show_visual", show_args.clone()),
            ("preview_visual", json!({"html": "<p>x</p>"})),
        ] {
            let result = hub.call_tool(session_id, tool, arguments).await;
            assert!(result.is_error, "{tool}");
            let text = serde_json::to_string(&result).unwrap();
            assert!(text.contains("policy_denied"), "{tool}: {text}");
            assert!(
                text.contains(&refusal[..30]),
                "{tool} gave another refusal: {text}"
            );
        }
    }
    // A normal turn may show a visual.
    let session_id = coordinator_turn(&hub, &ctx, Vec::new()).await;
    let result = hub.call_tool(session_id, "show_visual", show_args).await;
    assert!(!result.is_error, "{:?}", result.content);
}

async fn tool_names(hub: &Hub, session_id: consort_proto::ids::SessionId) -> Vec<String> {
    hub.tool_definitions(session_id)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect()
}

#[tokio::test]
async fn a_voice_task_cannot_show_a_visual() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let task_id = ctx.task.as_ref().unwrap().id;
    let names = tool_names(&hub, ctx.session.id).await;
    assert!(names.contains(&"show_visual".to_owned()));
    assert!(names.contains(&"preview_visual".to_owned()));
    hub.store
        .write(move |tx| tx.mark_voice_task(task_id))
        .await
        .unwrap();
    let names = tool_names(&hub, ctx.session.id).await;
    assert!(!names.contains(&"show_visual".to_owned()), "{names:?}");
    assert!(!names.contains(&"preview_visual".to_owned()), "{names:?}");
    let page = page_ref(&hub, "<p>x</p>").await;
    let result = hub
        .call_tool(
            ctx.session.id,
            "show_visual",
            json!({"html": "", "title": "Chart", "page": page}),
        )
        .await;
    assert!(result.is_error);
    let text = serde_json::to_string(&result).unwrap();
    assert!(text.contains("voice task"), "{text}");
}

// The cache key -----------------------------------------------------------------

#[test]
fn the_cache_key_follows_the_page_the_size_the_scale_and_the_style() {
    let page = Sha256Hash::from_bytes([1; 32]);
    let message_id = MessageId::new();
    let base = render(message_id, 720);
    let key = image_key(&page, &base);
    assert_eq!(key.len(), 64);
    // The same inputs give the same key. The message does not matter.
    assert_eq!(key, image_key(&page, &render(MessageId::new(), 720)));
    let mut other = base.clone();
    other.width = 721;
    assert_ne!(key, image_key(&page, &other));
    let mut other = base.clone();
    other.scale = 3;
    assert_ne!(key, image_key(&page, &other));
    let mut other = base.clone();
    other.style.mode = VisualMode::Light;
    assert_ne!(key, image_key(&page, &other));
    let mut other = base.clone();
    other.style.variables[1].value = "#2F74C0".to_owned();
    assert_ne!(key, image_key(&page, &other));
    let mut other = base.clone();
    other.style.variables.reverse();
    assert_ne!(key, image_key(&page, &other), "the order counts");
    assert_ne!(key, image_key(&Sha256Hash::from_bytes([2; 32]), &base));
}

// visual.render ---------------------------------------------------------------------

#[tokio::test]
async fn visual_render_asks_the_session_host_once_and_then_answers_from_the_cache() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let id = show(&hub, &ctx).await;
    let page = message(&hub, id).await.attachments[0].clone();
    let mut host = FakeHost::online(&hub, 1, 10).await;

    let asked = hub.visual_render(&Caller::Hub, render(id, 720));
    let answered = async {
        let (request, command) = host.command().await;
        assert_eq!(command.page, page);
        assert_eq!((command.width, command.scale), (720, 2));
        assert_eq!(command.style, style(VisualMode::Dark, "#77B4ED"));
        host.draw(&hub, &request, b"png bytes", 412).await;
    };
    let (image, ()) = tokio::join!(asked, answered);
    let image = image.unwrap();
    assert_eq!(image.height, 412);
    assert_eq!(image.image.mime, "image/png");
    assert_eq!(image.image.size, 9);
    assert_eq!(
        image.image.sha256,
        Sha256Hash::from_bytes(Sha256::digest(b"png bytes").into())
    );
    assert_eq!(
        hub.store
            .read(move |conn| visuals::image_count(conn, id))
            .await
            .unwrap(),
        1
    );

    // The second call, and one with another client's identity, hit the cache.
    let again = hub.visual_render(&Caller::Hub, render(id, 720)).await;
    assert_eq!(again.unwrap(), image);
    assert!(host.sent_nothing(), "the cache answered");

    // Another width is another image.
    let narrow = hub.visual_render(&Caller::Hub, render(id, 480));
    let answered = async {
        let (request, command) = host.command().await;
        assert_eq!(command.width, 480);
        host.draw(&hub, &request, b"narrow png", 640).await;
    };
    let (narrow, ()) = tokio::join!(narrow, answered);
    assert_eq!(narrow.unwrap().height, 640);
}

#[tokio::test]
async fn two_requests_for_one_key_at_once_make_one_command() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let id = show(&hub, &ctx).await;
    let mut host = FakeHost::online(&hub, 1, 10).await;

    let first = hub.visual_render(&Caller::Hub, render(id, 720));
    let second = hub.visual_render(&Caller::Hub, render(id, 720));
    let answered = async {
        let (request, _) = host.command().await;
        // Let both requests reach the render before the answer.
        tokio::time::sleep(Duration::from_millis(100)).await;
        host.draw(&hub, &request, b"png bytes", 300).await;
    };
    let (first, second, ()) = tokio::join!(first, second, answered);
    assert_eq!(first.unwrap(), second.unwrap());
    assert!(host.sent_nothing(), "one command only");
    assert!(hub.visual_renders.flights.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_render_that_failed_can_be_tried_again() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let id = show(&hub, &ctx).await;
    let mut host = FakeHost::online(&hub, 1, 10).await;
    let failing = hub.visual_render(&Caller::Hub, render(id, 720));
    let answered = async {
        let (request, _) = host.command().await;
        host.answer(
            &request,
            Err(ErrorBody::new(
                ErrorCode::Internal,
                "The page did not draw.",
            )),
        );
    };
    let (failed, ()) = tokio::join!(failing, answered);
    assert_eq!(
        failed.unwrap_err().to_error_body().message,
        "The page did not draw."
    );
    let retry = hub.visual_render(&Caller::Hub, render(id, 720));
    let answered = async {
        let (request, _) = host.command().await;
        host.draw(&hub, &request, b"png bytes", 300).await;
    };
    let (retry, ()) = tokio::join!(retry, answered);
    assert_eq!(retry.unwrap().height, 300);
}

#[tokio::test]
async fn visual_render_refuses_a_bad_width_scale_or_style_and_asks_nobody() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let id = show(&hub, &ctx).await;
    let mut host = FakeHost::online(&hub, 1, 10).await;
    let mut requests = Vec::new();
    for width in [0, VISUAL_RENDER_WIDTH_MIN - 1, VISUAL_RENDER_WIDTH_MAX + 1] {
        requests.push(render(id, width));
    }
    for scale in [0, VISUAL_SCALE_MAX + 1] {
        requests.push(VisualRender {
            scale,
            ..render(id, 720)
        });
    }
    for (name, value) in [
        ("background", "#fff"),
        ("--bg", "red;}body{display:none"),
        ("--bg", "</style><script>"),
    ] {
        let mut bad = render(id, 720);
        bad.style.variables = vec![CssVariable {
            name: name.to_owned(),
            value: value.to_owned(),
        }];
        requests.push(bad);
    }
    for request in requests {
        let error = hub
            .visual_render(&Caller::Hub, request.clone())
            .await
            .unwrap_err()
            .to_error_body();
        assert_eq!(error.code, ErrorCode::Invalid, "{request:?}");
    }
    assert!(host.sent_nothing());
    // The edges are fine: they get as far as the session host.
    for width in [VISUAL_RENDER_WIDTH_MIN, VISUAL_RENDER_WIDTH_MAX] {
        let edge = hub.visual_render(&Caller::Hub, render(id, width));
        let answered = async {
            let (request, command) = host.command().await;
            assert_eq!(command.width, width);
            host.draw(&hub, &request, format!("png {width}").as_bytes(), 100)
                .await;
        };
        let (edge, ()) = tokio::join!(edge, answered);
        edge.unwrap();
    }
}

#[tokio::test]
async fn visual_render_gives_not_found_for_a_message_that_is_not_a_visual() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let mut host = FakeHost::online(&hub, 1, 10).await;
    let not_found = |error: HubError| {
        let body = error.to_error_body();
        assert_eq!(body.code, ErrorCode::NotFound, "{}", body.message);
    };

    // No such message.
    not_found(
        hub.visual_render(&Caller::Hub, render(MessageId::new(), 720))
            .await
            .unwrap_err(),
    );
    // A text message (the card of the task).
    let card = ctx.task.as_ref().unwrap().card_message_id;
    not_found(
        hub.visual_render(&Caller::Hub, render(card, 720))
            .await
            .unwrap_err(),
    );
    // A notice of another kind.
    let draft = hub
        .store
        .write(move |tx| {
            let mut new = NewMessage::new(
                ctx.channel.id,
                Author::Assistant,
                MessageKind::Notice,
                r#"{"kind":"draft","text":"Letter"}"#,
            );
            new.attachments = vec![BlobRef {
                sha256: Sha256Hash::from_bytes([3; 32]),
                name: "a.html".to_owned(),
                mime: VISUAL_PAGE_MIME.to_owned(),
                size: 1,
            }];
            Ok(tx.insert_message(new)?.id)
        })
        .await
        .unwrap();
    not_found(
        hub.visual_render(&Caller::Hub, render(draft, 720))
            .await
            .unwrap_err(),
    );
    // A deleted visual.
    let ctx = task_ctx(&hub).await;
    let id = show(&hub, &ctx).await;
    hub.store
        .write(move |tx| {
            let mut gone = conversation::message_or_404(tx.db(), id)?;
            gone.deleted_at = Some(tx.now());
            tx.save_message(&gone)
        })
        .await
        .unwrap();
    not_found(
        hub.visual_render(&Caller::Hub, render(id, 720))
            .await
            .unwrap_err(),
    );
    // A visual whose page left the store.
    let ctx = task_ctx(&hub).await;
    let id = show(&hub, &ctx).await;
    let page = message(&hub, id).await.attachments[0].sha256.clone();
    tokio::fs::remove_file(hub.store.blobs_dir().join(page.as_str()))
        .await
        .unwrap();
    not_found(
        hub.visual_render(&Caller::Hub, render(id, 720))
            .await
            .unwrap_err(),
    );
    assert!(host.sent_nothing());
}

#[tokio::test]
async fn visual_render_with_no_session_host_online_is_unavailable() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let id = show(&hub, &ctx).await;
    let error = hub
        .visual_render(&Caller::Hub, render(id, 720))
        .await
        .unwrap_err()
        .to_error_body();
    assert_eq!(error.code, ErrorCode::WorkerOffline);
    assert_eq!(
        error.message,
        "No session host is online to draw this visual. Try again when the hub is online."
    );

    // A host whose connection closed, and a host that was removed, do not count.
    let closed = FakeHost::online(&hub, 1, 10).await;
    closed.link.outbox.close();
    let removed = FakeHost::online(&hub, 2, 20).await;
    let removed_id = removed.id;
    hub.store
        .write(move |tx| {
            tx.revoke_worker(removed_id, crate::store::machine::Origin::HubMachine)
                .map(|_| ())
        })
        .await
        .unwrap();
    let error = hub
        .visual_render(&Caller::Hub, render(id, 720))
        .await
        .unwrap_err()
        .to_error_body();
    assert_eq!(error.code, ErrorCode::WorkerOffline);
}

#[tokio::test]
async fn an_error_of_the_session_host_passes_through_with_its_message() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let id = show(&hub, &ctx).await;
    let mut host = FakeHost::online(&hub, 1, 10).await;
    let asked = hub.visual_render(&Caller::Hub, render(id, 720));
    let answered = async {
        let (request, _) = host.command().await;
        host.answer(
            &request,
            Err(ErrorBody::new(
                ErrorCode::WorkerOffline,
                "The preview browser is installing. Try again in a minute.",
            )),
        );
    };
    let (error, ()) = tokio::join!(asked, answered);
    let error = error.unwrap_err().to_error_body();
    assert_eq!(error.code, ErrorCode::WorkerOffline);
    assert_eq!(
        error.message,
        "The preview browser is installing. Try again in a minute."
    );
    assert_eq!(
        hub.store
            .read(move |conn| visuals::image_count(conn, id))
            .await
            .unwrap(),
        0,
        "an error is not cached"
    );
}

#[tokio::test]
async fn a_session_host_that_does_not_answer_in_time_is_unavailable() {
    let (hub, _dir) = test_hub().await;
    *hub.visual_renders.timeout.lock().unwrap() = Some(Duration::from_millis(100));
    let ctx = task_ctx(&hub).await;
    let id = show(&hub, &ctx).await;
    let _host = FakeHost::online(&hub, 1, 10).await;
    let error = hub
        .visual_render(&Caller::Hub, render(id, 720))
        .await
        .unwrap_err()
        .to_error_body();
    assert_eq!(error.code, ErrorCode::Unavailable);
    assert!(error.message.contains("in time"), "{}", error.message);
}

#[tokio::test]
async fn the_hub_asks_the_session_host_that_paired_first_and_tries_the_next_when_one_drops() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let id = show(&hub, &ctx).await;
    // The later host is created first, so the order is not the order of creation.
    let mut later = FakeHost::online(&hub, 2, 200).await;
    let mut first = FakeHost::online(&hub, 1, 100).await;

    let asked = hub.visual_render(&Caller::Hub, render(id, 720));
    let answered = async {
        let (request, _) = first.command().await;
        first.draw(&hub, &request, b"from the first", 111).await;
    };
    let (image, ()) = tokio::join!(asked, answered);
    assert_eq!(image.unwrap().height, 111);
    assert!(later.sent_nothing(), "the first host drew it");

    // The connection of the first host drops while the hub waits.
    let asked = hub.visual_render(&Caller::Hub, render(id, 480));
    let answered = async {
        let _ = first.command().await;
        first.link.fail_pending();
        let (request, command) = later.command().await;
        assert_eq!(command.width, 480);
        later.draw(&hub, &request, b"from the later", 222).await;
    };
    let (image, ()) = tokio::join!(asked, answered);
    assert_eq!(image.unwrap().height, 222);
}

#[tokio::test]
async fn an_answer_that_cannot_be_used_is_an_error_and_is_not_cached() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let id = show(&hub, &ctx).await;
    let mut host = FakeHost::online(&hub, 1, 10).await;
    let image = |sha256: Sha256Hash, height: u32| VisualImage {
        image: BlobRef {
            sha256,
            name: "page.png".to_owned(),
            mime: "image/png".to_owned(),
            size: 5,
        },
        height,
    };
    let png = put_blob(&hub, b"a png").await;
    for answer in [
        // Text that is not a `VisualImage`.
        json!({"image": "nope"}),
        // The image is not in the store.
        serde_json::to_value(image(Sha256Hash::from_bytes([9; 32]), 100)).unwrap(),
        // A height that is not possible.
        serde_json::to_value(image(png.clone(), 0)).unwrap(),
        serde_json::to_value(image(png.clone(), VISUAL_FIT_MAX + 1)).unwrap(),
    ] {
        let asked = hub.visual_render(&Caller::Hub, render(id, 720));
        let answered = async {
            let (request, _) = host.command().await;
            host.answer(&request, Ok(answer.clone()));
        };
        let (result, ()) = tokio::join!(asked, answered);
        assert!(result.is_err(), "{answer}");
    }
    assert_eq!(
        hub.store
            .read(move |conn| visuals::image_count(conn, id))
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn a_cached_image_whose_blob_is_gone_is_drawn_again() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let id = show(&hub, &ctx).await;
    let mut host = FakeHost::online(&hub, 1, 10).await;
    let first = hub.visual_render(&Caller::Hub, render(id, 720));
    let answered = async {
        let (request, _) = host.command().await;
        host.draw(&hub, &request, b"first png", 300).await;
    };
    let (first, ()) = tokio::join!(first, answered);
    let first = first.unwrap();
    tokio::fs::remove_file(hub.store.blobs_dir().join(first.image.sha256.as_str()))
        .await
        .unwrap();
    let second = hub.visual_render(&Caller::Hub, render(id, 720));
    let answered = async {
        let (request, _) = host.command().await;
        host.draw(&hub, &request, b"second png", 320).await;
    };
    let (second, ()) = tokio::join!(second, answered);
    assert_eq!(second.unwrap().height, 320);
}

// The blobs and the rows ------------------------------------------------------------

/// Draws an image of a message at a width, with a session host that
/// answers at once.
async fn draw(hub: &Hub, host: &mut FakeHost, id: MessageId, width: u32) -> VisualImage {
    let asked = hub.visual_render(&Caller::Hub, render(id, width));
    let answered = async {
        let (request, _) = host.command().await;
        host.draw(hub, &request, format!("png {id} {width}").as_bytes(), 200)
            .await;
    };
    let (image, ()) = tokio::join!(asked, answered);
    image.unwrap()
}

async fn image_rows(hub: &Hub, id: MessageId) -> i64 {
    hub.store
        .read(move |conn| visuals::image_count(conn, id))
        .await
        .unwrap()
}

#[tokio::test]
async fn the_page_and_the_images_stay_in_the_store_while_the_message_exists() {
    // The hub has no clean-up of blobs. This test fails when one is added
    // that forgets the blobs of a visual.
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let id = show(&hub, &ctx).await;
    let mut host = FakeHost::online(&hub, 1, 10).await;
    let image = draw(&hub, &mut host, id, 720).await;
    let page = message(&hub, id).await.attachments[0].sha256.clone();
    for hash in [page, image.image.sha256] {
        assert!(hub.stored_blob_size(&hash).await.is_some());
    }
}

#[tokio::test]
async fn purging_a_visual_removes_its_cache_rows() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let kept = show_page(&hub, &ctx, "<p>Kept</p>").await;
    let purged = show_page(&hub, &ctx, "<p>Purged</p>").await;
    let mut host = FakeHost::online(&hub, 1, 10).await;
    for id in [kept, purged] {
        draw(&hub, &mut host, id, 720).await;
        draw(&hub, &mut host, id, 480).await;
    }
    assert_eq!(image_rows(&hub, purged).await, 2);
    let message = message(&hub, purged).await;
    hub.store
        .write(move |tx| tx.purge_message(&message))
        .await
        .unwrap();
    assert_eq!(image_rows(&hub, purged).await, 0);
    assert_eq!(image_rows(&hub, kept).await, 2, "other visuals keep theirs");
}

#[tokio::test]
async fn deleting_a_task_or_a_channel_removes_the_cache_rows_of_its_visuals() {
    let (hub, _dir) = test_hub().await;
    // A task: its thread holds a visual.
    let ctx = task_ctx(&hub).await;
    let in_thread = show_page(&hub, &ctx, "<p>In a thread</p>").await;
    // A channel: its timeline holds a visual.
    let channel = hub
        .channel_create(
            &Caller::Hub,
            ChannelCreate {
                name: "Charts".to_owned(),
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
            },
        )
        .await
        .unwrap()
        .channel;
    let mut channel_ctx = coordinator_ctx(task_ctx(&hub).await);
    channel_ctx.channel = channel.clone();
    let in_channel = show_page(&hub, &channel_ctx, "<p>In a channel</p>").await;
    let mut host = FakeHost::online(&hub, 1, 10).await;
    draw(&hub, &mut host, in_thread, 720).await;
    draw(&hub, &mut host, in_channel, 720).await;

    let task = ctx.task.clone().unwrap();
    hub.store
        .write(move |tx| tx.purge_task(&task))
        .await
        .unwrap();
    assert_eq!(image_rows(&hub, in_thread).await, 0);
    assert_eq!(image_rows(&hub, in_channel).await, 1);

    hub.channel_delete(
        &Caller::Hub,
        consort_proto::client_api::ChannelDelete {
            channel_id: channel.id,
        },
    )
    .await
    .unwrap();
    assert_eq!(image_rows(&hub, in_channel).await, 0);
}

#[tokio::test]
async fn a_message_that_goes_while_the_image_is_drawn_leaves_no_row() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let id = show(&hub, &ctx).await;
    let mut host = FakeHost::online(&hub, 1, 10).await;
    let asked = hub.visual_render(&Caller::Hub, render(id, 720));
    let answered = async {
        let (request, _) = host.command().await;
        let gone = message(&hub, id).await;
        hub.store
            .write(move |tx| tx.purge_message(&gone))
            .await
            .unwrap();
        host.draw(&hub, &request, b"late png", 300).await;
    };
    let (_, ()) = tokio::join!(asked, answered);
    assert_eq!(image_rows(&hub, id).await, 0);
}

#[tokio::test]
async fn a_visual_keeps_at_most_the_newest_images() {
    let (hub, _dir) = test_hub().await;
    let ctx = task_ctx(&hub).await;
    let id = show(&hub, &ctx).await;
    let sha = put_blob(&hub, b"png").await;
    let max = visuals::IMAGES_PER_MESSAGE_MAX;
    for n in 0..max + 5 {
        let sha = sha.clone();
        hub.store
            .write(move |tx| {
                tx.save_visual_image(
                    &format!("key-{n:03}"),
                    id,
                    &CachedImage {
                        image_sha256: sha,
                        size: 3,
                        height: 100,
                    },
                )
            })
            .await
            .unwrap();
    }
    assert_eq!(image_rows(&hub, id).await, max);
    let (oldest, newest) = hub
        .store
        .read(move |conn| {
            Ok((
                visuals::image(conn, "key-000")?,
                visuals::image(conn, &format!("key-{:03}", max + 4))?,
            ))
        })
        .await
        .unwrap();
    assert!(oldest.is_none(), "the oldest went");
    assert!(newest.is_some());
}
