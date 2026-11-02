//! Files on the hub backend: the HTTP token, uploads with their progress,
//! downloads into the local blob cache, and the channel files.
//!
//! An upload runs as its own task. It hashes the file, asks the hub whether
//! it has the blob, sends it when it does not, and records each step in
//! [`Shared::uploads`]. A send waits for its uploads with
//! [`Shared::wait_for_uploads`]. An upload to a folder of channel files
//! calls `files.put` when the blob is on the hub, then leaves the list.

use std::{path::PathBuf, sync::Arc, time::Duration};

use consort_proto::{
    auth::AuthHttpToken,
    client_api::{FilesDelete, FilesList, FilesMove, FilesPut},
    model::{BlobRef, Sha256Hash},
};

use super::{Shared, invalid};
use crate::{
    AttachInput, ClientError,
    blob::{BlobError, BlobTransfer, MAX_BLOB_BYTES, Progress},
    files::join_path,
    format,
    uploads::{SendGate, UploadSource, UploadState, UploadTarget, mime_for},
};

/// A token is used until this long before it ends, so a long upload does
/// not start with a token that ends on the way.
const TOKEN_MARGIN_MS: i64 = 60_000;

/// How often a send looks at its uploads while it waits.
const UPLOAD_POLL: Duration = Duration::from_millis(100);

/// The message of a send that cannot go because an upload failed.
fn upload_failed(name: &str) -> ClientError {
    ClientError::Hub {
        code: "invalid".into(),
        message: format!(
            "The upload of {name} failed. Choose Try again on the file, or remove it, then send again"
        ),
    }
}

impl Shared {
    /// The transfer for the paired hub, with the local blob cache.
    fn transfer(&self) -> Result<BlobTransfer, ClientError> {
        let identity = self.identity().ok_or(ClientError::NotConnected)?;
        Ok(BlobTransfer::pinned(
            &identity.config.hub_url,
            identity.config.tls_sha256.as_ref(),
            Some(self.blob_cache_dir()),
        )?)
    }

    /// The folder of the local blob cache: `blobs` in the client's folder,
    /// or a folder in the temp folder for a client with no folder.
    pub(crate) fn blob_cache_dir(&self) -> PathBuf {
        match &self.store {
            Some(store) => store.dir().join("blobs"),
            None => std::env::temp_dir().join(format!("consort-blobs-{}", std::process::id())),
        }
    }

    /// A bearer token for the blob endpoints, from the cache while it is
    /// good, else a new one from `auth.http_token`.
    async fn http_token(&self, fresh: bool) -> Result<String, ClientError> {
        let now = format::now_ms();
        if !fresh
            && let Ok(token) = self.http_token.lock()
            && let Some((token, expires)) = token.as_ref()
            && *expires - TOKEN_MARGIN_MS > now
        {
            return Ok(token.clone());
        }
        let token = self.call(AuthHttpToken {}).await?;
        if let Ok(mut cached) = self.http_token.lock() {
            *cached = Some((token.token.clone(), token.expires_at.as_millis()));
        }
        Ok(token.token)
    }

    /// Runs `transfer` with a token. When the hub does not accept the token,
    /// it gets a new one and tries once more.
    async fn with_token<T, F, Fut>(&self, transfer: F) -> Result<T, ClientError>
    where
        F: Fn(String) -> Fut,
        Fut: std::future::Future<Output = Result<T, BlobError>>,
    {
        let token = self.http_token(false).await?;
        match transfer(token).await {
            Err(BlobError::Unauthorized) => {
                let token = self.http_token(true).await?;
                Ok(transfer(token).await?)
            }
            other => Ok(other?),
        }
    }

