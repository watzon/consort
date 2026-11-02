//! The mock's visuals: two pages in the timeline of mac-ext4, a bar chart
//! and a line chart that the assistant drew after a question about the
//! speed of the driver.
//!
//! Each visual has its page (HTML with inline SVG that uses the theme
//! variables of `docs/VISUALS-PLAN.md`), a height table, and one image. The
//! images are screenshots of the pages at 720 px wide, scale 2, in Spruce
//! dark, from the real headless browser. The mock writes the page and the
//! image to a folder in the temp folder, so the page has a file to load and
//! the mock render has a file to answer with. The mock render always gives
//! the image at 720 px wide and scale 2, in dark mode.

use std::path::PathBuf;

use consort_proto::model::{BlobRef, VISUAL_NOTICE, VISUAL_PAGE_MIME, VisualHeight, VisualNotice};

use crate::{
    ClientError, CommandOutput,
    blob::BlobTransfer,
    files::attachment_view,
    view::{MessageView, NoticeView, Speaker, TimelineItem, ViewState},
    visual::{VisualImageFile, VisualView},
};

/// The channel of the mock visuals: mac-ext4.
pub const CHANNEL: &str = "chn_mac_ext4";
/// The message of the bar chart.
pub const READ_SPEED: &str = "msg_visual_read_speed";
/// The message of the line chart.
pub const COPY_SPEED: &str = "msg_visual_copy_speed";

/// The width and the scale of the images.
const IMAGE_WIDTH: u32 = 720;
const IMAGE_SCALE: u8 = 2;

/// One mock visual.
struct MockVisual {
    message_id: &'static str,
    title: &'static str,
    time: &'static str,
    html: &'static str,
    png: &'static [u8],
    /// The height of the page at each width of `VISUAL_WIDTHS`, measured in
    /// the headless browser.
    heights: [u32; 10],
}

const VISUALS: [MockVisual; 2] = [
    MockVisual {
        message_id: READ_SPEED,
        title: "Read speed on a 10 GB file",
        time: "9:40 AM",
        html: include_str!("../fixtures/visuals/read-speed.html"),
        png: include_bytes!("../fixtures/visuals/read-speed.png"),
        heights: [134, 154, 174, 192, 221, 250, 264, 264, 264, 264],
    },
    MockVisual {
        message_id: COPY_SPEED,
        title: "Copy speed during a 60 GB copy",
        time: "9:44 AM",
        html: include_str!("../fixtures/visuals/copy-speed.html"),
        png: include_bytes!("../fixtures/visuals/copy-speed.png"),
        heights: [132, 152, 171, 189, 218, 246, 260, 260, 260, 260],
    },
];

impl MockVisual {
    fn notice(&self) -> VisualNotice {
        let heights = consort_proto::model::VISUAL_WIDTHS
            .iter()
            .zip(self.heights)
            .map(|(&width, height)| VisualHeight { width, height })
            .collect();
        // The height that the assistant gave is the one at the full width.
        VisualNotice::new(self.title, Some(self.heights[6]), heights)
    }

    /// The height of the image, in CSS pixels.
    fn image_height(&self) -> u32 {
        self.heights[6]
    }
}

/// The folder where the mock keeps its pages and images.
fn folder() -> PathBuf {
    crate::mock_files::folder().join("visuals")
}

/// Writes bytes to the folder under their hash and gives the path. `None`
/// when the temp folder cannot be written. The hash in the name means that a
/// file of an older build is never used. The file is complete when it has
/// its name, so a second process never reads half of it.
fn write(bytes: &[u8], extension: &str) -> Option<PathBuf> {
    let dir = folder();
    let path = dir.join(format!("{}.{extension}", BlobTransfer::hash_bytes(bytes)));
    if !path.exists() {
        std::fs::create_dir_all(&dir).ok()?;
        static PARTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let part = PARTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let partial =
            path.with_extension(format!("{extension}.{}.{part}.part", std::process::id()));
        std::fs::write(&partial, bytes).ok()?;
        std::fs::rename(&partial, &path).ok()?;
    }
    Some(path)
}

