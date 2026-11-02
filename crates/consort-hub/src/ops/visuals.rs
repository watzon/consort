//! Visuals: the `show_visual` hub tool and the `visual.render` client
//! request. The plan is `docs/VISUALS-PLAN.md`.
//!
//! A visual is a notice with `kind: "visual"` and one attachment, the
//! prepared page. The session host prepares the page before it calls
//! `show_visual`, so the hub never reads the HTML. `visual.render` gives a
//! client an image of the page. The hub keeps each image in a cache, and
//! asks the session host only on a miss.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use consort_proto::client_api::VisualRender;
use consort_proto::error::{ErrorBody, ErrorCode};
use consort_proto::frame::{Frame, FrameId};
use consort_proto::ids::MessageId;
use consort_proto::model::{
    Author, BlobRef, MessageKind, Sha256Hash, VISUAL_BOOTSTRAP_VERSION, VISUAL_FIT_MAX,
    VISUAL_PAGE_MAX_BYTES, VISUAL_PAGE_MIME, VISUAL_RENDER_WIDTH_MAX, VISUAL_RENDER_WIDTH_MIN,
    VISUAL_SCALE_MAX, VISUAL_TITLE_MAX_CHARS, VisualHeight, VisualImage, VisualNotice,
};
use consort_proto::session_host_link::{HubToSessionHost, VisualRenderCommand};
use consort_proto::tools::{SHOW_VISUAL_SHOWN, ShowVisualArgs, ShowVisualResult};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub, ToolCtx};
use crate::link::{CallError, WorkerLink};
use crate::store::conversation::{self, NewMessage};
use crate::store::machine;
use crate::store::visuals::{self, CachedImage};

/// How long the hub waits for the session host to draw one image.
const RENDER_TIMEOUT: Duration = Duration::from_secs(60);
/// The most rows in the height table of a visual.
const HEIGHTS_MAX: usize = 32;
/// The name and the media type of a cached image.
const IMAGE_NAME: &str = "visual.png";
const IMAGE_MIME: &str = "image/png";
/// The first line of the text that the cache key hashes, before the
/// bootstrap version. A new bootstrap draws new images.
const KEY_TAG: &str = "consort-visual-image";

/// What the assistant reads when the session host did not prepare the page.
const SESSION_HOST_TOO_OLD: &str = "The session host on the hub machine is too old to prepare a visual. Update the session host, then call show_visual again.";

/// What the assistant reads when `preview_visual` reaches the hub. The
/// session host answers this tool itself.
const PREVIEW_AT_HUB: &str = "The session host on the hub machine did not answer preview_visual itself, so it is too old for visuals. Update the session host, then try again.";

/// The error when the session host could not draw the image in time:
/// `unavailable`, so the client tries again later.
fn unavailable(message: impl Into<String>) -> HubError {
    HubError::new(ErrorCode::Unavailable, message)
}

/// The error when no session host is online: `worker_offline`, as for the
/// account requests.
fn no_session_host() -> HubError {
    HubError::new(
        ErrorCode::WorkerOffline,
        "No session host is online to draw this visual. Try again when the hub is online.",
    )
}

/// The error of `preview_visual` at the hub. The session host answers this
/// tool itself, so a call that arrives here comes from an old session host.
pub(crate) fn preview_at_hub() -> HubError {
    HubError::new(ErrorCode::Unsupported, PREVIEW_AT_HUB)
}

fn not_a_visual() -> HubError {
    HubError::not_found("This message is not a visual.")
}

/// One render that is running or has just ended. The callers that wait for
/// it get the same answer.
type Flight = Arc<OnceCell<std::result::Result<VisualImage, ErrorBody>>>;

/// The renders that are running now, by cache key. Two requests for one key
/// share one render.
#[derive(Default)]
pub struct VisualRenders {
    flights: Mutex<HashMap<String, Flight>>,
    /// A shorter wait for the tests.
    #[cfg(test)]
    pub(crate) timeout: Mutex<Option<Duration>>,
}

impl VisualRenders {
    /// The flight for a key. The first caller makes it, and the callers
    /// that come while it runs get the same one.
    fn flight(&self, key: &str) -> Flight {
        let Ok(mut flights) = self.flights.lock() else {
            return Arc::default();
        };
        Arc::clone(flights.entry(key.to_owned()).or_default())
    }

