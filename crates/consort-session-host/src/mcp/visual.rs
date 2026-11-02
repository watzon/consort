//! The visual tools of a session. The plan is `docs/VISUALS-PLAN.md`.
//!
//! - `preview_visual`: the session host answers it. It renders the page and
//!   gives the model the PNG, the height and the console output. Nothing
//!   goes to the hub.
//! - `show_visual`: the session host prepares the page (the check and the
//!   local images), uploads it, measures its height table, and then sends
//!   the call to the hub with the page in place of the HTML.
//!
//! A local image must be inside the workspace of the session. A task
//! session reads it on its worker; a coordinator reads its own workspace
//! on this machine.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::execution_link::{ExecTool, WorkspaceInspection};
use consort_proto::model::{
    BlobRef, Os, Sha256Hash, VISUAL_IMAGE_MAX_BYTES, VISUAL_PAGE_MAX_BYTES, VISUAL_PAGE_MIME,
    VISUAL_PREVIEW_WIDTH_DEFAULT, VISUAL_PREVIEW_WIDTH_MAX, VISUAL_PREVIEW_WIDTH_MIN,
    VISUAL_TITLE_MAX_CHARS, VISUAL_WIDTHS, VisualMode,
};
use consort_proto::tools::{
    HubTool, PreviewVisualArgs, PreviewVisualResult, ShowVisualArgs, ToolCallResult, ToolContent,
};
use consort_visual::{Host, PageError};
use serde_json::{Value, json};

use super::HubToolsServer;
use crate::exec_tools::ExecTools;
use crate::visual::RenderError;
use crate::visual::command::{blob_hash, render_error};

/// The tallest image of a preview, in CSS pixels. A taller page shows its
/// top part. Model APIs refuse or shrink an image with a side of more than
/// 2000 pixels in a long conversation, and a preview is at scale 1 and at
/// most 1144 pixels wide.
pub const PREVIEW_IMAGE_MAX_HEIGHT: u32 = 2_000;

/// The root of the workspace of a task session on its worker, as
/// `workspace.open` gave it. A path from the page is in the form of the
/// worker, which may differ from this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRoot {
    root: String,
    windows: bool,
    case_insensitive: bool,
}

impl WorkerRoot {
    /// A root in the form of the worker.
    pub fn new(root: impl Into<String>, windows: bool, case_insensitive: bool) -> Self {
        Self {
            root: root.into(),
            windows,
            case_insensitive,
        }
    }

    /// The root of an inspected workspace.
    pub fn of(inspection: &WorkspaceInspection) -> Self {
        Self::new(
            inspection.native_root.clone(),
            inspection.os == Os::Windows || inspection.path_separator == "\\",
            inspection.case_insensitive_paths,
        )
    }

    /// The path relative to the root, with `/` between its parts, for an
    /// absolute path inside the root. `None` for a relative path, a path
    /// outside the root, the root itself, and a path with `..`.
    pub fn relative(&self, path: &str) -> Option<String> {
        if !self.is_absolute(path) {
            return None;
        }
        let root = self.parts(&self.root)?;
        let parts = self.parts(path)?;
        if parts.len() <= root.len() {
            return None;
        }
        let same = |a: &str, b: &str| {
            if self.case_insensitive {
                a.to_lowercase() == b.to_lowercase()
            } else {
                a == b
            }
        };
        if !root.iter().zip(&parts).all(|(a, b)| same(a, b)) {
            return None;
        }
        Some(parts[root.len()..].join("/"))
    }

    fn is_absolute(&self, path: &str) -> bool {
        if !self.windows {
            return path.starts_with('/');
        }
        let bytes = path.as_bytes();
        let drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/');
        drive || path.starts_with("\\\\") || path.starts_with("//")
    }

    /// The names in a path. `None` when one is `..`.
    fn parts<'a>(&self, path: &'a str) -> Option<Vec<&'a str>> {
        let separators: &[char] = if self.windows { &['/', '\\'] } else { &['/'] };
        let mut out = Vec::new();
        for part in path.split(separators) {
            match part {
                "" | "." => {}
                ".." => return None,
                name => out.push(name),
            }
        }
        Some(out)
    }
}

/// The error for a page that the page contract refuses.
fn page_error(err: PageError) -> ErrorBody {
    ErrorBody::invalid(err.to_string())
}

fn outside(path: &str) -> ErrorBody {
    ErrorBody::invalid(format!(
        "The image {path} is outside your workspace. Copy it into the workspace, then try again."
    ))
}

fn missing(path: &str) -> ErrorBody {
    ErrorBody::new(
        ErrorCode::NotFound,
        format!("The image {path} does not exist. Check the path, then try again."),
    )
}

