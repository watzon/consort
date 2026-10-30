//! Files as the views show them: the attachments of a message, and the
//! files of a channel with their folders and the artifacts of each task.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    format,
    uploads::{file_detail, is_image},
    view::TaskStatus,
};

/// The folder that holds the artifacts of the tasks: `artifacts/<task-id>/`.
pub const ARTIFACTS_FOLDER: &str = "artifacts";

pub use consort_proto::model::{BlobRef, Sha256Hash};

/// The task that published an artifact.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArtifactTask {
    pub task_id: String,
    pub task_name: String,
    pub status: TaskStatus,
}

/// A file attached to a message or a thread post.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AttachmentView {
    pub sha256: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
    /// The app can draw it as a picture.
    pub image: bool,
    /// "PDF, 2.4 MB".
    pub detail: String,
    /// "2.4 MB".
    pub size_label: String,
    /// The copy in the local cache, once it is there.
    pub local_path: Option<String>,
    /// Set when the file is an artifact of a task in the channel files.
    pub artifact: Option<ArtifactTask>,
}

impl AttachmentView {
    /// The blob of this attachment, for a request that names it. `None`
    /// when the hash is not valid.
    pub fn blob(&self) -> Option<BlobRef> {
        Some(BlobRef {
            sha256: self.sha256.parse().ok()?,
            name: self.name.clone(),
            mime: self.mime.clone(),
            size: self.size,
        })
    }
}

/// One file of a channel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelFileView {
    pub channel_id: String,
    /// The path in the channel's folder, such as `photos/roof.jpg`.
    pub path: String,
    /// The last part of the path.
    pub name: String,
    /// The folder part of the path, empty for the top folder.
    pub folder: String,
    pub sha256: String,
    pub size: u64,
    pub mime: String,
    pub image: bool,
    pub size_label: String,
    /// "Today" or "Oct 24".
    pub date_label: String,
    pub updated_at: i64,
    /// The task that published it, for an artifact.
    pub task_id: Option<String>,
    pub local_path: Option<String>,
}

impl ChannelFileView {
    /// Whether this file is an artifact of a task.
    pub fn is_artifact(&self) -> bool {
        self.task_id.is_some() || self.path.starts_with(&format!("{ARTIFACTS_FOLDER}/"))
    }

    /// The attachment that names this file, for a message or a reply.
    pub fn blob(&self) -> Option<BlobRef> {
        Some(BlobRef {
            sha256: self.sha256.parse().ok()?,
            name: self.name.clone(),
            mime: self.mime.clone(),
            size: self.size,
        })
    }
}

/// A folder in a listing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FolderView {
    pub path: String,
    pub name: String,
    /// "4 files".
    pub count_label: String,
    /// The date of the newest file in it.
    pub date_label: String,
}

/// The artifacts of one task.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArtifactGroupView {
    pub task: ArtifactTask,
    pub files: Vec<ChannelFileView>,
}

/// What the Files tab shows for one folder.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilesListing {
    /// The folder, empty for the top folder.
    pub folder: String,
    pub folders: Vec<FolderView>,
    pub files: Vec<ChannelFileView>,
    /// Only in the top folder: the artifacts, one group per task.
    pub artifacts: Vec<ArtifactGroupView>,
    /// "9 files, 14.2 MB" for the whole channel.
    pub summary: String,
    /// The channel has no files at all.
    pub empty: bool,
}

/// Splits a path into its folder and its name.
pub fn split_path(path: &str) -> (&str, &str) {
    match path.rsplit_once('/') {
        Some((folder, name)) => (folder, name),
        None => ("", path),
    }
}

