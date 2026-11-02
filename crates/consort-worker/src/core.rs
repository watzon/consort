//! The worker core: the link handler, hub request dispatch and the inventory.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use consort_link::http;
use consort_link::identity::Identity;
use consort_link::link::{Link, LinkHandler};
use consort_proto::auth::{Signature, WorkerHello, WorkerHelloResult};
use consort_proto::error::{ErrorBody, ErrorCode, HubVersions};
use consort_proto::execution_link::{
    ExecInventory, ExecSkillBundleGet, HubToExecution, WorkerUpdateAck, WorkspaceInspection,
    WorkspacePrepare, WorkspaceSync, WorkspaceSyncResult,
};
use consort_proto::frame::{Empty, Frame, RequestFrame, SUPPORTED_PROTOCOL_VERSIONS};
use consort_proto::ids::WorkerId;
use consort_proto::model::{Inventory, Os, Sha256Hash, SkillBundle, WorkerLoad};
use consort_proto::session_host_link::SkillBundleGet;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::WorkerError;
use crate::attachments::{BLOB_MAX_BYTES, BlobCache, BlobFetch, ChannelCopy, save_attachments};
use crate::config::{WORKSPACES_DIR, WorkerToml};
use crate::exec::{Exec, HubFiles};
use crate::journal::{Journal, StoredWorkspace};
use crate::process::shell;
use crate::self_update::{UpdateEvents, Updater};
use crate::{tags, workspace};

/// The folder of the skill cache, in the worker's config folder.
const SKILLS_DIR: &str = "cache/skills";

/// Shared worker state.
pub(crate) struct Core {
    pub config_dir: PathBuf,
    pub limits: Arc<crate::limits::Limits>,
    pub toml: WorkerToml,
    pub identity: Identity,
    pub worker_id: WorkerId,
    pub hub_url: String,
    pub link: Link,
    pub journal: Arc<Journal>,
    pub exec: Mutex<Option<Arc<Exec>>>,
    pub detected_tags: Vec<String>,
    pub blob_cache: BlobCache,
    pub channel_copies: tokio::sync::Mutex<std::collections::HashMap<String, ChannelCopy>>,
    pub updater: Arc<Updater>,
    /// The requests of the hub that are being answered now.
    pub in_flight: AtomicUsize,
}

/// The events of an update, with a weak link so that an update that waits does
/// not keep a stopped worker alive.
struct CoreEvents(std::sync::Weak<Core>);

impl UpdateEvents for CoreEvents {
    fn busy(&self) -> bool {
        self.0.upgrade().is_some_and(|core| core.has_work())
    }

    fn changed(&self) {
        if let Some(core) = self.0.upgrade() {
            core.send_inventory();
        }
    }
}

/// Counts one request of the hub while it is being answered.
struct InFlight(Arc<Core>);