fn view_of(v: &MockVisual) -> VisualView {
    let blob = BlobRef {
        sha256: BlobTransfer::hash_bytes(v.html.as_bytes()),
        name: format!("{}.html", v.title),
        mime: VISUAL_PAGE_MIME.to_owned(),
        size: v.html.len() as u64,
    };
    let local = write(v.html.as_bytes(), "html").map(|p| p.display().to_string());
    VisualView {
        message_id: v.message_id.to_owned(),
        title: v.title.to_owned(),
        notice: v.notice(),
        page: attachment_view(&blob, None, local),
        time_label: v.time.to_owned(),
    }
}

fn visual_item(v: &MockVisual) -> TimelineItem {
    TimelineItem::Notice(NoticeView {
        message_id: v.message_id.to_owned(),
        time_label: v.time.to_owned(),
        created_at: 0,
        kind: VISUAL_NOTICE.to_owned(),
        text: v.title.to_owned(),
        visual: Some(view_of(v)),
        ..NoticeView::default()
    })
}

fn message(id: &str, speaker: Speaker, time: &str, body: &str) -> TimelineItem {
    TimelineItem::Message(MessageView {
        id: id.to_owned(),
        channel_id: CHANNEL.to_owned(),
        speaker,
        time_label: time.to_owned(),
        body: body.to_owned(),
        ..MessageView::default()
    })
}

/// The items that the mock adds to mac-ext4: two questions, two visuals and
/// the short answers of the assistant. It does not describe the visual: the
/// real `show_visual` tells the assistant not to.
pub fn items() -> Vec<TimelineItem> {
    let [read, copy] = &VISUALS;
    vec![
        message(
            "msg_ext4_speed_you",
            Speaker::You,
            "9:40 AM",
            "While I get the certificate, how fast is last week's build of the driver?",
        ),
        visual_item(read),
        message(
            "msg_ext4_speed_reply",
            Speaker::Consort,
            "9:41 AM",
            "On a 10 GB file, last week's build reads at 1,810 MB/s. That is 3.4 times the speed of the FUSE driver, and about 70 percent of the speed of an APFS volume.",
        ),
        message(
            "msg_ext4_copy_you",
            Speaker::You,
            "9:44 AM",
            "Does it keep that speed for a long copy?",
        ),
        visual_item(copy),
        message(
            "msg_ext4_copy_reply",
            Speaker::Consort,
            "9:44 AM",
            "Not quite. It copies at about 1,800 MB/s for 17 seconds, then drops to 1,100 MB/s. The write cache of the drive is full by then, so the drive is the limit, not the driver.",
        ),
    ]
}

/// Adds the mock visuals to the timeline of mac-ext4.
pub fn decorate(state: &mut ViewState) {
    if let Some(timeline) = state.timelines.iter_mut().find(|t| t.channel_id == CHANNEL) {
        timeline.items.extend(items());
    }
}

