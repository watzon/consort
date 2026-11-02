//! Visuals in the client: the view of a visual in the timeline and in a task
//! thread, the file of its image, and the memory of the images that the
//! client asked for. The plan is `docs/VISUALS-PLAN.md`.
//!
//! A visual is a notice with `kind: "visual"` and one `text/html`
//! attachment, the page. The desktop loads the page live, or draws the image
//! that [`crate::Client::render_visual`] gives.

use std::collections::HashMap;

use consort_proto::model::{VISUAL_PAGE_MIME, VisualNotice, VisualStyle};
use tokio::sync::watch;

use crate::{ClientError, files::AttachmentView};

/// A visual in the timeline or in a task thread.
#[derive(Clone, Debug, PartialEq)]
pub struct VisualView {
    /// The message of the visual. `Client::render_visual` takes it.
    pub message_id: String,
    /// The title: the line above the page, and the name of the page file.
    pub title: String,
    /// The notice, for the height table: `notice.height_at(width)` is the
    /// height to lay out before the page or its image reports one.
    pub notice: VisualNotice,
    /// The page: a `text/html` attachment. Its `local_path` is set once the
    /// client has fetched the blob (`Client::fetch_blob`).
    pub page: AttachmentView,
    /// When the assistant showed it, such as "9:12 AM".
    pub time_label: String,
}

/// Reads a visual from the body and attachments of a notice. `None` when the
/// body is not a visual notice, or when the message has no page: the notice
/// then shows as a plain line with its text.
pub(crate) fn visual_view(
    message_id: &str,
    body: &str,
    time_label: &str,
    attachments: &[AttachmentView],
) -> Option<VisualView> {
    let notice = VisualNotice::parse(body)?;
    let page = attachments.iter().find(|a| is_page(&a.mime))?.clone();
    let title = if notice.title.trim().is_empty() {
        notice.text.clone()
    } else {
        notice.title.clone()
    };
    Some(VisualView {
        message_id: message_id.to_owned(),
        title,
        notice,
        page,
        time_label: time_label.to_owned(),
    })
}

/// Whether a media type is the page of a visual. A parameter such as
/// `; charset=utf-8` does not matter.
fn is_page(mime: &str) -> bool {
    mime.split(';')
        .next()
        .is_some_and(|m| m.trim().eq_ignore_ascii_case(VISUAL_PAGE_MIME))
}

/// The image of a visual, in the local blob cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisualImageFile {
    /// The PNG.
    pub path: String,
    /// The width of the page in CSS pixels.
    pub width: u32,
    /// The height of the page in CSS pixels. The PNG is `height * scale`
    /// pixels high.
    pub height: u32,
    /// Device pixels per CSS pixel. The PNG is `width * scale` pixels wide.
    pub scale: u8,
}

/// What a client asked for, to find the same answer again.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct VisualKey {
    pub message_id: String,
    pub width: u32,
    pub scale: u8,
    pub style: VisualStyle,
}

/// What a render gives to the asks that wait for it.
pub(crate) type Rendered = Option<Result<VisualImageFile, ClientError>>;

enum Slot {
    /// The image is in the local cache.
    Ready(VisualImageFile),
    /// A request runs. Its result comes on this channel.
    Rendering(watch::Receiver<Rendered>),
}

/// What an ask does next.
pub(crate) enum Turn {
    /// The image is here. No request.
    Ready(VisualImageFile),
    /// Another ask sent the request. Wait for its result.
    Wait(watch::Receiver<Rendered>),
    /// Send the request, then call [`VisualImages::finish`].
    Render(watch::Sender<Rendered>),
}

/// The most images that the client remembers.
const KEEP_MAX: usize = 200;

/// The images that the client asked for in this run. One request runs for
/// each key at a time. An error is never remembered, so the next ask tries
/// again.
#[derive(Default)]
pub(crate) struct VisualImages {
    slots: HashMap<VisualKey, Slot>,
}

impl VisualImages {
    /// Decides what an ask for `key` does. An image whose file is gone from
    /// the cache is not used: the ask renders again.
    pub(crate) fn begin(&mut self, key: &VisualKey) -> Turn {
        match self.slots.get(key) {
            Some(Slot::Ready(file)) if std::path::Path::new(&file.path).is_file() => {
                return Turn::Ready(file.clone());
            }
            // A sender that is gone without a result belongs to an ask that
            // was dropped. Nobody finishes that request.
            Some(Slot::Rendering(rx)) if rx.has_changed().is_ok() => {
                return Turn::Wait(rx.clone());
            }
            _ => {}
        }
        let (tx, rx) = watch::channel(None);
        self.slots.insert(key.clone(), Slot::Rendering(rx));
        Turn::Render(tx)
    }