impl InFlight {
    fn start(core: Arc<Core>) -> Self {
        core.in_flight.fetch_add(1, Ordering::AcqRel);
        Self(core)
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

fn this_os() -> Os {
    Os::from_std_os(std::env::consts::OS).unwrap_or(Os::Linux)
}

/// The runtimes that the worker finds on this machine.
async fn runtimes() -> Vec<String> {
    let mut out = Vec::new();
    for (program, flag) in [
        ("git", "--version"),
        ("node", "--version"),
        ("python3", "--version"),
        ("cargo", "--version"),
        ("go", "version"),
    ] {
        if which::which(program).is_err() {
            continue;
        }
        let run = tokio::process::Command::new(program)
            .arg(flag)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        if let Ok(Ok(output)) = tokio::time::timeout(std::time::Duration::from_secs(5), run).await {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(line) = text.lines().next() {
                out.push(line.trim().to_owned());
            }
        }
    }
    out
}

/// What the model needs to know about a workspace.
pub(crate) async fn inspect(workspace: &StoredWorkspace) -> WorkspaceInspection {
    let os = this_os();
    let layout = workspace
        .record
        .entries
        .first()
        .map_or("empty", |entry| match entry {
            crate::journal::WorkspaceEntry::InPlace { .. } => "in_place",
            crate::journal::WorkspaceEntry::Worktree { .. } => "worktree",
            crate::journal::WorkspaceEntry::Clone { .. } => "clone",
            crate::journal::WorkspaceEntry::Empty { .. } => "empty",
        });
    WorkspaceInspection {
        capability: workspace.capability.clone(),
        worker: gethostname::gethostname().to_string_lossy().into_owned(),
        os,
        arch: std::env::consts::ARCH.to_owned(),
        native_root: workspace.path.clone(),
        path_separator: std::path::MAIN_SEPARATOR.to_string(),
        case_insensitive_paths: matches!(os, Os::Macos | Os::Windows),
        shell: shell().0.to_owned(),
        runtimes: runtimes().await,
        layout: layout.to_owned(),
    }
}

impl Core {
    /// How to reach the hub: its URL and the pin of its certificate.
    pub(crate) fn hub(&self) -> http::HubRef<'_> {
        http::HubRef {
            url: &self.hub_url,
            tls_sha256: self.toml.tls_sha256.as_ref(),
        }
    }

    /// True while a process runs or a request of the hub is being answered.
    /// A journal that cannot be read counts as work: the restart can wait.
    fn has_work(&self) -> bool {
        self.in_flight.load(Ordering::Acquire) > 0
            || self
                .journal
                .running_processes()
                .map_or(true, |running| !running.is_empty())
    }

    /// Follows the version that the hub names.
    fn follow(self: &Arc<Self>, version: &str) -> Option<consort_proto::model::WorkerUpdateStatus> {
        let events = Arc::new(CoreEvents(Arc::downgrade(self)));
        self.updater.clone().follow(version, events)
    }

    fn exec(&self) -> Option<Arc<Exec>> {
        self.exec.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn workspaces_dir(&self) -> PathBuf {
        self.config_dir.join(WORKSPACES_DIR)
    }

    fn inventory(&self) -> Inventory {
        let mut tags = self.detected_tags.clone();
        for tag in &self.toml.tags {
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
        let mut inventory = Inventory {
            boundary: None,
            hash: String::new(),
            hostname: gethostname::gethostname().to_string_lossy().into_owned(),
            os: this_os(),
            arch: std::env::consts::ARCH.to_owned(),
            tags,
            accounts: Vec::new(),
            local_mcp_servers: self.toml.local_server_infos(),
            checkouts: self.toml.checkout_infos(),
            policy: self.limits.get().to_proto(),
            load: WorkerLoad::default(),
            tools: Vec::new(),
            update: self.updater.status(),
        };
        inventory.hash = inventory.compute_hash();
        inventory
    }

    fn send_inventory(&self) {
        if let Err(err) = self.link.send_event(&ExecInventory(self.inventory())) {
            tracing::debug!(error = %err, "could not send the inventory");
        }
    }

    async fn prepare(
        self: &Arc<Self>,
        request: WorkspacePrepare,
    ) -> Result<WorkspaceInspection, WorkerError> {
        if let Some(mut existing) = self.journal.workspace(&request.key)?
            && Path::new(&existing.path).exists()
        {
            workspace::check_record_allowed(&existing.record, &self.limits.get())?;
            if existing.capability != request.capability {
                existing.capability = request.capability.clone();
                self.journal.put_workspace(&existing)?;
            }
            self.sync(WorkspaceSync {
                capability: request.capability,
                attachments: Vec::new(),
                channel_files: Some(request.channel_files),
            })
            .await?;
            return Ok(inspect(&existing).await);
        }
        let prepared = workspace::prepare(
            &self.workspaces_dir(),
            &request.key,
            request.task_id,
            &request.spec,
            &self.limits.get(),
        )
        .await?;
        workspace::exclude_worker_folders(&prepared.path).await;
        let stored = StoredWorkspace {
            key: request.key.clone(),
            capability: request.capability.clone(),
            path: prepared.path.to_string_lossy().into_owned(),
            record: prepared.record,
        };
        self.journal.put_workspace(&stored)?;
        self.sync(WorkspaceSync {
            capability: request.capability,
            attachments: Vec::new(),
            channel_files: Some(request.channel_files),
        })
        .await?;
        Ok(inspect(&stored).await)
    }

    async fn sync(
        self: &Arc<Self>,
        request: WorkspaceSync,
    ) -> Result<WorkspaceSyncResult, WorkerError> {
        let Some(workspace) = self.journal.workspace_of(&request.capability)? else {
            return Err(WorkerError::Workspace(
                "this worker has no workspace for this session".to_owned(),
            ));
        };
        let path = PathBuf::from(&workspace.path);
        let fetch = LinkBlobs(self.clone());
        let mut failures =
            save_attachments(&self.blob_cache, &fetch, &path, &request.attachments).await;
        if let Some(files) = &request.channel_files {
            let mut copies = self.channel_copies.lock().await;
            let copy = copies.entry(workspace.key.clone()).or_default();
            failures.extend(copy.sync(&self.blob_cache, &fetch, &path, files).await);
        }
        Ok(WorkspaceSyncResult {
            failures: failures.iter().map(|f| f.message("file")).collect(),
        })
    }

    async fn release(&self, key: &str, delete: bool) -> Result<(), WorkerError> {
        if let Some(workspace) = self.journal.workspace(key)? {
            workspace::release(&workspace.record, delete).await?;
            if delete {
                self.journal.delete_workspace(key)?;
            }
        }
        Ok(())
    }

    /// Answers one hub request.
    fn dispatch(self: Arc<Self>, request: RequestFrame) {
        tokio::spawn(async move {
            let _in_flight = InFlight::start(self.clone());
            let id = request.id.clone();
            let message: HubToExecution = match request.decode() {
                Ok(message) => message,
                Err(err) => {
                    self.reply::<Empty>(
                        id,
                        Err(ErrorBody::new(ErrorCode::Invalid, err.to_string())),
                    );
                    return;
                }
            };
            match message {
                HubToExecution::WorkspacePrepare(body) => {
                    let result = self.prepare(body).await.map_err(|e| e.to_error_body());
                    self.reply(id, result);
                }
                HubToExecution::WorkspaceInspect(body) => {
                    let result = match self.journal.workspace_of(&body.capability) {
                        Ok(Some(workspace)) => Ok(inspect(&workspace).await),
                        Ok(None) => Err(ErrorBody::new(
                            ErrorCode::NotFound,
                            "this worker has no such workspace",
                        )),
                        Err(err) => Err(err.to_error_body()),
                    };
                    self.reply(id, result);
                }
                HubToExecution::WorkspaceSync(body) => {
                    let result = self.sync(body).await.map_err(|e| e.to_error_body());
                    self.reply(id, result);
                }
                HubToExecution::WorkspaceRelease(body) => {
                    let result = self
                        .release(&body.key, body.delete)
                        .await
                        .map(|()| Empty {})
                        .map_err(|e| e.to_error_body());
                    self.reply(id, result);
                }
                HubToExecution::ExecOp(body) => {
                    let Some(exec) = self.exec() else {
                        self.reply::<Empty>(
                            id,
                            Err(ErrorBody::new(
                                ErrorCode::Internal,
                                "the worker is stopping",
                            )),
                        );
                        return;
                    };
                    let result = exec.run(body).await;
                    self.reply(id, Ok(result));
                }
                HubToExecution::PolicySet(body) => {
                    let result = self
                        .limits
                        .change(body)
                        .map(|policy| policy.to_proto())
                        .map_err(|e| e.to_error_body());
                    if result.is_ok() {
                        self.send_inventory();
                    }
                    self.reply(id, result);
                }
                HubToExecution::InventoryRefresh(_) => {
                    self.send_inventory();
                    self.reply(id, Ok(Empty {}));
                }
                HubToExecution::WorkerUpdate(body) => {
                    let status = self.follow(&body.version);
                    self.reply(id, Ok(WorkerUpdateAck { status }));
                }
            }
        });
    }

    fn reply<T: Serialize>(&self, id: consort_proto::frame::FrameId, result: Result<T, ErrorBody>) {
        let body = result.and_then(|value| {
            serde_json::to_value(&value)
                .map_err(|e| ErrorBody::new(ErrorCode::Internal, e.to_string()))
        });
        let frame = match body {
            Ok(body) => Frame::Response(consort_proto::frame::ResponseFrame {
                re: id,
                result: Ok(body),
            }),
            Err(error) => Frame::error(id, error),
        };
        if let Err(err) = self.link.send_frame(&frame) {
            tracing::debug!(error = %err, "could not answer the hub");
        }
    }
}

impl LinkHandler for Core {
    fn worker_id(&self) -> WorkerId {
        self.worker_id
    }

    fn hub_url(&self) -> &str {
        &self.hub_url
    }

    fn tls_sha256(&self) -> Option<&consort_proto::model::Sha256Hash> {
        self.toml.tls_sha256.as_ref()
    }

    fn ws_path(&self) -> &str {
        consort_proto::auth::WORKER_WS_PATH
    }

    fn sign(&self, payload: &[u8]) -> Signature {
        self.identity.sign(payload)
    }

    fn hello(&self) -> WorkerHello {
        WorkerHello {
            worker_id: self.worker_id,
            version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_versions: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
            sessions: Vec::new(),
        }
    }

    fn connected(self: Arc<Self>, _result: WorkerHelloResult) {
        self.send_inventory();
    }

    fn command(self: Arc<Self>, request: RequestFrame) {
        self.dispatch(request);
    }

    /// A hub that refuses this worker names its version in the error. A hub
    /// from before that field names nothing, and the worker waits for a new
    /// program by hand.
    fn protocol_mismatch(self: Arc<Self>, hub: Option<&HubVersions>) {
        match hub {
            Some(hub) => {
                self.follow(&hub.hub_version);
            }
            None => tracing::warn!(
                "the hub speaks no common protocol version and does not name its version. Update the hub"
            ),
        }
    }
}

/// Blobs through the signed blob routes.
pub(crate) struct LinkBlobs(pub Arc<Core>);

#[async_trait]
impl BlobFetch for LinkBlobs {
    async fn fetch(
        &self,
        sha256: &Sha256Hash,
        max_bytes: u64,
        dest: &Path,
    ) -> Result<(), WorkerError> {
        http::download_blob_to_file(
            self.0.hub(),
            self.0.worker_id,
            &self.0.identity,
            sha256,
            max_bytes,
            dest,
        )
        .await?;
        Ok(())
    }
}

/// The hub's files, for the tools.
pub(crate) struct CoreHub(pub std::sync::Weak<Core>);

impl CoreHub {
    fn core(&self) -> Result<Arc<Core>, String> {
        self.0
            .upgrade()
            .ok_or_else(|| "the worker is stopping".to_owned())
    }
}

#[async_trait]
impl HubFiles for CoreHub {
    async fn blob_put(&self, bytes: Vec<u8>) -> Result<String, String> {
        let core = self.core()?;
        let hash = Sha256Hash::from_bytes(Sha256::digest(&bytes).into());
        http::upload_blob(core.hub(), core.worker_id, &core.identity, &hash, bytes)
            .await
            .map_err(|e| e.to_string())?;
        Ok(hash.to_string())
    }

    async fn blob_get(&self, sha256: &str, dest: &Path) -> Result<(), String> {
        let core = self.core()?;
        let hash: Sha256Hash = sha256
            .parse()
            .map_err(|_| format!("`{sha256}` is not a sha256"))?;
        let cached = core
            .blob_cache
            .ensure(&LinkBlobs(core.clone()), &hash)
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::copy(&cached, dest)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn skill_folder(&self, name: &str, content_hash: &str) -> Result<PathBuf, String> {
        let core = self.core()?;
        let hash: Sha256Hash = content_hash
            .parse()
            .map_err(|_| format!("`{content_hash}` is not a content hash"))?;
        let folder = core
            .config_dir
            .join(SKILLS_DIR)
            .join(hash.as_str())
            .join("files");
        if !folder.is_dir() {
            let bundle: SkillBundle = core
                .link
                .request(&ExecSkillBundleGet(SkillBundleGet {
                    content_hash: hash.clone(),
                }))
                .await
                .map_err(|e| e.to_string())?;
            if bundle.content_hash != hash || !bundle.is_consistent() {
                return Err(format!(
                    "the hub sent files for skill `{name}` that do not match its hash"
                ));
            }
            consort_proto::model::validate_skill_files(&bundle.files).map_err(|e| e.to_string())?;
            let temp = core
                .config_dir
                .join(SKILLS_DIR)
                .join(format!(".tmp-{}", uuid::Uuid::now_v7().simple()));
            for file in &bundle.files {
                let bytes = http::download_blob(
                    core.hub(),
                    core.worker_id,
                    &core.identity,
                    &file.sha256,
                    BLOB_MAX_BYTES,
                )
                .await
                .map_err(|e| e.to_string())?;
                if Sha256Hash::from_bytes(Sha256::digest(&bytes).into()) != file.sha256 {
                    return Err(format!("the file {} does not match its hash", file.path));
                }
                let path = temp.join("files").join(&file.path);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
                #[cfg(unix)]
                if file.executable {
                    use std::os::unix::fs::PermissionsExt as _;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                        .map_err(|e| e.to_string())?;
                }
            }
            let dir = core.config_dir.join(SKILLS_DIR).join(hash.as_str());
            if let Err(err) = std::fs::rename(&temp, &dir) {
                let _ = std::fs::remove_dir_all(&temp);
                if !folder.is_dir() {
                    return Err(err.to_string());
                }
            }
        }
        Ok(folder)
    }
}

/// The tags of this machine.
pub(crate) fn detected_tags() -> Vec<String> {
    tags::detected_tags()
}