/// Joins a folder and a name into a path.
pub fn join_path(folder: &str, name: &str) -> String {
    let folder = folder.trim_matches('/');
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The listing of `folder` in the files of one channel. `tasks` names the
/// tasks of the artifacts: (task ID, name, status).
pub fn listing(
    files: &[ChannelFileView],
    channel_id: &str,
    folder: &str,
    tasks: &[(String, String, TaskStatus)],
    now_ms: i64,
) -> FilesListing {
    let folder = folder.trim_matches('/');
    let channel: Vec<&ChannelFileView> = files
        .iter()
        .filter(|f| f.channel_id == channel_id)
        .collect();
    let total: u64 = channel.iter().map(|f| f.size).sum();
    let summary = format!(
        "{}, {}",
        plural(channel.len(), "file", "files"),
        format::file_size_label(total)
    );
    let prefix = if folder.is_empty() {
        String::new()
    } else {
        format!("{folder}/")
    };
    let mut folders: BTreeMap<String, (usize, i64)> = BTreeMap::new();
    let mut here = Vec::new();
    let mut artifacts: BTreeMap<String, Vec<ChannelFileView>> = BTreeMap::new();
    for file in &channel {
        // The artifacts are their own section of the top folder.
        if file.is_artifact() {
            if folder.is_empty() {
                let task = file.task_id.clone().unwrap_or_else(|| {
                    file.path
                        .split('/')
                        .nth(1)
                        .map(str::to_owned)
                        .unwrap_or_default()
                });
                artifacts.entry(task).or_default().push((*file).clone());
            }
            continue;
        }
        let Some(rest) = file.path.strip_prefix(&prefix) else {
            continue;
        };
        match rest.split_once('/') {
            Some((sub, _)) => {
                let entry = folders.entry(sub.to_owned()).or_insert((0, 0));
                entry.0 += 1;
                entry.1 = entry.1.max(file.updated_at);
            }
            None => here.push((*file).clone()),
        }
    }
    here.sort_by_key(|f| f.name.to_lowercase());
    let folders = folders
        .into_iter()
        .map(|(name, (count, newest))| FolderView {
            path: join_path(folder, &name),
            count_label: plural(count, "file", "files"),
            date_label: format::file_date_label(newest, now_ms),
            name,
        })
        .collect();
    let task_names: BTreeMap<&str, (&str, TaskStatus)> = tasks
        .iter()
        .map(|(id, name, status)| (id.as_str(), (name.as_str(), *status)))
        .collect();
    let mut groups: Vec<ArtifactGroupView> = artifacts
        .into_iter()
        .map(|(task_id, mut files)| {
            files.sort_by_key(|f| f.name.to_lowercase());
            let (task_name, status) = task_names
                .get(task_id.as_str())
                .map(|(n, s)| ((*n).to_owned(), *s))
                .unwrap_or_else(|| ("A task that is gone".to_owned(), TaskStatus::Resolved));
            ArtifactGroupView {
                task: ArtifactTask {
                    task_id,
                    task_name,
                    status,
                },
                files,
            }
        })
        .collect();
    // The newest work first.
    groups.sort_by_key(|g| {
        std::cmp::Reverse(g.files.iter().map(|f| f.updated_at).max().unwrap_or(0))
    });
    FilesListing {
        folder: folder.to_owned(),
        folders,
        files: here,
        artifacts: groups,
        summary,
        empty: channel.is_empty(),
    }
}

/// The folders of a channel, for the Move dialog: every folder that holds a
/// file, and the top folder (empty), sorted.
pub fn folders_of(files: &[ChannelFileView], channel_id: &str) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    out.insert(String::new());
    for file in files
        .iter()
        .filter(|f| f.channel_id == channel_id && !f.is_artifact())
    {
        let mut folder = file.folder.as_str();
        while !folder.is_empty() {
            out.insert(folder.to_owned());
            folder = split_path(folder).0;
        }
    }
    out.into_iter().collect()
}