    /// Ends the request of [`Turn::Render`]. A good image stays for the next
    /// ask. `keep` is false when the client has a new owner meanwhile.
    pub(crate) fn finish(
        &mut self,
        key: &VisualKey,
        tx: &watch::Sender<Rendered>,
        result: &Result<VisualImageFile, ClientError>,
        keep: bool,
    ) {
        match result {
            Ok(file) if keep => {
                // Only the images that are done count. The requests that
                // run stay, so their asks still find them.
                if self.slots.len() >= KEEP_MAX {
                    self.slots.retain(|_, s| matches!(s, Slot::Rendering(_)));
                }
                self.slots.insert(key.clone(), Slot::Ready(file.clone()));
            }
            _ => {
                self.slots.remove(key);
            }
        }
        tx.send_replace(Some(result.clone()));
    }

    /// Forgets the images that are done, for example when the client pairs
    /// with another hub.
    pub(crate) fn forget_ready(&mut self) {
        self.slots.retain(|_, s| matches!(s, Slot::Rendering(_)));
    }
}

/// The error of a render as the desktop sees it. The code `unavailable`
/// (no session host is online, or the browser for visuals is installing)
/// becomes [`ClientError::Unavailable`], so the app can try again later.
/// The code `worker_offline` means the same here, because the only machine
/// that a render needs is the session host. `not_found` stays a plain hub
/// error.
pub(crate) fn render_error(err: ClientError) -> ClientError {
    match err {
        ClientError::Hub { code, message } if code == "unavailable" || code == "worker_offline" => {
            let message = if message.trim().is_empty() {
                "Consort cannot draw this visual now. Try again in a minute.".to_owned()
            } else {
                message
            };
            ClientError::Unavailable { message }
        }
        other => other,
    }
}

