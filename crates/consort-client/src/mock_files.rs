//! The mock's files, as the phase 1 designs show them: the channel files of
//! house-lights, the photos and the quote that the user sent there, the
//! artifacts of the roofline task, and the uploads of the composer.
//!
//! The pictures are small SVG drawings that the mock writes to a folder in
//! the temp folder, so a thumbnail and the light box have a file to draw.

use std::path::PathBuf;

use consort_proto::model::BlobRef;

use crate::{
    blob::BlobTransfer,
    files::{AttachmentView, ChannelFileView, attachment_view, split_path},
    format,
    uploads::{UploadStatus, UploadTarget, UploadView, file_detail, is_image, mime_for},
    view::*,
};

/// The house-lights channel of the mock.
pub const LIGHTS: &str = "chn_house_lights";
/// The roofline task of the mock.
pub const ROOF_TASK: &str = "tsk_0192f0c4-roofline";
/// A resolved task with an artifact, in the Files tab.
pub const SWITCH_TASK: &str = "tsk_0192f0c4-porch-switches";
/// A channel with no files, for the empty Files tab.
pub const EMPTY_CHANNEL: &str = "chn_mac_ext4";

const DAY: i64 = 86_400_000;

/// A front view of a house at dusk with a line of lights on its roof.
const ROOF_FRONT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="900" viewBox="0 0 1200 900"><defs><linearGradient id="s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#34466A"/><stop offset="1" stop-color="#9B7B6A"/></linearGradient></defs><rect width="1200" height="900" fill="url(#s)"/><circle cx="960" cy="160" r="52" fill="#7D8091"/><path d="M0 620 L260 460 L510 590 L780 360 L1200 600 L1200 900 L0 900 Z" fill="#1B2321"/><path d="M260 460 L510 590 L780 360 L1200 600" fill="none" stroke="#F3D277" stroke-width="8" stroke-dasharray="14 12"/><rect x="330" y="680" width="80" height="70" fill="#C9A15A"/><rect x="800" y="640" width="80" height="70" fill="#957A4A"/></svg>"##;

/// A side view of a long roof at night.
const ROOF_SIDE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="900" viewBox="0 0 1200 900"><defs><linearGradient id="s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#2A3A4E"/><stop offset="1" stop-color="#4E5462"/></linearGradient></defs><rect width="1200" height="900" fill="url(#s)"/><path d="M0 480 L600 300 L1200 480 L1200 900 L0 900 Z" fill="#1B2321"/><path d="M0 480 L600 300 L1200 480" fill="none" stroke="#F3D277" stroke-width="8" stroke-dasharray="14 12"/><rect x="60" y="520" width="1080" height="14" fill="#3A4441"/><rect x="560" y="640" width="90" height="150" fill="#957A4A"/></svg>"##;

/// A garage at dusk.
const GARAGE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="900" viewBox="0 0 1200 900"><defs><linearGradient id="s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#3D4C69"/><stop offset="1" stop-color="#A88A73"/></linearGradient></defs><rect width="1200" height="900" fill="url(#s)"/><path d="M100 520 L600 280 L1100 520 L1100 900 L100 900 Z" fill="#1E2624"/><rect x="380" y="600" width="440" height="300" fill="#2C3533"/></svg>"##;

/// The roofline photo with the planned runs marked.
const ROOF_MARKED: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="900" viewBox="0 0 1200 900"><rect width="1200" height="900" fill="#55607A"/><path d="M0 620 L260 460 L510 590 L780 360 L1200 600 L1200 900 L0 900 Z" fill="#1B2321"/><path d="M260 460 L510 590 L780 360 L1200 600" fill="none" stroke="#77B4ED" stroke-width="10"/><text x="420" y="430" font-family="sans-serif" font-size="44" fill="#F0EBDF">62 ft</text><text x="900" y="420" font-family="sans-serif" font-size="44" fill="#F0EBDF">124 ft</text></svg>"##;

/// A document page, for the files that are not pictures.
const PAGE: &str = "Consort mock file. The real file is on the hub.\n";

/// One mock file: its name, its size, its contents.
struct MockFile {
    name: &'static str,
    size: u64,
    body: &'static str,
}

