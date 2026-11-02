//! `visual.render` from the hub: an image of a visual for one client.
//!
//! The session host downloads the prepared page from the hub, adds the
//! bootstrap with the style of the client, renders it, uploads the PNG, and
//! answers with a [`VisualImage`]. The plan is `docs/VISUALS-PLAN.md`,
//! "`visual.render` (hub to session host)".

use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::model::{
    BlobRef, Sha256Hash, VISUAL_FIT_MAX, VISUAL_PAGE_MAX_BYTES, VISUAL_RENDER_WIDTH_MAX,
    VISUAL_RENDER_WIDTH_MIN, VISUAL_SCALE_MAX, VisualImage,
};
use consort_proto::session_host_link::VisualRenderCommand;
use consort_proto::tools::PREVIEW_VISUAL_INSTALLING;
use consort_visual::Host;
use sha2::{Digest, Sha256};

use super::{RenderError, VisualEngine};
use crate::mcp::ToolBackend;

/// The file name of the image of a visual.
pub(crate) const IMAGE_NAME: &str = "visual.png";
/// The media type of the image of a visual.
pub(crate) const IMAGE_MIME: &str = "image/png";

/// The SHA-256 of a blob.
pub(crate) fn blob_hash(bytes: &[u8]) -> Sha256Hash {
    Sha256Hash::from_bytes(Sha256::digest(bytes).into())
}

/// The error body of a failed render, for the hub and the model.
///
/// - The browser installs: `unavailable`, with
///   [`PREVIEW_VISUAL_INSTALLING`].
/// - The render took too long: `unavailable`. The next try can work.
/// - Any other failure: `internal`, with what happened.
pub(crate) fn render_error(err: &RenderError) -> ErrorBody {
    match err {
        RenderError::Installing => {
            ErrorBody::new(ErrorCode::Unavailable, PREVIEW_VISUAL_INSTALLING)
        }
        RenderError::Timeout(_) => ErrorBody::new(ErrorCode::Unavailable, err.to_string()),
        other => ErrorBody::new(ErrorCode::Internal, other.to_string()),
    }
}

/// Checks the request before any download.
fn check(command: &VisualRenderCommand) -> Result<(), ErrorBody> {
    if !(VISUAL_RENDER_WIDTH_MIN..=VISUAL_RENDER_WIDTH_MAX).contains(&command.width) {
        return Err(ErrorBody::invalid(format!(
            "The width must be {VISUAL_RENDER_WIDTH_MIN} to {VISUAL_RENDER_WIDTH_MAX} pixels."
        )));
    }
    if !(1..=VISUAL_SCALE_MAX).contains(&command.scale) {
        return Err(ErrorBody::invalid(format!(
            "The scale must be 1 to {VISUAL_SCALE_MAX}."
        )));
    }
    command
        .style
        .check()
        .map_err(|e| ErrorBody::invalid(e.to_string()))
}