/// The answer of the mock to `Command::RenderVisual`: the image of the
/// visual, always at 720 px wide and scale 2, in dark mode. A message that is
/// not a mock visual is `not_found`, as the hub says it.
pub fn render(message_id: &str) -> Result<CommandOutput, ClientError> {
    let not_found = || ClientError::Hub {
        code: "not_found".into(),
        message: "This visual is gone. It may have been deleted in another client.".into(),
    };
    let visual = VISUALS
        .iter()
        .find(|v| v.message_id == message_id)
        .ok_or_else(not_found)?;
    let path = write(visual.png, "png").ok_or_else(|| ClientError::Storage {
        path: folder().display().to_string(),
        message: "The folder for the demo images cannot be written".into(),
    })?;
    Ok(CommandOutput::VisualImage(VisualImageFile {
        path: path.display().to_string(),
        width: IMAGE_WIDTH,
        height: visual.image_height(),
        scale: IMAGE_SCALE,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::model::VISUAL_WIDTHS;

    /// The size in the header of a PNG file.
    fn png_size(png: &[u8]) -> (u32, u32) {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "a PNG file");
        let word = |at: usize| u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
        (word(16), word(20))
    }

    #[test]
    fn each_image_matches_its_height_table() {
        for v in &VISUALS {
            let (width, height) = png_size(v.png);
            assert_eq!(width, IMAGE_WIDTH * u32::from(IMAGE_SCALE), "{}", v.title);
            assert_eq!(
                height,
                v.image_height() * u32::from(IMAGE_SCALE),
                "{}",
                v.title
            );
            assert_eq!(VISUAL_WIDTHS[6], IMAGE_WIDTH);
            let notice = v.notice();
            assert_eq!(notice.heights.len(), VISUAL_WIDTHS.len());
            assert_eq!(notice.table_height(IMAGE_WIDTH), Some(v.image_height()));
            // A narrow page is as tall as a wide one, or taller.
            assert!(
                notice
                    .heights
                    .windows(2)
                    .all(|w| w[0].height <= w[1].height)
            );
        }
    }

    #[test]
    fn each_page_uses_the_theme_variables_and_no_bootstrap() {
        for v in &VISUALS {
            assert!(v.html.contains("var(--chart-1)"), "{}", v.title);
            assert!(v.html.contains("var(--foreground)"), "{}", v.title);
            // The Paper names and the theme values are not in the page.
            assert!(!v.html.contains("--color-"), "{}", v.title);
            assert!(!v.html.contains('#'), "{}", v.title);
            assert!(!v.html.contains("<script"), "{}", v.title);
        }
    }

    #[test]
    fn the_timeline_of_mac_ext4_has_two_visuals_with_their_pages() {
        let state = crate::mock::view_state();
        let timeline = state.timeline(CHANNEL).expect("the timeline");
        let visuals: Vec<&VisualView> = timeline
            .items
            .iter()
            .filter_map(|i| match i {
                TimelineItem::Notice(n) => n.visual.as_ref(),
                _ => None,
            })
            .collect();
        assert_eq!(visuals.len(), 2);
        for view in visuals {
            assert_eq!(view.page.mime, "text/html");
            let path = view.page.local_path.as_deref().expect("a page file");
            let text = std::fs::read_to_string(path).expect("readable");
            assert!(text.contains("<svg"));
            assert_eq!(view.notice.title, view.title);
        }
        // The notices have a time, so the timeline can place its separators.
        assert!(timeline.items.iter().all(|i| match i {
            TimelineItem::Notice(n) => n.created_at > 0,
            _ => true,
        }));
    }

    #[test]
    fn the_mock_backend_answers_render_visual() {
        let client = crate::Client::mock().expect("mock");
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(async {
                let style = consort_proto::model::VisualStyle::default();
                let file = client
                    .render_visual(COPY_SPEED.into(), 640, 2, style.clone())
                    .await
                    .expect("an image");
                assert_eq!((file.width, file.height, file.scale), (720, 260, 2));
                assert!(std::path::Path::new(&file.path).is_file());
                // The client refuses a request out of range, as it does with a hub.
                assert!(
                    client
                        .render_visual(COPY_SPEED.into(), 10, 2, style.clone())
                        .await
                        .is_err()
                );
                let err = client
                    .render_visual("msg_other".into(), 720, 2, style)
                    .await
                    .unwrap_err();
                assert!(matches!(&err, ClientError::Hub { code, .. } if code == "not_found"));
            });
    }

    #[test]
    fn the_mock_render_answers_with_the_image_and_refuses_another_message() {
        let Ok(CommandOutput::VisualImage(file)) = render(READ_SPEED) else {
            panic!("an image");
        };
        assert_eq!((file.width, file.height, file.scale), (720, 264, 2));
        let bytes = std::fs::read(&file.path).expect("the image file");
        assert_eq!(png_size(&bytes), (1440, 528));
        let err = render("msg_other").unwrap_err();
        assert!(matches!(&err, ClientError::Hub { code, .. } if code == "not_found"));
    }
}