const fn mb(tenths: u64) -> u64 {
    tenths * 1024 * 1024 / 10
}

const FILES: &[MockFile] = &[
    MockFile {
        name: "roofline-front.jpg",
        size: mb(31),
        body: ROOF_FRONT,
    },
    MockFile {
        name: "roofline-side.jpg",
        size: mb(28),
        body: ROOF_SIDE,
    },
    MockFile {
        name: "garage-side.jpg",
        size: mb(28),
        body: GARAGE,
    },
    MockFile {
        name: "porch.jpg",
        size: mb(19),
        body: ROOF_SIDE,
    },
    MockFile {
        name: "roofline-marked.png",
        size: mb(22),
        body: ROOF_MARKED,
    },
    MockFile {
        name: "brightline-quote.pdf",
        size: mb(24),
        body: PAGE,
    },
    MockFile {
        name: "glowline-quote.pdf",
        size: mb(11),
        body: PAGE,
    },
    MockFile {
        name: "lumen-quote.pdf",
        size: mb(9),
        body: PAGE,
    },
    MockFile {
        name: "wiring-plan.pdf",
        size: mb(12),
        body: PAGE,
    },
    MockFile {
        name: "brightline-warranty.pdf",
        size: 380 * 1024,
        body: PAGE,
    },
    MockFile {
        name: "measurements.xlsx",
        size: 96 * 1024,
        body: PAGE,
    },
    MockFile {
        name: "quote-comparison.xlsx",
        size: 48 * 1024,
        body: PAGE,
    },
    MockFile {
        name: "switch-shortlist.md",
        size: 6 * 1024,
        body: PAGE,
    },
    MockFile {
        name: "gutter-detail.heic",
        size: mb(41),
        body: PAGE,
    },
];

/// The folder where the mock keeps its files.
pub fn folder() -> PathBuf {
    std::env::temp_dir().join("consort-mock-files")
}

fn find(name: &str) -> &'static MockFile {
    FILES
        .iter()
        .find(|f| f.name == name)
        .unwrap_or(&FILES[FILES.len() - 1])
}

/// The blob of a mock file. Its hash is the hash of its name, so it stays
/// the same in every run.
pub fn blob(name: &str) -> BlobRef {
    let file = find(name);
    BlobRef {
        sha256: BlobTransfer::hash_bytes(name.as_bytes()),
        name: name.to_owned(),
        mime: mime_for(name),
        size: file.size,
    }
}

/// Writes the mock files to [`folder`] once and returns the path of one.
/// `None` when the temp folder cannot be written; the views then show no
/// thumbnail.
pub fn local_path(name: &str) -> Option<String> {
    let file = find(name);
    let dir = folder();
    let path = dir.join(file.name);
    if !path.exists() {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&path, file.body).ok()?;
    }
    Some(path.display().to_string())
}

/// The view of a mock attachment.
pub fn attachment(name: &str, artifact: Option<(&str, &str, TaskStatus)>) -> AttachmentView {
    attachment_view(
        &blob(name),
        artifact.map(|(id, task_name, status)| crate::files::ArtifactTask {
            task_id: id.into(),
            task_name: task_name.into(),
            status,
        }),
        local_path(name),
    )
}

fn file(path: &str, task: Option<&str>, days_ago: i64) -> ChannelFileView {
    let (folder, name) = split_path(path);
    let b = blob(name);
    let at = crate::mock::MOCK_NOW - days_ago * DAY;
    ChannelFileView {
        channel_id: LIGHTS.into(),
        path: path.into(),
        name: name.into(),
        folder: folder.into(),
        sha256: b.sha256.to_string(),
        size: b.size,
        image: is_image(&b.mime),
        mime: b.mime,
        size_label: format::file_size_label(b.size),
        date_label: format::file_date_label(at, crate::mock::MOCK_NOW),
        updated_at: at,
        task_id: task.map(str::to_owned),
        local_path: local_path(name),
    }
}

