//! Uploads: the files that the user attaches before they go to the hub.
//!
//! An upload belongs to one target: the composer of a channel, the reply box
//! of a task thread, or a folder of a channel's files. It goes through these
//! states:
//!
//! - **Preparing**: the client reads the file and makes its SHA-256 hash.
//! - **Uploading**: `PUT /v1/blobs/{sha256}` runs. A blob that the hub has
//!   already skips this step.
//! - **Ready**: the blob is on the hub. A message, a reply or `files.put`
//!   can name it.
//! - **Failed**: the message says what happened and what to do. Try again
//!   starts again from Preparing.
//!
//! Each start of the work has an attempt number. A result from an older
//! attempt (an upload that the user removed or tried again) does nothing.
//! [`Uploads::gate`] tells a send whether it can go now, must wait, or
//! cannot go.

use consort_proto::model::{BlobRef, Sha256Hash};

use crate::format::file_size_label;

/// Where an upload goes when it is ready.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum UploadTarget {
    /// The composer of a channel. The upload goes out with the next message.
    Composer { channel_id: String },
    /// The reply box of a task thread.
    Reply { task_id: String },
    /// A folder of a channel's files. `folder` is empty for the top folder.
    /// The upload becomes a channel file when it is ready.
    ChannelFiles { channel_id: String, folder: String },
}

/// Where the bytes of an upload come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadSource {
    /// A file on this computer.
    Path(String),
    /// A blob that is on the hub already, such as a channel file.
    Blob(BlobRef),
}

/// The state of one upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadState {
    /// The client reads the file and makes its hash.
    Preparing,
    /// The bytes go to the hub.
    Uploading { done: u64, total: u64 },
    /// The blob is on the hub.
    Ready(BlobRef),
    /// What happened and what to do next.
    Failed(String),
}

/// One upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upload {
    pub id: String,
    pub target: UploadTarget,
    pub source: UploadSource,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub state: UploadState,
    /// Counts the starts of the work. A result for another attempt is old.
    pub attempt: u32,
}

/// What a send can do with the uploads of its target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendGate {
    /// Every upload is on the hub. These are the blobs, in the order the
    /// user attached them.
    Ready(Vec<BlobRef>),
    /// At least one upload still runs, and none failed.
    Waiting,
    /// An upload failed. The send cannot go until the user tries it again or
    /// removes it. The name of the first failed file.
    Failed(String),
}

/// The uploads of every target, in the order the user attached them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Uploads {
    items: Vec<Upload>,
    next: u64,
}

impl Uploads {
    /// Adds an upload. A file starts in Preparing. A blob that is on the hub
    /// is Ready at once. Returns its ID.
    pub fn add(
        &mut self,
        target: UploadTarget,
        source: UploadSource,
        name: String,
        mime: String,
        size: u64,
    ) -> String {
        self.next += 1;
        let id = format!("upl_{}", self.next);
        let state = match &source {
            UploadSource::Blob(blob) => UploadState::Ready(blob.clone()),
            UploadSource::Path(_) => UploadState::Preparing,
        };
        self.items.push(Upload {
            id: id.clone(),
            target,
            source,
            name,
            mime,
            size,
            state,
            attempt: 1,
        });
        id
    }

    pub fn get(&self, id: &str) -> Option<&Upload> {
        self.items.iter().find(|u| u.id == id)
    }

    pub fn all(&self) -> &[Upload] {
        &self.items
    }

