//! `visual.render` against the fake hub: the image file, the memory of the
//! images, one request for two asks at once, the errors, and the long wait.
//! `fake_hub.rs` sends this request type here.

use super::*;
use consort_proto::model::{BlobRef, CssVariable, VisualImage, VisualMode, VisualStyle};

/// The visual state of the fake hub.
#[derive(Default)]
pub struct State {
    /// `visual.render` answers with this error.
    pub error: Option<ErrorBody>,
    /// While set, `visual.render` waits until this fires.
    pub gate: Option<Arc<tokio::sync::Notify>>,
}

/// The request types that this part answers.
pub fn handles(kind: &str) -> bool {
    kind == "visual.render"
}

/// The height that the fake hub gives for every image.
const HEIGHT: u32 = 312;

/// The bytes of the image for one request. The width, the scale and the mode
/// are in them, so another ask is another blob.
fn png_bytes(body: &api::VisualRender) -> Vec<u8> {
    format!(
        "fake png of {} at {} px, scale {}, {}",
        body.message_id,
        body.width,
        body.scale,
        body.style.mode.as_str()
    )
    .into_bytes()
}

/// Answers one request.
pub async fn answer(hub: &Shared, req: &RequestFrame) -> Frame {
    let body: api::VisualRender = req.decode_as().unwrap();
    let gate = {
        let mut h = hub.lock().unwrap();
        h.recorded.push((req.kind.clone(), req.body.clone()));
        h.visual.gate.clone()
    };
    if let Some(gate) = gate {
        gate.notified().await;
    }
    let mut h = hub.lock().unwrap();
    if let Some(error) = h.visual.error.clone() {
        return req.respond_error(error);
    }
    let png = png_bytes(&body);
    let sha = sha_of(&png);
    let size = png.len() as u64;
    h.blobs.insert(sha.clone(), png);
    req.respond_ok(&VisualImage {
        image: BlobRef {
            sha256: sha.parse().unwrap(),
            name: "visual.png".into(),
            mime: "image/png".into(),
            size,
        },
        height: HEIGHT,
    })
    .unwrap()
}

fn style(mode: VisualMode) -> VisualStyle {
    VisualStyle {
        mode,
        variables: vec![CssVariable {
            name: "--background".into(),
            value: "#131E1A".into(),
        }],
    }
}

fn renders(hub: &Shared) -> Vec<serde_json::Value> {
    recorded_of(hub, "visual.render")
}

fn gets(hub: &Shared) -> usize {
    blob_log(hub).iter().filter(|(m, _)| m == "GET").count()
}

/// The client sends `visual.render`, downloads the PNG that the hub names,
/// and gives its path and the height. The same ask again sends nothing. A
/// width, a scale or a mode that differs is another image.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn render_visual_gives_the_image_file_and_asks_once() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let message_id = MessageId::new().to_string();

    let file = client
        .render_visual(message_id.clone(), 720, 2, style(VisualMode::Dark))
        .await
        .unwrap();
    assert_eq!((file.width, file.height, file.scale), (720, HEIGHT, 2));
    let bytes = std::fs::read(&file.path).unwrap();
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        format!("fake png of {message_id} at 720 px, scale 2, dark")
    );
    let sent = renders(&hub);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["message_id"], message_id.as_str());
    assert_eq!(sent[0]["width"], 720);
    assert_eq!(sent[0]["scale"], 2);
    assert_eq!(sent[0]["style"]["mode"], "dark");
    assert_eq!(sent[0]["style"]["variables"][0]["name"], "--background");
    assert_eq!(gets(&hub), 1);

    // The same ask: the same file, no request and no download.
    let again = client
        .render_visual(message_id.clone(), 720, 2, style(VisualMode::Dark))
        .await
        .unwrap();
    assert_eq!(again, file);
    assert_eq!(renders(&hub).len(), 1);
    assert_eq!(gets(&hub), 1);

    // Another width, another scale and another mode are other images.
    for (width, scale, mode) in [
        (640, 2, VisualMode::Dark),
        (720, 1, VisualMode::Dark),
        (720, 2, VisualMode::Light),
    ] {
        let other = client
            .render_visual(message_id.clone(), width, scale, style(mode))
            .await
            .unwrap();
        assert_ne!(other.path, file.path);
        assert_eq!((other.width, other.scale), (width, scale));
    }
    assert_eq!(renders(&hub).len(), 4);
    // Another message is another image too.
    client
        .render_visual(
            MessageId::new().to_string(),
            720,
            2,
            style(VisualMode::Dark),
        )
        .await
        .unwrap();
    assert_eq!(renders(&hub).len(), 5);
}