/// The channel files of house-lights.
pub fn channel_files() -> Vec<ChannelFileView> {
    let roof = format!("artifacts/{ROOF_TASK}");
    let switches = format!("artifacts/{SWITCH_TASK}");
    vec![
        file("photos/roofline-front.jpg", None, 0),
        file("photos/roofline-side.jpg", None, 0),
        file("photos/garage-side.jpg", None, 1),
        file("photos/porch.jpg", None, 2),
        file("quotes/brightline-quote.pdf", None, 0),
        file("quotes/glowline-quote.pdf", None, 3),
        file("quotes/lumen-quote.pdf", None, 3),
        file("wiring-plan.pdf", None, 5),
        file("brightline-warranty.pdf", None, 6),
        file(&format!("{roof}/quote-comparison.xlsx"), Some(ROOF_TASK), 0),
        file(&format!("{roof}/roofline-marked.png"), Some(ROOF_TASK), 0),
        file(
            &format!("{switches}/switch-shortlist.md"),
            Some(SWITCH_TASK),
            8,
        ),
    ]
}

/// An upload view of a mock file.
pub fn upload(id: &str, target: UploadTarget, name: &str, status: UploadStatus) -> UploadView {
    let b = blob(name);
    UploadView {
        id: id.into(),
        target: Some(target),
        name: name.into(),
        image: is_image(&b.mime),
        detail: file_detail(&b.mime, name, b.size),
        local_path: local_path(name),
        sha256: matches!(status, UploadStatus::Ready).then(|| b.sha256.to_string()),
        size: b.size,
        mime: b.mime,
        status,
    }
}

/// The composer of house-lights in the design "Composer with
/// attachments": two pictures, a file, an upload that runs and one that
/// failed.
pub fn composer_uploads() -> Vec<UploadView> {
    let target = UploadTarget::Composer {
        channel_id: LIGHTS.into(),
    };
    vec![
        upload(
            "upl_mock_1",
            target.clone(),
            "roofline-front.jpg",
            UploadStatus::Ready,
        ),
        upload(
            "upl_mock_2",
            target.clone(),
            "roofline-side.jpg",
            UploadStatus::Ready,
        ),
        upload(
            "upl_mock_3",
            target.clone(),
            "brightline-quote.pdf",
            UploadStatus::Ready,
        ),
        upload(
            "upl_mock_4",
            target.clone(),
            "measurements.xlsx",
            UploadStatus::Uploading {
                fraction: 0.62,
                label: "62%".into(),
            },
        ),
        upload(
            "upl_mock_5",
            target,
            "gutter-detail.heic",
            UploadStatus::Failed("Upload failed. The hub is offline.".into()),
        ),
    ]
}

/// An upload into the top folder of the house-lights files, as the Files
/// tab shows it.
pub fn files_upload() -> UploadView {
    upload(
        "upl_mock_files",
        UploadTarget::ChannelFiles {
            channel_id: LIGHTS.into(),
            folder: String::new(),
        },
        "measurements.xlsx",
        UploadStatus::Uploading {
            fraction: 0.45,
            label: "45%".into(),
        },
    )
}

fn message(
    id: &str,
    speaker: Speaker,
    time: &str,
    body: &str,
    attachments: Vec<AttachmentView>,
) -> TimelineItem {
    TimelineItem::Message(MessageView {
        id: id.into(),
        channel_id: LIGHTS.into(),
        speaker,
        time_label: time.into(),
        body: body.into(),
        attachments,
        ..MessageView::default()
    })
}

/// The house-lights timeline of the design "Bubbles with attachments".
pub fn timeline_items() -> Vec<TimelineItem> {
    let roof = Some((ROOF_TASK, "Price out roofline lighting", TaskStatus::Review));
    vec![
        TimelineItem::Day(DayView {
            key: "day_lights_today".into(),
            label: "Today".into(),
        }),
        message(
            "msg_lights_photos",
            Speaker::You,
            "7:40 PM",
            "Here are the roofline photos and the quote from Brightline. Can you compare it with the other two?",
            vec![
                attachment("roofline-front.jpg", None),
                attachment("roofline-side.jpg", None),
                attachment("brightline-quote.pdf", None),
            ],
        ),
        TimelineItem::TaskCard(TaskCardView {
            message_id: format!("msg_card_{ROOF_TASK}"),
            task_id: ROOF_TASK.into(),
            time_label: "8:25 PM".into(),
            worker: Some(WorkerChip {
                name: "hub".into(),
                online: true,
                account: "Claude Work".into(),
            }),
            summary: "Compared three quotes and marked up both photos. Published 2 artifacts."
                .into(),
            ..TaskCardView::default()
        }),
        message(
            "msg_lights_compare",
            Speaker::Consort,
            "8:26 PM",
            "The comparison is ready. Brightline costs the most, at $4,850 for 186 ft of roofline. It is also the only quote with a 5-year warranty.",
            vec![attachment("quote-comparison.xlsx", roof)],
        ),
        message(
            "msg_lights_warranty",
            Speaker::Consort,
            "8:26 PM",
            "Section 4 of their warranty covers the dimmers. I attached it from the channel files.",
            vec![attachment("brightline-warranty.pdf", None)],
        ),
    ]
}