    /// Forgets a flight that has ended, unless a newer one took its key.
    fn land(&self, key: &str, flight: &Flight) {
        if let Ok(mut flights) = self.flights.lock()
            && flights.get(key).is_some_and(|f| Arc::ptr_eq(f, flight))
        {
            flights.remove(key);
        }
    }

    fn render_timeout(&self) -> Duration {
        #[cfg(test)]
        if let Some(timeout) = self.timeout.lock().ok().and_then(|t| *t) {
            return timeout;
        }
        RENDER_TIMEOUT
    }
}

fn check_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > VISUAL_TITLE_MAX_CHARS {
        return Err(HubError::invalid(format!(
            "The title must have 1 to {VISUAL_TITLE_MAX_CHARS} characters."
        )));
    }
    Ok(title.to_owned())
}

fn check_heights(heights: &[VisualHeight]) -> Result<()> {
    if heights.len() > HEIGHTS_MAX {
        return Err(HubError::invalid(format!(
            "The height table has {} rows, and the limit is {HEIGHTS_MAX}.",
            heights.len()
        )));
    }
    if heights
        .iter()
        .any(|h| !(VISUAL_RENDER_WIDTH_MIN..=VISUAL_RENDER_WIDTH_MAX).contains(&h.width))
    {
        return Err(HubError::invalid(format!(
            "A width in the height table must be {VISUAL_RENDER_WIDTH_MIN} to {VISUAL_RENDER_WIDTH_MAX} pixels."
        )));
    }
    Ok(())
}

fn check_render(request: &VisualRender) -> Result<()> {
    if !(VISUAL_RENDER_WIDTH_MIN..=VISUAL_RENDER_WIDTH_MAX).contains(&request.width) {
        return Err(HubError::invalid(format!(
            "The width must be {VISUAL_RENDER_WIDTH_MIN} to {VISUAL_RENDER_WIDTH_MAX} pixels."
        )));
    }
    if !(1..=VISUAL_SCALE_MAX).contains(&request.scale) {
        return Err(HubError::invalid(format!(
            "The scale must be 1 to {VISUAL_SCALE_MAX}."
        )));
    }
    request
        .style
        .check()
        .map_err(|err| HubError::invalid(err.to_string()))
}

/// The key of the cache: the SHA-256 of a text that names the page, the
/// width, the scale and the style. The variables stay in the order that the
/// client gave. A value cannot hold a line break (the check refuses control
/// characters), so two different styles never give the same text.
fn image_key(page: &Sha256Hash, request: &VisualRender) -> String {
    let mut text = format!(
        "{KEY_TAG}\nbootstrap={VISUAL_BOOTSTRAP_VERSION}\npage={page}\nwidth={}\nscale={}\nmode={}\n",
        request.width,
        request.scale,
        request.style.mode.as_str()
    );
    for variable in &request.style.variables {
        text.push_str(&variable.name);
        text.push('=');
        text.push_str(&variable.value);
        text.push('\n');
    }
    Sha256Hash::from_bytes(Sha256::digest(text.as_bytes()).into()).to_string()
}