/// A second ask for an image that the hub is still drawing waits for the
/// first request. It sends no request of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_asks_at_once_share_one_request() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let gate = Arc::new(tokio::sync::Notify::new());
    hub.lock().unwrap().visual.gate = Some(gate.clone());
    let message_id = MessageId::new().to_string();
    let ask = |client: &Client| {
        let (client, id) = (client.clone(), message_id.clone());
        tokio::spawn(async move {
            client
                .render_visual(id, 720, 2, style(VisualMode::Dark))
                .await
        })
    };
    let first = ask(&client);
    until("the hub holds the request", || renders(&hub).len() == 1).await;
    let second = ask(&client);
    // Give the second ask time to send its own request, if it would.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(renders(&hub).len(), 1, "one request for two asks");
    assert!(!first.is_finished() && !second.is_finished());
    gate.notify_waiters();
    let (first, second) = (
        first.await.unwrap().unwrap(),
        second.await.unwrap().unwrap(),
    );
    assert_eq!(first, second);
    assert_eq!(renders(&hub).len(), 1);
    assert_eq!(gets(&hub), 1);
}

/// The hub that has no session host online says so. The client gives an
/// error that the app tells apart, and keeps nothing: the next ask tries
/// again. A visual that is gone is a plain hub error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unavailable_hub_is_told_apart_and_not_remembered() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let message_id = MessageId::new().to_string();
    let ask = || client.render_visual(message_id.clone(), 720, 2, style(VisualMode::Dark));

    hub.lock().unwrap().visual.error = Some(ErrorBody::new(
        ErrorCode::WorkerOffline,
        "The session host is offline. Try again in a minute.",
    ));
    assert_eq!(
        ask().await.unwrap_err(),
        ClientError::Unavailable {
            message: "The session host is offline. Try again in a minute.".into()
        }
    );
    hub.lock().unwrap().visual.error = Some(ErrorBody::new(
        ErrorCode::NotFound,
        "This visual is gone. It may have been deleted in another client.",
    ));
    assert_eq!(
        ask().await.unwrap_err(),
        ClientError::Hub {
            code: "not_found".into(),
            message: "This visual is gone. It may have been deleted in another client.".into()
        }
    );
    assert_eq!(renders(&hub).len(), 2, "an error is not remembered");

    hub.lock().unwrap().visual.error = None;
    let file = ask().await.unwrap();
    assert_eq!(file.height, HEIGHT);
    assert_eq!(renders(&hub).len(), 3);
}

/// The client refuses a width, a scale or a style that the hub would
/// refuse, and sends nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_out_of_range_is_refused_before_it_is_sent() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let id = MessageId::new().to_string();
    for (width, scale) in [(100, 2), (3_000, 2), (720, 0), (720, 4)] {
        let err = client
            .render_visual(id.clone(), width, scale, style(VisualMode::Dark))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ClientError::Hub { code, .. } if code == "invalid"),
            "{err:?}"
        );
    }
    let bad = VisualStyle {
        mode: VisualMode::Dark,
        variables: vec![CssVariable {
            name: "--background".into(),
            value: "red;}body{display:none".into(),
        }],
    };
    assert!(client.render_visual(id, 720, 2, bad).await.is_err());
    assert!(renders(&hub).is_empty());
}

/// The hub may need a minute to draw a visual. The link waits for that
/// answer longer than it waits for any other request, and stays up.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_render_does_not_end_the_link() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    // Any other request that waits 1.2 s ends the link after 600 ms. The
    // fake hub reads no ping while it holds the request, so the idle limit
    // is long.
    let patient = Timeouts {
        idle: Duration::from_secs(30),
        ..quick()
    };
    let client = paired_client(&url, dir.path(), options(patient)).await;
    let gate = Arc::new(tokio::sync::Notify::new());
    hub.lock().unwrap().visual.gate = Some(gate.clone());
    let sessions = hub.lock().unwrap().sessions.len();
    let asking = {
        let client = client.clone();
        let id = MessageId::new().to_string();
        tokio::spawn(async move {
            client
                .render_visual(id, 720, 2, style(VisualMode::Dark))
                .await
        })
    };
    until("the hub holds the request", || renders(&hub).len() == 1).await;
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    gate.notify_waiters();
    let file = asking.await.unwrap().unwrap();
    assert_eq!(file.height, HEIGHT);
    assert_eq!(
        hub.lock().unwrap().sessions.len(),
        sessions,
        "the client did not reconnect"
    );
    assert_eq!(client.state().connection, Connection::Online);
}