/// The mock with the files of the phase 1 designs: house-lights has its
/// files, its photos and quotes, and the roofline task is ready for review
/// with two artifacts.
pub fn with_files(mut state: ViewState) -> ViewState {
    if let Some(timeline) = state.timelines.iter_mut().find(|t| t.channel_id == LIGHTS) {
        timeline.items = timeline_items();
    }
    if let Some(task) = state.tasks.iter_mut().find(|t| t.id == ROOF_TASK) {
        task.title = "Price out roofline lighting".into();
        task.status = TaskStatus::Review;
        task.worker_name = Some("hub".into());
        task.account_label = Some("Claude Work".into());
    }
    if !state.tasks.iter().any(|t| t.id == SWITCH_TASK) {
        state.tasks.push(TaskView {
            id: SWITCH_TASK.into(),
            channel_id: LIGHTS.into(),
            channel_name: "house-lights".into(),
            title: "Pick smart switches for the porch".into(),
            status: TaskStatus::Resolved,
            age_label: "8 d".into(),
            updated_at: crate::mock::MOCK_NOW - 8 * DAY,
            card_message_id: format!("msg_card_{SWITCH_TASK}"),
            ..TaskView::default()
        });
    }
    if let Some(thread) = state.threads.iter_mut().find(|t| t.task_id == ROOF_TASK) {
        thread.title = "Price out roofline lighting".into();
        thread.status = TaskStatus::Review;
        thread.posts.push(ThreadPost {
            id: "msg_roof_artifacts".into(),
            speaker: Speaker::Task,
            time_label: "8:24 PM".into(),
            bodies: vec![
                PostBody::Text(
                    "I compared the three quotes per foot and marked the light runs on your front photo. Brightline is $26 a foot. The other two are $19 and $21."
                        .into(),
                ),
                PostBody::Attachments(vec![
                    attachment(
                        "quote-comparison.xlsx",
                        Some((ROOF_TASK, "Price out roofline lighting", TaskStatus::Review)),
                    ),
                    attachment(
                        "roofline-marked.png",
                        Some((ROOF_TASK, "Price out roofline lighting", TaskStatus::Review)),
                    ),
                ]),
            ],
            pending: false,
        });
    }
    crate::mock::stamp_times(&mut state.timelines);
    state
}

/// How the mock moves an upload along: a few steps of progress, then the
/// blob is on the "hub".
const UPLOAD_STEPS: [(u64, f32); 3] = [(250, 0.35), (550, 0.8), (850, 1.0)];

fn invalid(message: &str) -> crate::ClientError {
    crate::ClientError::Hub {
        code: "invalid".into(),
        message: message.into(),
    }
}