impl Hub {
    /// The size of a blob that the hub holds, from the file. `None` when
    /// the hub has no such blob.
    async fn stored_blob_size(&self, hash: &Sha256Hash) -> Option<u64> {
        tokio::fs::metadata(self.store.blobs_dir().join(hash.as_str()))
            .await
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len())
    }

    /// `show_visual`: posts a visual. The session host has prepared the
    /// page and uploaded it. A coordinator posts in its channel under the
    /// rules of its turn, and a task session in the thread of its task.
    pub(crate) async fn tool_show_visual(
        &self,
        ctx: &ToolCtx,
        args: ShowVisualArgs,
    ) -> Result<Value> {
        let title = check_title(&args.title)?;
        let mut page = args
            .page
            .ok_or_else(|| HubError::new(ErrorCode::Unsupported, SESSION_HOST_TOO_OLD))?;
        if page.mime != VISUAL_PAGE_MIME {
            return Err(HubError::invalid(format!(
                "The page must have the media type {VISUAL_PAGE_MIME}."
            )));
        }
        // The size comes from the blob that the hub holds, not from the call.
        let stored = self.stored_blob_size(&page.sha256).await.ok_or_else(|| {
            HubError::invalid("The hub has no blob for this page. The session host uploads it before it calls this tool.")
        })?;
        if stored > VISUAL_PAGE_MAX_BYTES || page.size > VISUAL_PAGE_MAX_BYTES {
            return Err(HubError::invalid(format!(
                "The page has more than {} MiB. Make it smaller.",
                VISUAL_PAGE_MAX_BYTES / (1024 * 1024)
            )));
        }
        page.size = stored;
        let heights = args.heights.unwrap_or_default();
        check_heights(&heights)?;
        let body = VisualNotice::new(&title, args.height, heights).to_body();
        let (channel_id, session_id) = (ctx.channel.id, ctx.session.id);
        let task = ctx.task.clone();
        let message = self
            .store
            .write(move |tx| {
                // A coordinator posts under the rules of its turn, as with
                // `post_message`: the research pass may not post, and a
                // proactive turn marks the visual. A task session has no
                // turn of this kind.
                let posting = match &task {
                    None => Some(crate::proactivity::tools::posting(
                        tx,
                        session_id,
                        channel_id,
                        false,
                        Vec::new(),
                    )?),
                    Some(_) => None,
                };
                let mut new =
                    NewMessage::new(channel_id, Author::Assistant, MessageKind::Notice, body);
                new.author_session_id = Some(session_id);
                new.attachments = vec![page];
                if let Some(task) = &task {
                    new.thread_root_id = Some(task.card_message_id);
                    new.author_task_id = Some(task.id);
                }
                let message = tx.insert_message(new)?;
                match posting {
                    Some(posting) => crate::tools::chat::mark_message(tx, message, posting),
                    None => Ok(message),
                }
            })
            .await?;
        Ok(serde_json::to_value(ShowVisualResult {
            message_id: message.id,
            message: SHOW_VISUAL_SHOWN.to_owned(),
        })?)
    }

    /// `visual.render`: an image of a visual at a width, a scale and a
    /// theme. The hub answers from the cache. On a miss it asks a session
    /// host, saves the image, and answers. Requests for one key that come
    /// together share one render.
    pub(crate) async fn visual_render(
        &self,
        _caller: &Caller,
        request: VisualRender,
    ) -> Result<VisualImage> {
        check_render(&request)?;
        let message_id = request.message_id;
        let page = self
            .store
            .read(move |conn| {
                let message = conversation::message(conn, message_id)?
                    .filter(|m| m.deleted_at.is_none())
                    .ok_or_else(not_a_visual)?;
                // The same check as the other requests of a message: the
                // channel must exist.
                conversation::channel_or_404(conn, message.channel_id)?;
                if message.kind != MessageKind::Notice
                    || VisualNotice::parse(&message.body).is_none()
                {
                    return Err(not_a_visual());
                }
                message
                    .attachments
                    .into_iter()
                    .find(|a| a.mime == VISUAL_PAGE_MIME)
                    .ok_or_else(not_a_visual)
            })
            .await?;
        if self.stored_blob_size(&page.sha256).await.is_none() {
            return Err(HubError::not_found(
                "The page of this visual is no longer on the hub.",
            ));
        }
        let key = image_key(&page.sha256, &request);
        if let Some(image) = self.cached_visual_image(&key).await? {
            return Ok(image);
        }
        let flight = self.visual_renders.flight(&key);
        let result = flight
            .get_or_init(|| async {
                let drawn = self.draw_visual(message_id, page, &request, &key).await;
                self.visual_renders.land(&key, &flight);
                drawn.map_err(|err| err.to_error_body())
            })
            .await;
        result.clone().map_err(HubError::Api)
    }

    /// The image of a key, when the cache has the row and the hub still
    /// holds the blob.
    async fn cached_visual_image(&self, key: &str) -> Result<Option<VisualImage>> {
        let key = key.to_owned();
        let Some(row) = self
            .store
            .read(move |conn| visuals::image(conn, &key))
            .await?
        else {
            return Ok(None);
        };
        Ok(self
            .stored_blob_size(&row.image_sha256)
            .await
            .map(|size| VisualImage {
                image: BlobRef {
                    sha256: row.image_sha256,
                    name: IMAGE_NAME.to_owned(),
                    mime: IMAGE_MIME.to_owned(),
                    size,
                },
                height: row.height,
            }))
    }

    /// Asks a session host for the image, checks the answer and saves the
    /// row.
    async fn draw_visual(
        &self,
        message_id: MessageId,
        page: BlobRef,
        request: &VisualRender,
        key: &str,
    ) -> Result<VisualImage> {
        let answer = self
            .ask_session_host(HubToSessionHost::VisualRender(VisualRenderCommand {
                page,
                width: request.width,
                scale: request.scale,
                style: request.style.clone(),
            }))
            .await?;
        let drawn: VisualImage = serde_json::from_value(answer).map_err(|_| {
            HubError::new(
                ErrorCode::Internal,
                "The session host gave an answer that the hub cannot read. Update the session host.",
            )
        })?;
        if drawn.height == 0 || drawn.height > VISUAL_FIT_MAX {
            return Err(HubError::new(
                ErrorCode::Internal,
                "The session host gave an image with a height that is not possible.",
            ));
        }
        let size = self
            .stored_blob_size(&drawn.image.sha256)
            .await
            .ok_or_else(|| {
                HubError::new(
                    ErrorCode::Internal,
                    "The session host did not upload the image. Try again.",
                )
            })?;
        let row = CachedImage {
            image_sha256: drawn.image.sha256.clone(),
            size,
            height: drawn.height,
        };
        let key = key.to_owned();
        self.store
            .write(move |tx| {
                // A message that went while the image was drawn leaves no row.
                let alive = conversation::message(tx.db(), message_id)?
                    .is_some_and(|m| m.deleted_at.is_none());
                if alive {
                    tx.save_visual_image(&key, message_id, &row)?;
                }
                Ok(())
            })
            .await?;
        Ok(VisualImage {
            image: BlobRef {
                sha256: drawn.image.sha256,
                name: IMAGE_NAME.to_owned(),
                mime: IMAGE_MIME.to_owned(),
                size,
            },
            height: drawn.height,
        })
    }

    /// The links of the session hosts that are online, the one of the hub
    /// machine first. A session host is a program on the hub machine, and
    /// the first one that paired is the one that the setup made, so the
    /// hub tries the session hosts in the order of their pairing. The
    /// others are for a hub that has more than one.
    async fn online_session_hosts(&self) -> Result<Vec<Arc<WorkerLink>>> {
        let mut hosts: Vec<_> = self
            .store
            .read(machine::workers)
            .await?
            .into_iter()
            .filter(|w| {
                w.kind == consort_proto::model::MachineKind::SessionHost && w.revoked_at.is_none()
            })
            .collect();
        hosts.sort_by_key(|w| (w.paired_at, w.id));
        Ok(hosts
            .iter()
            .filter(|w| self.links.is_online(w.id))
            .filter_map(|w| self.links.get(w.id))
            .collect())
    }

    /// Sends a command to an online session host and waits for the answer.
    /// An error of the session host passes through with its message. A
    /// connection that closes while the hub waits makes the hub try the
    /// next session host.
    async fn ask_session_host(&self, command: HubToSessionHost) -> Result<Value> {
        let hosts = self.online_session_hosts().await?;
        let request = match Frame::family_request(FrameId::generate(), &command)? {
            Frame::Request(request) => request,
            _ => return Err(HubError::internal("a command did not encode as a request")),
        };
        let timeout = self.visual_renders.render_timeout();
        for link in hosts {
            match link
                .call(
                    FrameId::generate(),
                    request.kind.clone(),
                    request.body.clone(),
                    timeout,
                )
                .await
            {
                Ok(Ok(value)) => return Ok(value),
                Ok(Err(body)) => return Err(HubError::Api(body)),
                Err(CallError::Gone) => continue,
                Err(CallError::Timeout) => {
                    return Err(unavailable(
                        "The session host did not draw the visual in time. Try again.",
                    ));
                }
                Err(CallError::Encode(reason)) => {
                    return Err(HubError::internal(format!(
                        "a visual command did not encode: {reason}"
                    )));
                }
            }
        }
        Err(no_session_host())
    }
}

#[cfg(test)]
mod tests;