    /// The uploads of one target, oldest first.
    pub fn of<'a>(&'a self, target: &'a UploadTarget) -> impl Iterator<Item = &'a Upload> + 'a {
        self.items.iter().filter(move |u| &u.target == target)
    }

    /// The upload `id` at `attempt`, while it still runs. `None` for an
    /// upload that is gone, that ended, or that started again.
    fn running_mut(&mut self, id: &str, attempt: u32) -> Option<&mut Upload> {
        self.items.iter_mut().find(|u| {
            u.id == id
                && u.attempt == attempt
                && matches!(
                    u.state,
                    UploadState::Preparing | UploadState::Uploading { .. }
                )
        })
    }

    /// Records progress of the transfer. False when the result is old.
    pub fn progress(&mut self, id: &str, attempt: u32, done: u64, total: u64) -> bool {
        match self.running_mut(id, attempt) {
            Some(upload) => {
                let total = total.max(1);
                upload.state = UploadState::Uploading {
                    done: done.min(total),
                    total,
                };
                true
            }
            None => false,
        }
    }

    /// The blob is on the hub. False when the result is old.
    pub fn finish(&mut self, id: &str, attempt: u32, sha256: Sha256Hash, size: u64) -> bool {
        match self.running_mut(id, attempt) {
            Some(upload) => {
                upload.size = size;
                upload.state = UploadState::Ready(BlobRef {
                    sha256,
                    name: upload.name.clone(),
                    mime: upload.mime.clone(),
                    size,
                });
                true
            }
            None => false,
        }
    }

    /// The upload failed with `message`. False when the result is old.
    pub fn fail(&mut self, id: &str, attempt: u32, message: String) -> bool {
        match self.running_mut(id, attempt) {
            Some(upload) => {
                upload.state = UploadState::Failed(message);
                true
            }
            None => false,
        }
    }

    /// Starts a failed upload again. Returns the new attempt, or `None` when
    /// the upload is gone or did not fail.
    pub fn retry(&mut self, id: &str) -> Option<u32> {
        let upload = self.items.iter_mut().find(|u| u.id == id)?;
        if !matches!(upload.state, UploadState::Failed(_)) {
            return None;
        }
        upload.attempt += 1;
        upload.state = UploadState::Preparing;
        Some(upload.attempt)
    }

    /// Takes an upload away. A transfer that still runs finishes, but its
    /// result does nothing.
    pub fn remove(&mut self, id: &str) -> Option<Upload> {
        let at = self.items.iter().position(|u| u.id == id)?;
        Some(self.items.remove(at))
    }

    /// Whether a send of `target` can go with `ids` (all the uploads of the
    /// target when `ids` is `None`).
    pub fn gate(&self, target: &UploadTarget, ids: Option<&[String]>) -> SendGate {
        let mut blobs = Vec::new();
        let mut waiting = false;
        for upload in self.of(target) {
            if ids.is_some_and(|ids| !ids.contains(&upload.id)) {
                continue;
            }
            match &upload.state {
                UploadState::Ready(blob) => blobs.push(blob.clone()),
                UploadState::Failed(_) => return SendGate::Failed(upload.name.clone()),
                UploadState::Preparing | UploadState::Uploading { .. } => waiting = true,
            }
        }
        if waiting {
            SendGate::Waiting
        } else {
            SendGate::Ready(blobs)
        }
    }

    /// Takes the uploads with `ids` away after their send went out.
    pub fn take(&mut self, ids: &[String]) {
        self.items.retain(|u| !ids.contains(&u.id));
    }

    /// The views of every upload.
    pub fn views(&self) -> Vec<UploadView> {
        self.items.iter().map(UploadView::of).collect()
    }
}

/// An upload as the composer, the reply box or the Files tab shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UploadView {
    pub id: String,
    pub target: Option<UploadTarget>,
    pub name: String,
    pub mime: String,
    /// An image: the chip shows a thumbnail.
    pub image: bool,
    /// "PDF, 2.4 MB".
    pub detail: String,
    /// The file on this computer, for the thumbnail of an image. `None`
    /// for a blob from the hub that is not in the local cache.
    pub local_path: Option<String>,
    /// The blob hash, once it is known.
    pub sha256: Option<String>,
    pub size: u64,
    pub status: UploadStatus,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum UploadStatus {
    #[default]
    Preparing,
    /// The share that went up, from 0 to 1, and its label ("62%").
    Uploading {
        fraction: f32,
        label: String,
    },
    Ready,
    /// What happened and what to do next.
    Failed(String),
}