/// Moves upload `id` through the mock's steps. When it is ready and goes to
/// a folder of channel files, it becomes a file there.
fn simulate_upload(id: &str, later: &mut Vec<crate::mock_backend_ext::Later>) {
    for (ms, fraction) in UPLOAD_STEPS {
        let id = id.to_owned();
        later.push((
            std::time::Duration::from_millis(ms),
            Box::new(move |state: &mut ViewState| {
                let Some(at) = state.uploads.iter().position(|u| u.id == id) else {
                    return;
                };
                if matches!(state.uploads[at].status, UploadStatus::Failed(_)) {
                    return;
                }
                if fraction < 1.0 {
                    state.uploads[at].status = UploadStatus::Uploading {
                        fraction,
                        label: format!("{}%", (fraction * 100.0) as u32),
                    };
                    return;
                }
                let upload = &mut state.uploads[at];
                upload.status = UploadStatus::Ready;
                upload.sha256 = Some(
                    BlobTransfer::hash_bytes(
                        upload
                            .local_path
                            .clone()
                            .unwrap_or_else(|| upload.name.clone())
                            .as_bytes(),
                    )
                    .to_string(),
                );
                if let Some(UploadTarget::ChannelFiles { channel_id, folder }) =
                    upload.target.clone()
                {
                    let upload = state.uploads.remove(at);
                    let path = crate::files::join_path(&folder, &upload.name);
                    state
                        .channel_files
                        .retain(|f| !(f.channel_id == channel_id && f.path == path));
                    state.channel_files.push(ChannelFileView {
                        channel_id,
                        folder: folder.clone(),
                        name: upload.name.clone(),
                        path,
                        sha256: upload.sha256.clone().unwrap_or_default(),
                        size: upload.size,
                        image: upload.image,
                        mime: upload.mime.clone(),
                        size_label: format::file_size_label(upload.size),
                        date_label: "Today".into(),
                        updated_at: format::now_ms(),
                        task_id: None,
                        local_path: upload.local_path,
                    });
                }
            }),
        ));
    }
}

/// The local copy of a blob that the mock knows, by its hash.
fn known_local(state: &ViewState, sha256: &str) -> Option<(String, String)> {
    let from_files = state
        .channel_files
        .iter()
        .find(|f| f.sha256 == sha256)
        .and_then(|f| f.local_path.clone().map(|p| (p, f.name.clone())));
    let from_messages = || {
        state
            .timelines
            .iter()
            .flat_map(|t| t.items.iter())
            .filter_map(|i| match i {
                TimelineItem::Message(m) => Some(m.attachments.iter()),
                _ => None,
            })
            .flatten()
            .find(|a| a.sha256 == sha256)
            .and_then(|a| a.local_path.clone().map(|p| (p, a.name.clone())))
    };
    let from_threads = || {
        state
            .threads
            .iter()
            .flat_map(|t| t.posts.iter())
            .flat_map(|p| p.bodies.iter())
            .filter_map(|b| match b {
                PostBody::Attachments(list) => Some(list.iter()),
                _ => None,
            })
            .flatten()
            .find(|a| a.sha256 == sha256)
            .and_then(|a| a.local_path.clone().map(|p| (p, a.name.clone())))
    };
    from_files.or_else(from_messages).or_else(from_threads)
}

/// The attachments of a send in the mock: the uploads `ids`, which must be
/// ready. They leave the upload list.
pub(crate) fn take_ready(
    state: &mut ViewState,
    ids: &[String],
) -> Result<Vec<AttachmentView>, crate::ClientError> {
    let mut out = Vec::new();
    for id in ids {
        let Some(upload) = state.uploads.iter().find(|u| &u.id == id) else {
            return Err(invalid(
                "A file of this message is not attached any more. Attach it again, then send",
            ));
        };
        if let UploadStatus::Failed(_) = upload.status {
            return Err(invalid(&format!(
                "The upload of {} failed. Choose Try again on the file, or remove it, then send again",
                upload.name
            )));
        }
        out.push(AttachmentView {
            sha256: upload.sha256.clone().unwrap_or_default(),
            name: upload.name.clone(),
            mime: upload.mime.clone(),
            size: upload.size,
            image: upload.image,
            detail: upload.detail.clone(),
            size_label: format::file_size_label(upload.size),
            local_path: upload.local_path.clone(),
            artifact: None,
        });
    }
    state.uploads.retain(|u| !ids.contains(&u.id));
    Ok(out)
}

/// Whether a send with the uploads `ids` can stop waiting: they all left
/// Preparing and Uploading, or one of them failed, as on the hub.
pub(crate) fn uploads_settled(state: &ViewState, ids: &[String]) -> bool {
    let mine = || state.uploads.iter().filter(|u| ids.contains(&u.id));
    mine().any(|u| matches!(u.status, UploadStatus::Failed(_)))
        || mine().all(|u| matches!(u.status, UploadStatus::Ready))
}