/// The file of a local image in the workspace on this machine. The path
/// must be absolute, and it must stay in the workspace after symlinks.
pub(crate) fn local_image_file(workspace: &Path, given: &str) -> Result<PathBuf, ErrorBody> {
    let candidate = Path::new(given);
    let root = std::fs::canonicalize(workspace).map_err(|_| {
        ErrorBody::new(
            ErrorCode::Internal,
            "The workspace of this session is gone. Try again in a new conversation.",
        )
    })?;
    if !candidate.is_absolute()
        || !(candidate.starts_with(workspace) || candidate.starts_with(&root))
    {
        return Err(outside(given));
    }
    let real = std::fs::canonicalize(candidate).map_err(|_| missing(given))?;
    if !real.starts_with(&root) {
        return Err(outside(given));
    }
    if !real.is_file() {
        return Err(ErrorBody::invalid(format!(
            "The image {given} is not a file. Give the path of an image file, then try again."
        )));
    }
    Ok(real)
}

/// The message of a failed worker call: `message` of a worker error, or
/// `error.message` of an error of the hub.
fn worker_message(value: &Value) -> &str {
    value
        .get("message")
        .or_else(|| value.pointer("/error/message"))
        .and_then(Value::as_str)
        .unwrap_or("no reason was given")
}

impl HubToolsServer {
    /// Whether the hub lists the tool for this session. A native approval
    /// of the harness passes only for a listed tool, and a hub that does not
    /// list it does not post it.
    async fn require_listed(&self, tool: HubTool) -> Result<(), ErrorBody> {
        let name = tool.name();
        if self.tools().await?.iter().any(|t| t.name == name) {
            return Ok(());
        }
        Err(ErrorBody::new(
            ErrorCode::Unsupported,
            format!("This session has no {name} tool."),
        ))
    }

    /// `preview_visual`.
    pub(super) async fn preview_visual(&self, arguments: Value) -> ToolCallResult {
        match self.preview(arguments).await {
            Ok(result) => result,
            Err(err) => ToolCallResult::error(&err),
        }
    }

    async fn preview(&self, arguments: Value) -> Result<ToolCallResult, ErrorBody> {
        self.require_listed(HubTool::PreviewVisual).await?;
        let args: PreviewVisualArgs = serde_json::from_value(arguments).map_err(|e| {
            ErrorBody::invalid(format!(
                "The arguments of preview_visual are not valid: {e}"
            ))
        })?;
        let width = args.width.unwrap_or(VISUAL_PREVIEW_WIDTH_DEFAULT);
        if !(VISUAL_PREVIEW_WIDTH_MIN..=VISUAL_PREVIEW_WIDTH_MAX).contains(&width) {
            return Err(ErrorBody::invalid(format!(
                "The width must be {VISUAL_PREVIEW_WIDTH_MIN} to {VISUAL_PREVIEW_WIDTH_MAX} pixels. The conversation column is {VISUAL_PREVIEW_WIDTH_DEFAULT}."
            )));
        }
        let mode = args.appearance.unwrap_or_default();
        let page = self.prepare_page(&args.html).await?;
        let document =
            consort_visual::document(&page, &consort_visual::default_style(mode), Host::Renderer)
                .map_err(page_error)?;
        let rendered = self
            .visual
            .render(document, width, 1, mode, PREVIEW_IMAGE_MAX_HEIGHT)
            .await
            .map_err(|err| render_error(&err))?;
        let result = PreviewVisualResult {
            width,
            height: rendered.height,
            console: rendered.console,
            omitted: rendered.omitted,
        };
        let text = serde_json::to_string(&result).map_err(|_| ErrorBody::internal())?;
        let mut content = vec![
            ToolContent::Image {
                data: base64::engine::general_purpose::STANDARD.encode(&rendered.png),
                mime_type: "image/png".to_owned(),
            },
            ToolContent::Text { text },
        ];
        if rendered.height > PREVIEW_IMAGE_MAX_HEIGHT {
            content.push(ToolContent::Text {
                text: format!(
                    "The page is {} px tall. The image shows only the top {PREVIEW_IMAGE_MAX_HEIGHT} px.",
                    rendered.height
                ),
            });
        }
        // No structured content: a client that prefers it would not show
        // the image.
        Ok(ToolCallResult {
            content,
            is_error: false,
            structured_content: None,
        })
    }

    /// The arguments of `show_visual` for the hub: the page is checked,
    /// prepared and uploaded, and the call carries its blob and its height
    /// table in place of the HTML. Nothing goes to the hub when the page or
    /// an image is refused.
    pub(super) async fn show_visual_arguments(&self, arguments: Value) -> Result<Value, ErrorBody> {
        self.require_listed(HubTool::ShowVisual).await?;
        let mut args: ShowVisualArgs = serde_json::from_value(arguments).map_err(|e| {
            ErrorBody::invalid(format!("The arguments of show_visual are not valid: {e}"))
        })?;
        let title_chars = args.title.trim().chars().count();
        if title_chars == 0 || title_chars > VISUAL_TITLE_MAX_CHARS {
            return Err(ErrorBody::invalid(format!(
                "The title must have 1 to {VISUAL_TITLE_MAX_CHARS} characters."
            )));
        }
        let page = self.prepare_page(&args.html).await?;
        // The height table is for the default theme in dark mode. A client
        // theme changes colors, not sizes.
        let document = consort_visual::document(
            &page,
            &consort_visual::default_style(VisualMode::Dark),
            Host::Renderer,
        )
        .map_err(page_error)?;
        let bytes = page.into_bytes();
        let sha256 = blob_hash(&bytes);
        let size = bytes.len() as u64;
        let (uploaded, measured) = tokio::join!(
            self.backend.upload_blob(&sha256, bytes),
            self.visual
                .measure(document, &VISUAL_WIDTHS, VisualMode::Dark)
        );
        uploaded?;
        // The model does not wait for an install or a page that does not
        // end. The client measures the page itself when the table is
        // empty.
        let heights = match measured {
            Ok(heights) => Some(heights),
            Err(RenderError::Installing) => {
                tracing::info!(session = %self.session_id, "a visual has no height table: the preview browser installs");
                None
            }
            Err(err) => {
                tracing::warn!(session = %self.session_id, error = %err, "a visual has no height table");
                None
            }
        };
        args.page = Some(BlobRef {
            sha256,
            name: consort_visual::page_name(&args.title),
            mime: VISUAL_PAGE_MIME.to_owned(),
            size,
        });
        args.heights = heights;
        args.html = String::new();
        serde_json::to_value(&args).map_err(|_| ErrorBody::internal())
    }