/// Answers one `visual.render`.
pub(crate) async fn render_command(
    engine: &dyn VisualEngine,
    blobs: &dyn ToolBackend,
    command: VisualRenderCommand,
) -> Result<VisualImage, ErrorBody> {
    check(&command)?;
    let bytes = blobs
        .download_blob(&command.page.sha256, VISUAL_PAGE_MAX_BYTES)
        .await?;
    if blob_hash(&bytes) != command.page.sha256 {
        return Err(ErrorBody::new(
            ErrorCode::Internal,
            "The page from the hub does not match its hash. Try again.",
        ));
    }
    let page = String::from_utf8(bytes).map_err(|_| {
        ErrorBody::invalid("The page of the visual is not UTF-8 text, so it cannot be drawn.")
    })?;
    let document = consort_visual::document(&page, &command.style, Host::Renderer)
        .map_err(|e| ErrorBody::invalid(e.to_string()))?;
    let rendered = engine
        .render(
            document,
            command.width,
            command.scale,
            command.style.mode,
            VISUAL_FIT_MAX,
        )
        .await
        .map_err(|err| {
            if err != RenderError::Installing {
                tracing::warn!(error = %err, width = command.width, scale = command.scale, "could not draw a visual for the hub");
            }
            render_error(&err)
        })?;
    let sha256 = blob_hash(&rendered.png);
    let size = rendered.png.len() as u64;
    blobs.upload_blob(&sha256, rendered.png).await?;
    Ok(VisualImage {
        image: BlobRef {
            sha256,
            name: IMAGE_NAME.to_owned(),
            mime: IMAGE_MIME.to_owned(),
            size,
        },
        height: rendered.height,
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use consort_proto::model::{CssVariable, VISUAL_PAGE_MIME, VisualMode, VisualStyle};

    use super::*;
    use crate::visual::testing::{EngineCall, FAKE_PNG, FakeEngine, MemoryBackend};

    fn command(backend: &MemoryBackend, page: &[u8]) -> VisualRenderCommand {
        let sha256 = backend.serve(page);
        VisualRenderCommand {
            page: BlobRef {
                sha256,
                name: "Tokens.html".to_owned(),
                mime: VISUAL_PAGE_MIME.to_owned(),
                size: page.len() as u64,
            },
            width: 640,
            scale: 2,
            style: VisualStyle {
                mode: VisualMode::Light,
                variables: vec![CssVariable {
                    name: "--background".to_owned(),
                    value: "#FFFFFF".to_owned(),
                }],
            },
        }
    }

    #[tokio::test]
    async fn visual_render_draws_the_page_with_the_style_and_uploads_the_image() {
        let engine = FakeEngine::new(312);
        let backend = MemoryBackend::default();
        let request = command(&backend, b"<p>Tokens by week</p>");
        let page_sha = request.page.sha256.clone();
        let image = render_command(&engine, &backend, request).await.unwrap();

        assert_eq!(image.height, 312);
        assert_eq!(image.image.name, "visual.png");
        assert_eq!(image.image.mime, "image/png");
        assert_eq!(image.image.size, FAKE_PNG.len() as u64);
        assert_eq!(image.image.sha256, blob_hash(FAKE_PNG));
        assert_eq!(backend.uploads(), vec![image.image.sha256.clone()]);
        assert_eq!(backend.blob(&image.image.sha256).unwrap(), FAKE_PNG);
        assert_eq!(*backend.downloads.lock().unwrap(), vec![page_sha]);

        let calls = engine.calls();
        let [
            EngineCall::Render {
                document,
                width,
                scale,
                mode,
                max_height,
            },
        ] = calls.as_slice()
        else {
            panic!("one render: {calls:?}");
        };
        assert_eq!((*width, *scale, *mode), (640, 2, VisualMode::Light));
        assert_eq!(*max_height, VISUAL_FIT_MAX);
        // The bootstrap has the style of the client, before the page.
        let theme = document.find("--background:#FFFFFF").unwrap();
        let page = document.find("<p>Tokens by week</p>").unwrap();
        assert!(theme < page);
        assert!(document.contains("color-scheme:light"));
        // The renderer measures from outside, so the page has no bootstrap
        // script of the webview.
        assert!(!document.contains("consort-visual-bootstrap"));
    }

    #[tokio::test]
    async fn visual_render_refuses_a_bad_request_before_any_download() {
        let engine = FakeEngine::new(100);
        let backend = MemoryBackend::default();
        let mut narrow = command(&backend, b"<p>x</p>");
        narrow.width = VISUAL_RENDER_WIDTH_MIN - 1;
        let mut wide = command(&backend, b"<p>x</p>");
        wide.width = VISUAL_RENDER_WIDTH_MAX + 1;
        let mut zero = command(&backend, b"<p>x</p>");
        zero.scale = 0;
        let mut big = command(&backend, b"<p>x</p>");
        big.scale = VISUAL_SCALE_MAX + 1;
        let mut style = command(&backend, b"<p>x</p>");
        style.style.variables[0].value = "red;}</style><script>".to_owned();
        for bad in [narrow, wide, zero, big, style] {
            let err = render_command(&engine, &backend, bad).await.unwrap_err();
            assert_eq!(err.code, ErrorCode::Invalid, "{err:?}");
        }
        assert!(backend.downloads.lock().unwrap().is_empty());
        assert!(engine.calls().is_empty());
        assert!(backend.uploads().is_empty());
    }

    #[tokio::test]
    async fn visual_render_refuses_a_page_that_is_not_text_or_not_the_blob() {
        let engine = FakeEngine::new(100);
        let backend = MemoryBackend::default();
        let binary = command(&backend, &[0xFF, 0xFE, 0x00, 0x80]);
        let err = render_command(&engine, &backend, binary).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Invalid);
        assert!(err.message.contains("UTF-8"), "{}", err.message);

        // A hub that sends other bytes for the hash.
        let mut wrong = command(&backend, b"<p>a</p>");
        backend
            .blobs
            .lock()
            .unwrap()
            .insert(wrong.page.sha256.to_string(), b"<p>b</p>".to_vec());
        wrong.page.size = 8;
        let err = render_command(&engine, &backend, wrong).await.unwrap_err();
        assert!(err.message.contains("hash"), "{}", err.message);
        assert!(engine.calls().is_empty());
    }

    #[tokio::test]
    async fn visual_render_maps_the_failures_of_the_browser() {
        for (failure, code) in [
            (RenderError::Installing, ErrorCode::Unavailable),
            (
                RenderError::Timeout(Duration::from_secs(20)),
                ErrorCode::Unavailable,
            ),
            (
                RenderError::MissingLibrary("libnss3.so".to_owned()),
                ErrorCode::Internal,
            ),
            (RenderError::RootWithoutSandbox, ErrorCode::Internal),
        ] {
            let engine = FakeEngine::failing(failure.clone());
            let backend = MemoryBackend::default();
            let request = command(&backend, b"<p>x</p>");
            let err = render_command(&engine, &backend, request)
                .await
                .unwrap_err();
            assert_eq!(err.code, code, "{failure:?}");
            assert_eq!(err.message, render_error(&failure).message);
            assert!(backend.uploads().is_empty());
        }
        assert_eq!(
            render_error(&RenderError::Installing).message,
            PREVIEW_VISUAL_INSTALLING
        );
        assert!(
            render_error(&RenderError::MissingLibrary("libnss3.so".to_owned()))
                .message
                .contains("libnss3.so")
        );
    }
}