/// The file commands in the mock.
pub(crate) fn apply(
    state: &mut ViewState,
    command: crate::Command,
    next_id: &mut u64,
    later: &mut Vec<crate::mock_backend_ext::Later>,
) -> Result<crate::CommandOutput, crate::ClientError> {
    use crate::{AttachInput, Command, CommandOutput};
    match command {
        Command::Attach { target, inputs } => {
            let mut ids = Vec::new();
            for input in inputs {
                *next_id += 1;
                let id = format!("upl_mock_{}", *next_id + 100);
                let (name, path, size, blob) = match input {
                    AttachInput::Path(path) => {
                        let name = std::path::Path::new(&path)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.clone());
                        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                        (name, Some(path), size, None)
                    }
                    AttachInput::Pasted { name, bytes, .. } => {
                        let dir = folder().join("pasted");
                        let file = dir.join(format!("{id}-{name}"));
                        let path = std::fs::create_dir_all(&dir)
                            .and_then(|()| std::fs::write(&file, bytes.0.as_slice()))
                            .ok()
                            .map(|()| file.display().to_string());
                        (name, path, bytes.0.len() as u64, None)
                    }
                    AttachInput::Blob(blob) => {
                        let local = known_local(state, &blob.sha256.to_string()).map(|(p, _)| p);
                        (blob.name.clone(), local, blob.size, Some(blob))
                    }
                };
                let mime = blob
                    .as_ref()
                    .map(|b| b.mime.clone())
                    .unwrap_or_else(|| mime_for(&name));
                let too_large = size > crate::blob::MAX_BLOB_BYTES;
                state.uploads.push(UploadView {
                    id: id.clone(),
                    target: Some(target.clone()),
                    image: is_image(&mime),
                    detail: file_detail(&mime, &name, size),
                    local_path: path,
                    sha256: blob.as_ref().map(|b| b.sha256.to_string()),
                    size,
                    mime,
                    name,
                    status: if too_large {
                        UploadStatus::Failed(format!(
                            "Upload failed. {}.",
                            crate::blob::BlobError::TooLarge { size }
                        ))
                    } else if blob.is_some() {
                        UploadStatus::Ready
                    } else {
                        UploadStatus::Preparing
                    },
                });
                if !too_large && blob.is_none() {
                    simulate_upload(&id, later);
                }
                ids.push(id);
            }
            Ok(CommandOutput::Attached(ids))
        }
        Command::RetryUpload { id } => {
            let upload = state
                .uploads
                .iter_mut()
                .find(|u| u.id == id)
                .ok_or_else(|| invalid("This upload is not there any more."))?;
            if !matches!(upload.status, UploadStatus::Failed(_)) {
                return Err(invalid("This upload did not fail."));
            }
            upload.status = UploadStatus::Preparing;
            simulate_upload(&id, later);
            Ok(CommandOutput::Done)
        }
        Command::RemoveUpload { id } => {
            state.uploads.retain(|u| u.id != id);
            Ok(CommandOutput::Done)
        }
        Command::LoadChannelFiles { channel_id } => {
            if !state.files_loaded.contains(&channel_id) {
                state.files_loaded.push(channel_id);
            }
            Ok(CommandOutput::Done)
        }
        Command::MoveChannelFile {
            channel_id,
            from_path,
            to_path,
        } => {
            let to_path = to_path.trim().trim_matches('/').to_owned();
            if to_path.is_empty() {
                return Err(invalid("Enter a name for the file."));
            }
            if to_path != from_path
                && state
                    .channel_files
                    .iter()
                    .any(|f| f.channel_id == channel_id && f.path == to_path)
            {
                return Err(invalid(
                    "A file with this name is in that folder already. Choose another name.",
                ));
            }
            let file = state
                .channel_files
                .iter_mut()
                .find(|f| f.channel_id == channel_id && f.path == from_path)
                .ok_or_else(|| invalid("This file is not there any more."))?;
            let (folder, name) = split_path(&to_path);
            file.folder = folder.to_owned();
            file.name = name.to_owned();
            file.path = to_path;
            Ok(CommandOutput::Done)
        }
        Command::DeleteChannelFile { channel_id, path } => {
            state
                .channel_files
                .retain(|f| !(f.channel_id == channel_id && f.path == path));
            Ok(CommandOutput::Done)
        }
        Command::SaveToChannelFiles {
            channel_id,
            path,
            blob,
        } => {
            let (folder, name) = split_path(&path);
            let sha = blob.sha256.to_string();
            let local = known_local(state, &sha).map(|(p, _)| p);
            state
                .channel_files
                .retain(|f| !(f.channel_id == channel_id && f.path == path));
            state.channel_files.push(ChannelFileView {
                channel_id,
                folder: folder.to_owned(),
                name: name.to_owned(),
                path: path.clone(),
                sha256: sha,
                size: blob.size,
                image: is_image(&blob.mime),
                mime: blob.mime,
                size_label: format::file_size_label(blob.size),
                date_label: "Today".into(),
                updated_at: format::now_ms(),
                task_id: None,
                local_path: local,
            });
            Ok(CommandOutput::Done)
        }
        Command::FetchBlob { sha256, name, .. } => {
            let (path, known_name) = known_local(state, &sha256).ok_or_else(|| {
                invalid("This demo file has no copy on this computer. Connect Consort to a hub to download files.")
            })?;
            let Some(name) = name else {
                return Ok(CommandOutput::LocalFile(path));
            };
            let name = if name.is_empty() { known_name } else { name };
            let dir = folder().join("open").join(&sha256);
            let target = dir.join(&name);
            std::fs::create_dir_all(&dir)
                .and_then(|()| std::fs::copy(&path, &target).map(|_| ()))
                .map_err(|e| crate::ClientError::Storage {
                    path: target.display().to_string(),
                    message: e.to_string(),
                })?;
            Ok(CommandOutput::LocalFile(target.display().to_string()))
        }
        Command::SaveBlob { sha256, to, .. } => {
            let (path, _) = known_local(state, &sha256).ok_or_else(|| {
                invalid("This demo file has no copy on this computer. Connect Consort to a hub to download files.")
            })?;
            std::fs::copy(&path, &to).map_err(|e| crate::ClientError::Storage {
                path: to.clone(),
                message: e.to_string(),
            })?;
            Ok(CommandOutput::Done)
        }
        _ => Ok(CommandOutput::Done),
    }
}