    /// Checks the page and puts its local images in it.
    async fn prepare_page(&self, html: &str) -> Result<String, ErrorBody> {
        consort_visual::check_html(html).map_err(page_error)?;
        let images = consort_visual::local_images(html);
        if images.is_empty() {
            return Ok(html.to_owned());
        }
        let mut total = html.len() as u64;
        let mut files = Vec::with_capacity(images.len());
        for image in images {
            let bytes = self.read_image(&image.path).await?;
            if bytes.len() as u64 > VISUAL_IMAGE_MAX_BYTES {
                return Err(page_error(PageError::ImageTooLarge { path: image.path }));
            }
            // Stop before many large images fill the memory. The page
            // check below gives the exact size.
            total = total.saturating_add(bytes.len() as u64);
            if total > VISUAL_PAGE_MAX_BYTES {
                return Err(page_error(PageError::PageTooLarge { bytes: total }));
            }
            files.push((image, bytes));
        }
        consort_visual::inline_images(html, &files).map_err(page_error)
    }

    /// The bytes of a local image: on the worker for a task session, in
    /// the workspace on this machine for a coordinator.
    async fn read_image(&self, path: &str) -> Result<Vec<u8>, ErrorBody> {
        match (&self.exec, &self.worker_root) {
            (Some(exec), Some(root)) => self.read_worker_image(exec, root, path).await,
            (Some(_), None) => Err(ErrorBody::new(
                ErrorCode::Internal,
                "The session host does not know the workspace of this task yet. Try again.",
            )),
            (None, _) => {
                let file = local_image_file(&self.workspace, path)?;
                let size = tokio::fs::metadata(&file)
                    .await
                    .map(|m| m.len())
                    .map_err(|_| missing(path))?;
                if size > VISUAL_IMAGE_MAX_BYTES {
                    return Err(page_error(PageError::ImageTooLarge {
                        path: path.to_owned(),
                    }));
                }
                tokio::fs::read(&file).await.map_err(|e| {
                    ErrorBody::new(
                        ErrorCode::Internal,
                        format!("The image {path} could not be read: {e}. Try again."),
                    )
                })
            }
        }
    }

    /// Reads an image on the worker: the worker uploads it to the hub
    /// (`blob_put`), and the session host downloads it from there.
    async fn read_worker_image(
        &self,
        exec: &ExecTools,
        root: &WorkerRoot,
        path: &str,
    ) -> Result<Vec<u8>, ErrorBody> {
        let relative = root.relative(path).ok_or_else(|| outside(path))?;
        let result = exec
            .call(ExecTool::BlobPut, json!({ "path": relative }), None)
            .await;
        let value = result.structured_content.unwrap_or(Value::Null);
        if result.is_error {
            return Err(match value.get("error_code").and_then(Value::as_str) {
                Some("not_found") => missing(path),
                Some("path_refused") => ErrorBody::invalid(format!(
                    "The image {path} cannot be read: {}",
                    worker_message(&value)
                )),
                _ => ErrorBody::new(
                    ErrorCode::Internal,
                    format!(
                        "The worker could not read the image {path}: {}",
                        worker_message(&value)
                    ),
                ),
            });
        }
        let sha = value
            .get("sha256")
            .and_then(Value::as_str)
            .and_then(|hash| hash.parse::<Sha256Hash>().ok())
            .ok_or_else(|| {
                ErrorBody::new(
                    ErrorCode::Internal,
                    format!("The worker did not upload the image {path}. Try again."),
                )
            })?;
        if value
            .get("size")
            .and_then(Value::as_u64)
            .is_some_and(|size| size > VISUAL_IMAGE_MAX_BYTES)
        {
            return Err(page_error(PageError::ImageTooLarge {
                path: path.to_owned(),
            }));
        }
        let bytes = self
            .backend
            .download_blob(&sha, VISUAL_IMAGE_MAX_BYTES)
            .await?;
        if blob_hash(&bytes) != sha {
            return Err(ErrorBody::new(
                ErrorCode::Internal,
                format!("The image {path} from the hub does not match its hash. Try again."),
            ));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests;