/// The view of a blob that a message names. `files` are the channel files,
/// to find an artifact by its hash. `local` maps hashes to cached files.
pub fn attachment_view(
    blob: &consort_proto::model::BlobRef,
    artifact: Option<ArtifactTask>,
    local_path: Option<String>,
) -> AttachmentView {
    AttachmentView {
        sha256: blob.sha256.to_string(),
        name: blob.name.clone(),
        mime: blob.mime.clone(),
        size: blob.size,
        image: is_image(&blob.mime),
        detail: file_detail(&blob.mime, &blob.name, blob.size),
        size_label: format::file_size_label(blob.size),
        local_path,
        artifact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64, task: Option<&str>, at: i64) -> ChannelFileView {
        let (folder, name) = split_path(path);
        ChannelFileView {
            channel_id: "chn_1".into(),
            path: path.into(),
            name: name.into(),
            folder: folder.into(),
            size,
            updated_at: at,
            task_id: task.map(str::to_owned),
            ..ChannelFileView::default()
        }
    }

    #[test]
    fn the_top_folder_lists_folders_files_and_artifacts_by_task() {
        let files = vec![
            file("photos/front.jpg", 1024, None, 10),
            file("photos/side.jpg", 1024, None, 20),
            file("quotes/a.pdf", 1024, None, 5),
            file("wiring-plan.pdf", 1024, None, 1),
            file("Brightline.pdf", 1024, None, 1),
            file(
                "artifacts/tsk_1/quote-comparison.xlsx",
                1024,
                Some("tsk_1"),
                30,
            ),
            file(
                "artifacts/tsk_2/switch-shortlist.md",
                1024,
                Some("tsk_2"),
                3,
            ),
            ChannelFileView {
                channel_id: "chn_other".into(),
                ..file("elsewhere.txt", 1, None, 1)
            },
        ];
        let tasks = vec![
            (
                "tsk_1".to_owned(),
                "Price out roofline lighting".to_owned(),
                TaskStatus::Review,
            ),
            (
                "tsk_2".to_owned(),
                "Pick smart switches".to_owned(),
                TaskStatus::Resolved,
            ),
        ];
        let top = listing(&files, "chn_1", "", &tasks, 0);
        assert_eq!(top.summary, "7 files, 7 KB");
        assert!(!top.empty);
        let folders: Vec<_> = top.folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(folders, ["photos", "quotes"]);
        assert_eq!(top.folders[0].count_label, "2 files");
        assert_eq!(top.folders[1].count_label, "1 file");
        let here: Vec<_> = top.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(here, ["Brightline.pdf", "wiring-plan.pdf"]);
        // The newest artifacts first, with the task name and status.
        assert_eq!(top.artifacts.len(), 2);
        assert_eq!(
            top.artifacts[0].task.task_name,
            "Price out roofline lighting"
        );
        assert_eq!(top.artifacts[0].task.status, TaskStatus::Review);
        assert_eq!(top.artifacts[1].files[0].name, "switch-shortlist.md");

        let photos = listing(&files, "chn_1", "photos", &tasks, 0);
        assert!(photos.folders.is_empty() && photos.artifacts.is_empty());
        assert_eq!(photos.files.len(), 2);

        let empty = listing(&files, "chn_none", "", &tasks, 0);
        assert!(empty.empty);
        assert_eq!(empty.summary, "0 files, 0 B");
    }

    #[test]
    fn the_move_dialog_offers_every_folder_with_a_file() {
        let files = vec![
            file("a/b/c.txt", 1, None, 1),
            file("d.txt", 1, None, 1),
            file("artifacts/tsk_1/x.txt", 1, Some("tsk_1"), 1),
        ];
        assert_eq!(folders_of(&files, "chn_1"), ["", "a", "a/b"]);
    }

    #[test]
    fn paths_split_and_join() {
        assert_eq!(split_path("a/b/c.txt"), ("a/b", "c.txt"));
        assert_eq!(split_path("c.txt"), ("", "c.txt"));
        assert_eq!(join_path("", "c.txt"), "c.txt");
        assert_eq!(join_path("a/b/", "c.txt"), "a/b/c.txt");
    }
}