/// A small skill zip for the Add skill dialog of the mock. The mock only
/// reads its name and size.
pub fn skill_zip_path() -> Option<String> {
    let dir = folder();
    let path = dir.join("porch-lights-skill.zip");
    if !path.exists() {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&path, b"PK\x05\x06 Consort mock skill").ok()?;
    }
    Some(path.display().to_string())
}

/// The reply box of the roofline task in the design "Task thread reply
/// with an attachment": one picture.
pub fn reply_upload() -> UploadView {
    upload(
        "upl_mock_reply",
        UploadTarget::Reply {
            task_id: ROOF_TASK.into(),
        },
        "garage-side.jpg",
        UploadStatus::Ready,
    )
}

/// The mock for a screen of the phase 1 designs, by its `--screen` name.
/// `None` for a screen that uses the plain mock.
pub fn screen_state(screen: &str) -> Option<ViewState> {
    let base = || crate::mock::view_state_with_files();
    match screen {
        "attachments" => {
            let mut state = base();
            state.uploads = composer_uploads();
            Some(state)
        }
        "thread-attach" => {
            let mut state = base();
            state.uploads = vec![reply_upload()];
            Some(state)
        }
        "files" | "files-menu" | "files-drop" | "files-upload" => {
            let mut state = base();
            if screen == "files-upload" || screen == "files" || screen == "files-menu" {
                state.uploads = vec![files_upload()];
            }
            Some(state)
        }
        "drop" | "bubbles" | "lightbox" | "files-empty" | "add-skill-upload" => Some(base()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_files_have_stable_hashes_and_local_copies() {
        assert_eq!(blob("wiring-plan.pdf"), blob("wiring-plan.pdf"));
        assert_ne!(blob("wiring-plan.pdf").sha256, blob("porch.jpg").sha256);
        let path = local_path("roofline-front.jpg").expect("the temp folder takes it");
        assert!(std::fs::read_to_string(path).unwrap().starts_with("<svg"));
        let files = channel_files();
        assert_eq!(files.iter().filter(|f| f.is_artifact()).count(), 3);
        assert!(files.iter().all(|f| f.local_path.is_some()));
    }
}