impl UploadView {
    fn of(upload: &Upload) -> UploadView {
        let status = match &upload.state {
            UploadState::Preparing => UploadStatus::Preparing,
            UploadState::Uploading { done, total } => {
                let fraction = (*done as f64 / (*total).max(1) as f64).clamp(0.0, 1.0) as f32;
                UploadStatus::Uploading {
                    fraction,
                    label: format!("{}%", (fraction * 100.0).floor() as u32),
                }
            }
            UploadState::Ready(_) => UploadStatus::Ready,
            UploadState::Failed(message) => UploadStatus::Failed(message.clone()),
        };
        let sha256 = match &upload.state {
            UploadState::Ready(blob) => Some(blob.sha256.to_string()),
            _ => match &upload.source {
                UploadSource::Blob(blob) => Some(blob.sha256.to_string()),
                UploadSource::Path(_) => None,
            },
        };
        UploadView {
            id: upload.id.clone(),
            target: Some(upload.target.clone()),
            name: upload.name.clone(),
            mime: upload.mime.clone(),
            image: is_image(&upload.mime),
            detail: file_detail(&upload.mime, &upload.name, upload.size),
            local_path: match &upload.source {
                UploadSource::Path(path) => Some(path.clone()),
                UploadSource::Blob(_) => None,
            },
            sha256,
            size: upload.size,
            status,
        }
    }
}

/// The media type of a file, from its name. `application/octet-stream`
/// for a name that it does not know.
pub fn mime_for(name: &str) -> String {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        "tif" | "tiff" => "image/tiff",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "json" => "application/json",
        "md" => "text/markdown",
        "txt" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "rs" | "py" | "ts" | "js" | "toml" | "yaml" | "yml" | "sh" => "text/plain",
        _ => "application/octet-stream",
    }
    .to_owned()
}

/// Whether the app can show a file of this type as a picture. HEIC and TIFF
/// are images, but the app cannot draw them, so they show as file chips.
pub fn is_image(mime: &str) -> bool {
    matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp" | "image/svg+xml"
    )
}

/// The short type name of a file: "PDF", "JPEG", "XLSX". From the name's
/// extension, else from the media type, else "File".
pub fn type_label(mime: &str, name: &str) -> String {
    if let Some((_, ext)) = name.rsplit_once('.')
        && !ext.is_empty()
        && ext.len() <= 5
    {
        let ext = ext.to_ascii_uppercase();
        return match ext.as_str() {
            "JPG" => "JPEG".to_owned(),
            _ => ext,
        };
    }
    match mime.split_once('/') {
        Some((_, sub)) if !sub.is_empty() && sub != "octet-stream" => sub.to_ascii_uppercase(),
        _ => "File".to_owned(),
    }
}