    /// Adds uploads for `inputs` and starts them. Returns their IDs.
    pub(crate) fn attach(
        self: &Arc<Self>,
        target: UploadTarget,
        inputs: Vec<AttachInput>,
    ) -> Vec<String> {
        let mut ids = Vec::new();
        for input in inputs {
            let (source, name, mime, size, early_failure) = match input {
                AttachInput::Path(path) => {
                    let name = std::path::Path::new(&path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.clone());
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    let mime = mime_for(&name);
                    (UploadSource::Path(path), name, mime, size, None)
                }
                AttachInput::Pasted { name, mime, bytes } => {
                    let size = bytes.0.len() as u64;
                    match self.keep_pasted(&name, &bytes.0) {
                        Ok(path) => (UploadSource::Path(path), name, mime, size, None),
                        Err(err) => (
                            UploadSource::Path(String::new()),
                            name,
                            mime,
                            size,
                            Some(err.to_string()),
                        ),
                    }
                }
                AttachInput::Blob(blob) => {
                    let (name, mime, size) = (blob.name.clone(), blob.mime.clone(), blob.size);
                    (UploadSource::Blob(blob), name, mime, size, None)
                }
            };
            let (id, start) = {
                let Ok(mut uploads) = self.uploads.lock() else {
                    continue;
                };
                let id = uploads.add(target.clone(), source, name, mime, size);
                if let Some(message) = early_failure {
                    uploads.fail(&id, 1, message);
                }
                let start = uploads
                    .get(&id)
                    .is_some_and(|u| matches!(u.state, UploadState::Preparing));
                (id, start)
            };
            if start {
                self.start_upload(id.clone(), 1);
            } else if matches!(target, UploadTarget::ChannelFiles { .. }) {
                // A blob from the hub goes into the folder at once.
                self.start_upload(id.clone(), 1);
            }
            ids.push(id);
        }
        self.publish();
        ids
    }

    /// Writes pasted bytes to a file in the cache folder, so the upload
    /// and the thumbnail read them like any file.
    fn keep_pasted(&self, name: &str, bytes: &[u8]) -> Result<String, ClientError> {
        let dir = self.blob_cache_dir().join("pasted");
        let storage = |path: &std::path::Path, err: std::io::Error| ClientError::Storage {
            path: path.display().to_string(),
            message: err.to_string(),
        };
        std::fs::create_dir_all(&dir).map_err(|e| storage(&dir, e))?;
        let file = dir.join(format!("{}-{name}", uuid::Uuid::now_v7()));
        std::fs::write(&file, bytes).map_err(|e| storage(&file, e))?;
        Ok(file.display().to_string())
    }

    /// Starts the work of upload `id` at `attempt` on its own task.
    pub(crate) fn start_upload(self: &Arc<Self>, id: String, attempt: u32) {
        let shared = self.clone();
        tokio::spawn(async move {
            let result = shared.run_upload(&id, attempt).await;
            if let Err(err) = result {
                let message = upload_message(&err);
                if let Ok(mut uploads) = shared.uploads.lock() {
                    uploads.fail(&id, attempt, message);
                }
            }
            shared.publish();
        });
    }

    async fn run_upload(self: &Arc<Self>, id: &str, attempt: u32) -> Result<(), ClientError> {
        let Some(upload) = self.uploads.lock().ok().and_then(|u| u.get(id).cloned()) else {
            return Ok(());
        };
        let blob = match &upload.source {
            UploadSource::Blob(blob) => blob.clone(),
            UploadSource::Path(path) => {
                let path = PathBuf::from(path);
                let size = std::fs::metadata(&path)
                    .map_err(|e| ClientError::Storage {
                        path: path.display().to_string(),
                        message: e.to_string(),
                    })?
                    .len();
                if size > MAX_BLOB_BYTES {
                    return Err(BlobError::TooLarge { size }.into());
                }
                let (sha, size) = BlobTransfer::hash_file(&path, |_| {}).await?;
                let transfer = self.transfer()?;
                let progress = {
                    let shared = self.clone();
                    let id = id.to_owned();
                    move |p: Progress| {
                        if let Ok(mut uploads) = shared.uploads.lock()
                            && uploads.progress(&id, attempt, p.done, p.total)
                        {
                            drop(uploads);
                            shared.publish();
                        }
                    }
                };
                let progress = Arc::new(progress);
                self.with_token(|token| {
                    let transfer = transfer.clone();
                    let path = path.clone();
                    let sha = sha.clone();
                    let progress = progress.clone();
                    async move {
                        transfer
                            .upload_file_if_missing(&token, &path, &sha, size, move |p| progress(p))
                            .await
                    }
                })
                .await?;
                // The file that went up is in the blob cache too, so a
                // thumbnail of the sent message needs no download.
                if let Ok(mut cache) = self.cache.lock() {
                    cache
                        .local_blobs
                        .entry(sha.to_string())
                        .or_insert_with(|| path.display().to_string());
                }
                BlobRef {
                    sha256: sha,
                    name: upload.name.clone(),
                    mime: upload.mime.clone(),
                    size,
                }
            }
        };
        let finished = self.uploads.lock().is_ok_and(|mut u| {
            u.finish(id, attempt, blob.sha256.clone(), blob.size)
                || matches!(upload.source, UploadSource::Blob(_))
        });
        if !finished {
            return Ok(());
        }
        // An upload to the channel files becomes a file there.
        if let UploadTarget::ChannelFiles { channel_id, folder } = &upload.target {
            let asked_at = self.cache.lock().map(|c| c.seq).unwrap_or(0);
            let put = FilesPut {
                channel_id: super::commands::parse_id(channel_id, "channel")?,
                path: join_path(folder, &blob.name),
                blob_sha256: blob.sha256.clone(),
                size: blob.size,
                mime: blob.mime.clone(),
            };
            let stored = self.call(put).await?;
            if let Ok(mut cache) = self.cache.lock() {
                cache.merge_file(stored.file, asked_at);
            }
            if let Ok(mut uploads) = self.uploads.lock() {
                uploads.remove(id);
            }
        }
        Ok(())
    }