/// Refuses a render request that the hub would refuse, with a message that
/// says what to do.
pub(crate) fn check_render(width: u32, scale: u8, style: &VisualStyle) -> Result<(), ClientError> {
    use consort_proto::model::{
        VISUAL_RENDER_WIDTH_MAX, VISUAL_RENDER_WIDTH_MIN, VISUAL_SCALE_MAX,
    };
    let invalid = |message: String| ClientError::Hub {
        code: "invalid".into(),
        message,
    };
    if !(VISUAL_RENDER_WIDTH_MIN..=VISUAL_RENDER_WIDTH_MAX).contains(&width) {
        return Err(invalid(format!(
            "A visual can be drawn from {VISUAL_RENDER_WIDTH_MIN} to {VISUAL_RENDER_WIDTH_MAX} pixels wide. Use a width in that range"
        )));
    }
    if !(1..=VISUAL_SCALE_MAX).contains(&scale) {
        return Err(invalid(format!(
            "A visual can be drawn at a scale from 1 to {VISUAL_SCALE_MAX}. Use a scale in that range"
        )));
    }
    style.check().map_err(|e| invalid(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::model::{CssVariable, VisualHeight, VisualMode};

    fn page(mime: &str) -> AttachmentView {
        AttachmentView {
            sha256: "a".repeat(64),
            name: "Tokens by week.html".into(),
            mime: mime.into(),
            size: 900,
            ..AttachmentView::default()
        }
    }

    fn body() -> String {
        VisualNotice::new(
            "Tokens by week",
            Some(300),
            vec![VisualHeight {
                width: 720,
                height: 340,
            }],
        )
        .to_body()
    }

    #[test]
    fn a_visual_notice_with_its_page_is_a_visual() {
        let view = visual_view("msg_1", &body(), "9:12 AM", &[page("text/html")]).unwrap();
        assert_eq!(view.message_id, "msg_1");
        assert_eq!(view.title, "Tokens by week");
        assert_eq!(view.page.name, "Tokens by week.html");
        assert_eq!(view.time_label, "9:12 AM");
        assert_eq!(view.notice.height_at(720), 340);
        assert_eq!(view.notice.height_at(640), 340);
    }

    #[test]
    fn a_visual_notice_with_no_page_is_a_plain_notice() {
        assert_eq!(visual_view("msg_1", &body(), "9:12 AM", &[]), None);
        let other = page("image/png");
        assert_eq!(visual_view("msg_1", &body(), "9:12 AM", &[other]), None);
    }

    #[test]
    fn another_notice_is_not_a_visual() {
        let draft = r#"{"kind":"draft","text":"Draft: Claim form"}"#;
        assert_eq!(
            visual_view("msg_1", draft, "9:12 AM", &[page("text/html")]),
            None
        );
        assert_eq!(
            visual_view("msg_1", "text", "9:12 AM", &[page("text/html")]),
            None
        );
    }

    #[test]
    fn the_page_may_name_a_charset() {
        assert!(is_page("text/html"));
        assert!(is_page("text/html; charset=utf-8"));
        assert!(is_page("Text/HTML"));
        assert!(!is_page("text/plain"));
        assert!(!is_page(""));
    }

    fn key(message: &str, width: u32) -> VisualKey {
        VisualKey {
            message_id: message.into(),
            width,
            scale: 2,
            style: VisualStyle::default(),
        }
    }

    fn file(path: &std::path::Path) -> VisualImageFile {
        VisualImageFile {
            path: path.display().to_string(),
            width: 720,
            height: 300,
            scale: 2,
        }
    }

    #[test]
    fn a_good_image_answers_the_next_ask_with_no_request() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("a.png");
        std::fs::write(&png, b"png").unwrap();
        let mut images = VisualImages::default();
        let k = key("msg_1", 720);
        let Turn::Render(tx) = images.begin(&k) else {
            panic!("the first ask renders");
        };
        images.finish(&k, &tx, &Ok(file(&png)), true);
        match images.begin(&k) {
            Turn::Ready(found) => assert_eq!(found, file(&png)),
            _ => panic!("the second ask finds the image"),
        }
        // Another width, scale or style is another image.
        assert!(matches!(images.begin(&key("msg_1", 640)), Turn::Render(_)));
        let mut dark = k.clone();
        dark.style.mode = VisualMode::Light;
        assert!(matches!(images.begin(&dark), Turn::Render(_)));
        let mut vars = k.clone();
        vars.style.variables = vec![CssVariable {
            name: "--chart-1".into(),
            value: "#77B4ED".into(),
        }];
        assert!(matches!(images.begin(&vars), Turn::Render(_)));
    }

    #[test]
    fn an_ask_while_a_request_runs_waits_for_it() {
        let mut images = VisualImages::default();
        let k = key("msg_1", 720);
        let Turn::Render(tx) = images.begin(&k) else {
            panic!("the first ask renders");
        };
        let Turn::Wait(rx) = images.begin(&k) else {
            panic!("the second ask waits");
        };
        let err = Err(ClientError::TimedOut);
        images.finish(&k, &tx, &err, true);
        assert_eq!(rx.borrow().clone(), Some(err));
        // An error is not kept: the next ask sends a new request.
        assert!(matches!(images.begin(&k), Turn::Render(_)));
    }

    #[test]
    fn an_image_whose_file_is_gone_is_rendered_again() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("a.png");
        std::fs::write(&png, b"png").unwrap();
        let mut images = VisualImages::default();
        let k = key("msg_1", 720);
        let Turn::Render(tx) = images.begin(&k) else {
            panic!("the first ask renders");
        };
        images.finish(&k, &tx, &Ok(file(&png)), true);
        std::fs::remove_file(&png).unwrap();
        assert!(matches!(images.begin(&k), Turn::Render(_)));
    }

    #[test]
    fn a_request_that_nobody_finishes_does_not_block_the_next_ask() {
        let mut images = VisualImages::default();
        let k = key("msg_1", 720);
        let Turn::Render(tx) = images.begin(&k) else {
            panic!("the first ask renders");
        };
        drop(tx);
        assert!(matches!(images.begin(&k), Turn::Render(_)));
    }

    #[test]
    fn a_new_owner_forgets_the_images_but_not_the_requests() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("a.png");
        std::fs::write(&png, b"png").unwrap();
        let mut images = VisualImages::default();
        let (done, running) = (key("msg_1", 720), key("msg_2", 720));
        let Turn::Render(tx) = images.begin(&done) else {
            panic!("renders");
        };
        images.finish(&done, &tx, &Ok(file(&png)), true);
        let Turn::Render(_pending) = images.begin(&running) else {
            panic!("renders");
        };
        images.forget_ready();
        assert!(matches!(images.begin(&done), Turn::Render(_)));
        assert!(matches!(images.begin(&running), Turn::Wait(_)));
    }

    #[test]
    fn the_codes_of_a_render_error_are_told_apart() {
        let hub = |code: &str, message: &str| ClientError::Hub {
            code: code.into(),
            message: message.into(),
        };
        assert_eq!(
            render_error(hub(
                "unavailable",
                "The browser is installing. Try again in a minute."
            )),
            ClientError::Unavailable {
                message: "The browser is installing. Try again in a minute.".into()
            }
        );
        assert_eq!(
            render_error(hub("unavailable", " ")),
            ClientError::Unavailable {
                message: "Consort cannot draw this visual now. Try again in a minute.".into()
            }
        );
        assert_eq!(
            render_error(hub("worker_offline", "The session host is offline.")),
            ClientError::Unavailable {
                message: "The session host is offline.".into()
            }
        );
        let missing = hub("not_found", "This visual is gone.");
        assert_eq!(render_error(missing.clone()), missing);
        assert_eq!(render_error(ClientError::TimedOut), ClientError::TimedOut);
    }

    #[test]
    fn a_render_request_out_of_range_is_refused_before_it_is_sent() {
        let style = VisualStyle::default();
        assert!(check_render(720, 2, &style).is_ok());
        assert!(check_render(160, 1, &style).is_ok());
        assert!(check_render(2_400, 3, &style).is_ok());
        for (width, scale) in [(159, 2), (2_401, 2), (720, 0), (720, 4)] {
            let err = check_render(width, scale, &style).unwrap_err();
            assert!(
                matches!(&err, ClientError::Hub { code, .. } if code == "invalid"),
                "{err:?}"
            );
        }
        let bad = VisualStyle {
            mode: VisualMode::Dark,
            variables: vec![CssVariable {
                name: "background".into(),
                value: "#fff".into(),
            }],
        };
        assert!(check_render(720, 2, &bad).is_err());
    }
}