/// "PDF, 2.4 MB".
pub fn file_detail(mime: &str, name: &str, size: u64) -> String {
    format!("{}, {}", type_label(mime, name), file_size_label(size))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(byte: u8) -> Sha256Hash {
        Sha256Hash::from_bytes([byte; 32])
    }

    fn composer() -> UploadTarget {
        UploadTarget::Composer {
            channel_id: "chn_1".into(),
        }
    }

    fn file(uploads: &mut Uploads, name: &str) -> String {
        uploads.add(
            composer(),
            UploadSource::Path(format!("/tmp/{name}")),
            name.into(),
            mime_for(name),
            1000,
        )
    }

    #[test]
    fn an_upload_goes_from_preparing_through_uploading_to_ready() {
        let mut uploads = Uploads::default();
        let id = file(&mut uploads, "quote.pdf");
        assert_eq!(uploads.get(&id).unwrap().state, UploadState::Preparing);
        assert_eq!(uploads.gate(&composer(), None), SendGate::Waiting);
        assert!(uploads.progress(&id, 1, 620, 1000));
        let view = &uploads.views()[0];
        assert_eq!(
            view.status,
            UploadStatus::Uploading {
                fraction: 0.62,
                label: "62%".into()
            }
        );
        assert!(uploads.finish(&id, 1, hash(1), 1000));
        let SendGate::Ready(blobs) = uploads.gate(&composer(), None) else {
            panic!("ready")
        };
        assert_eq!(blobs.len(), 1);
        assert_eq!(blobs[0].name, "quote.pdf");
        assert_eq!(blobs[0].mime, "application/pdf");
        assert_eq!(blobs[0].sha256, hash(1));
    }

    #[test]
    fn a_failed_upload_blocks_the_send_until_it_is_tried_again_or_removed() {
        let mut uploads = Uploads::default();
        let good = file(&mut uploads, "a.png");
        let bad = file(&mut uploads, "gutter-detail.heic");
        uploads.finish(&good, 1, hash(1), 1000);
        assert!(uploads.fail(&bad, 1, "Upload failed. The hub is offline.".into()));
        assert_eq!(
            uploads.gate(&composer(), None),
            SendGate::Failed("gutter-detail.heic".into())
        );
        // Try again starts a new attempt, and the send waits.
        assert_eq!(uploads.retry(&bad), Some(2));
        assert_eq!(uploads.gate(&composer(), None), SendGate::Waiting);
        // A result from the first attempt does nothing.
        assert!(!uploads.finish(&bad, 1, hash(9), 1000));
        assert!(uploads.finish(&bad, 2, hash(2), 1000));
        assert!(matches!(uploads.gate(&composer(), None), SendGate::Ready(b) if b.len() == 2));
        // Remove takes it out of the send.
        uploads.fail(&good, 1, "late".into());
        assert!(matches!(
            uploads.get(&good).unwrap().state,
            UploadState::Ready(_)
        ));
        uploads.remove(&bad);
        assert!(matches!(uploads.gate(&composer(), None), SendGate::Ready(b) if b.len() == 1));
    }

    #[test]
    fn retry_works_only_on_a_failed_upload_and_a_removed_upload_takes_no_result() {
        let mut uploads = Uploads::default();
        let id = file(&mut uploads, "a.txt");
        assert_eq!(uploads.retry(&id), None);
        uploads.remove(&id);
        assert!(!uploads.progress(&id, 1, 1, 2));
        assert!(!uploads.finish(&id, 1, hash(1), 2));
        assert!(uploads.all().is_empty());
    }

    #[test]
    fn targets_do_not_mix_and_a_blob_from_the_hub_is_ready_at_once() {
        let mut uploads = Uploads::default();
        let reply = UploadTarget::Reply {
            task_id: "tsk_1".into(),
        };
        file(&mut uploads, "a.pdf");
        let blob = BlobRef {
            sha256: hash(3),
            name: "warranty.pdf".into(),
            mime: "application/pdf".into(),
            size: 380_000,
        };
        let id = uploads.add(
            reply.clone(),
            UploadSource::Blob(blob.clone()),
            blob.name.clone(),
            blob.mime.clone(),
            blob.size,
        );
        assert_eq!(uploads.gate(&reply, None), SendGate::Ready(vec![blob]));
        assert_eq!(uploads.gate(&composer(), None), SendGate::Waiting);
        assert_eq!(uploads.of(&reply).count(), 1);
        uploads.take(&[id]);
        assert_eq!(uploads.gate(&reply, None), SendGate::Ready(vec![]));
        assert_eq!(uploads.all().len(), 1);
    }

    #[test]
    fn a_send_can_name_only_some_uploads_of_its_target() {
        let mut uploads = Uploads::default();
        let first = file(&mut uploads, "a.pdf");
        let _later = file(&mut uploads, "b.pdf");
        uploads.finish(&first, 1, hash(1), 1);
        assert!(matches!(
            uploads.gate(&composer(), Some(std::slice::from_ref(&first))),
            SendGate::Ready(b) if b.len() == 1
        ));
    }

    #[test]
    fn names_give_the_type_and_the_detail() {
        assert_eq!(mime_for("Roof.JPG"), "image/jpeg");
        assert_eq!(mime_for("notes"), "application/octet-stream");
        assert!(is_image("image/png"));
        assert!(!is_image("image/heic"));
        assert_eq!(type_label("application/pdf", "brightline-quote.pdf"), "PDF");
        assert_eq!(type_label("image/jpeg", "roof.jpg"), "JPEG");
        assert_eq!(type_label("application/octet-stream", "notes"), "File");
        assert_eq!(type_label("image/png", "paste"), "PNG");
        assert_eq!(
            file_detail("application/pdf", "q.pdf", 2_516_582),
            "PDF, 2.4 MB"
        );
    }
}