    /// Starts a failed upload again.
    pub(crate) fn retry_upload(self: &Arc<Self>, id: &str) -> Result<(), ClientError> {
        let attempt = self
            .uploads
            .lock()
            .ok()
            .and_then(|mut u| u.retry(id))
            .ok_or_else(|| invalid("This upload is not there any more."))?;
        self.start_upload(id.to_owned(), attempt);
        self.publish();
        Ok(())
    }

    pub(crate) fn remove_upload(&self, id: &str) {
        if let Ok(mut uploads) = self.uploads.lock() {
            uploads.remove(id);
        }
        self.publish();
    }

    /// Waits until the uploads `ids` of `target` are on the hub, and gives
    /// their blobs. A failed upload or one that is gone is an error.
    pub(crate) async fn wait_for_uploads(
        &self,
        target: &UploadTarget,
        ids: &[String],
    ) -> Result<Vec<BlobRef>, ClientError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        loop {
            let gate = {
                let uploads = self
                    .uploads
                    .lock()
                    .map_err(|_| ClientError::Other("The upload list is not usable".into()))?;
                if let Some(missing) = ids.iter().find(|id| uploads.get(id).is_none()) {
                    let _ = missing;
                    return Err(invalid(
                        "A file of this message is not attached any more. Attach it again, then send",
                    ));
                }
                uploads.gate(target, Some(ids))
            };
            match gate {
                SendGate::Ready(blobs) => return Ok(blobs),
                SendGate::Failed(name) => return Err(upload_failed(&name)),
                SendGate::Waiting => tokio::time::sleep(UPLOAD_POLL).await,
            }
        }
    }

    /// Takes the uploads of a send away after the send went out.
    pub(crate) fn take_uploads(&self, ids: &[String]) {
        if let Ok(mut uploads) = self.uploads.lock() {
            uploads.take(ids);
        }
        self.publish();
    }

    /// Downloads a blob into the local cache and records it.
    pub(crate) async fn fetch_blob(&self, sha256: &str, size: u64) -> Result<PathBuf, ClientError> {
        let sha: Sha256Hash = sha256
            .parse()
            .map_err(|_| invalid("This file has a hash that is not valid."))?;
        let transfer = self.transfer()?;
        let path = match transfer.cached(&sha) {
            Some(path) => path,
            None => {
                self.with_token(|token| {
                    let transfer = transfer.clone();
                    let sha = sha.clone();
                    async move { transfer.download(&token, &sha, Some(size), |_| {}).await }
                })
                .await?
            }
        };
        if let Ok(mut cache) = self.cache.lock() {
            cache
                .local_blobs
                .insert(sha.to_string(), path.display().to_string());
        }
        self.publish();
        Ok(path)
    }

    /// A copy of a cached blob with its own file name, so another app can
    /// open it: `<cache>/open/<sha256>/<name>`.
    pub(crate) fn named_copy(
        &self,
        sha256: &str,
        cached: &std::path::Path,
        name: &str,
    ) -> Result<PathBuf, ClientError> {
        let name = std::path::Path::new(name)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| sha256.to_owned());
        let dir = self.blob_cache_dir().join("open").join(sha256);
        let target = dir.join(name);
        let storage = |path: &std::path::Path, err: std::io::Error| ClientError::Storage {
            path: path.display().to_string(),
            message: err.to_string(),
        };
        if !target.exists() {
            std::fs::create_dir_all(&dir).map_err(|e| storage(&dir, e))?;
            std::fs::copy(cached, &target).map_err(|e| storage(&target, e))?;
        }
        Ok(target)
    }

    /// The source of a skill for `skill.preview` and `skill.install`. A zip
    /// on this computer goes to the hub first, through the blob upload. The
    /// hub keeps a blob that it has, so a preview then an install sends the
    /// file once.
    pub(crate) async fn skill_source(
        &self,
        input: &crate::skills::SkillInstallInput,
    ) -> Result<consort_proto::client_api::SkillInstallSource, ClientError> {
        use crate::extensions_view::{UploadedZip, check_skill_zip, install_source};
        let crate::skills::SkillInstallInput::Upload { file_path } = input else {
            return install_source(input, None);
        };
        let path = PathBuf::from(file_path);
        let size = std::fs::metadata(&path)
            .map_err(|e| ClientError::Storage {
                path: file_path.clone(),
                message: e.to_string(),
            })?
            .len();
        check_skill_zip(file_path, size)?;
        let (sha, size) = BlobTransfer::hash_file(&path, |_| {}).await?;
        let transfer = self.transfer()?;
        self.with_token(|token| {
            let transfer = transfer.clone();
            let path = path.clone();
            let sha = sha.clone();
            async move {
                transfer
                    .upload_file_if_missing(&token, &path, &sha, size, |_| {})
                    .await
            }
        })
        .await?;
        install_source(input, Some(&UploadedZip { sha256: sha, size }))
    }

    /// `files.list`: the files of one channel.
    pub(crate) async fn load_files(&self, channel_id: &str) -> Result<(), ClientError> {
        let asked_at = self.cache.lock().map(|c| c.seq).unwrap_or(0);
        let list = self
            .call(FilesList {
                channel_id: super::commands::parse_id(channel_id, "channel")?,
                prefix: None,
            })
            .await?;
        if let Ok(mut cache) = self.cache.lock() {
            cache.load_files(channel_id, list.files, asked_at);
        }
        self.publish();
        Ok(())
    }

    pub(crate) async fn move_file(
        &self,
        channel_id: &str,
        from_path: String,
        to_path: String,
    ) -> Result<(), ClientError> {
        let to_path = to_path.trim().trim_matches('/').to_owned();
        if to_path.is_empty() {
            return Err(invalid("Enter a name for the file."));
        }
        if to_path == from_path {
            return Ok(());
        }
        let asked_at = self.cache.lock().map(|c| c.seq).unwrap_or(0);
        let moved = self
            .call(FilesMove {
                channel_id: super::commands::parse_id(channel_id, "channel")?,
                from_path: from_path.clone(),
                to_path,
            })
            .await?;
        if let Ok(mut cache) = self.cache.lock() {
            cache
                .channel_files
                .remove(&format!("{channel_id}:{from_path}"));
            cache.merge_file(moved.file, asked_at);
        }
        self.publish();
        Ok(())
    }

    pub(crate) async fn delete_file(
        &self,
        channel_id: &str,
        path: String,
    ) -> Result<(), ClientError> {
        self.call(FilesDelete {
            channel_id: super::commands::parse_id(channel_id, "channel")?,
            path: path.clone(),
        })
        .await?;
        if let Ok(mut cache) = self.cache.lock() {
            cache.channel_files.remove(&format!("{channel_id}:{path}"));
        }
        self.publish();
        Ok(())
    }

    pub(crate) async fn put_file(
        &self,
        channel_id: &str,
        path: String,
        blob: BlobRef,
    ) -> Result<(), ClientError> {
        let asked_at = self.cache.lock().map(|c| c.seq).unwrap_or(0);
        let stored = self
            .call(FilesPut {
                channel_id: super::commands::parse_id(channel_id, "channel")?,
                path,
                blob_sha256: blob.sha256,
                size: blob.size,
                mime: blob.mime,
            })
            .await?;
        if let Ok(mut cache) = self.cache.lock() {
            cache.merge_file(stored.file, asked_at);
        }
        self.publish();
        Ok(())
    }
}

/// What a failed upload chip says: what happened and what to do next.
fn upload_message(err: &ClientError) -> String {
    match err {
        ClientError::NotConnected => "Upload failed. The hub is offline.".to_owned(),
        ClientError::TimedOut => "Upload failed. The hub did not answer in time.".to_owned(),
        other => {
            let text = other.to_string();
            let text = text.trim_end_matches('.');
            format!("Upload failed. {text}.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_upload_says_what_happened() {
        assert_eq!(
            upload_message(&ClientError::NotConnected),
            "Upload failed. The hub is offline."
        );
        assert_eq!(
            upload_message(
                &BlobError::TooLarge {
                    size: 300 * 1024 * 1024
                }
                .into()
            ),
            "Upload failed. This file is 300 MB. The limit is 256 MB. Choose a smaller file."
        );
    }
}
