//! The client against an in-process fake hub: pairing, the signed challenge,
//! hello, snapshot, subscribe, the feed, sending, answering an approval, and
//! a reconnect that catches up from the feed position.

// Tests may unwrap (AGENTS.md: "No unwrap or expect outside tests").
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{
        State,
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use consort_client::{
    ApprovalChoice, Client, ClientError, Command, Connection, PostBody, TaskCardView, TaskStatus,
    TimelineItem, ViewState,
    hub::{ClientOptions, Timeouts},
    store::Store,
};
use consort_proto::{
    auth::{
        AuthChallenge, AuthResponse, ClientHelloResult, Nonce, PairClientRequest,
        PairClientResponse, auth_signing_payload, normalize_origin,
    },
    client_api::{
        self as api, ApprovalAnswer, ApprovalResult, EntityPayload, FeedEntry, MessageList,
        MessageListResult, MessageResult, MessageSend, SNAPSHOT_MESSAGE_LIMIT, Snapshot,
        SnapshotScope, SyncSnapshot, SyncSubscribe, TaskGet, TaskResult,
    },
    error::{ErrorBody, ErrorCode},
    frame::{Empty, EventFrame, Frame, RequestFrame},
    ids::{ApprovalId, ChannelId, ClientId, MessageId, RequestId, SessionId, TaskId},
    model::{
        Approval, ApprovalDetails, ApprovalMode, ApprovalStatus, AuditStatus, Author, CallDecision,
        CallOutcome, Channel, ChannelKind, ConnectorCall, Isolation, Message, MessageKind,
        PlacementRequest, RequestOption, RequestOptionKind, Skill, SkillAudit, SkillDirectoryEntry,
        SkillDirectoryKind, SkillDirectorySource, SkillFileInfo, SkillSource, Task, TaskCreator,
        ToolKind,
    },
    status::TaskStatus as PS,
    time::Timestamp,
};
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};

const CODE: &str = "ABCD-EFGH";

#[path = "fake_hub_parts/phase4.rs"]
mod phase4;
#[path = "fake_hub_parts/voice.rs"]
mod voice;

#[derive(Default)]
struct Hub {
    origin: String,
    key: Option<VerifyingKey>,
    client_id: Option<ClientId>,
    seq: u64,
    /// The request types of each WebSocket session, in order.
    sessions: Vec<Vec<String>>,
    /// `hub.status` says that a backup runs for this many more answers.
    backup_polls_left: u32,
    /// The backup settings that `hub.backup_settings` keeps.
    backup_settings: Option<api::BackupSettings>,
    /// The other paired app that `client.list` names.
    other_client: Option<consort_proto::model::Client>,
    channel: Option<Channel>,
    task: Option<Task>,
    approval: Option<Approval>,
    messages: Vec<Message>,
    /// On a reconnect, answer `sync.subscribe` with `resync_required` once.
    resync_on_reconnect: bool,
    resync_sent: bool,
    kick: Arc<tokio::sync::Notify>,
    /// More channels, besides `channel`.
    extra: Vec<Channel>,
    /// Stop reading from every socket, so the hub answers no ping.
    mute: Arc<tokio::sync::Notify>,
    /// Refuse every WebSocket upgrade, as a hub that is down.
    down: bool,
    /// Store `message.send` requests but never answer them.
    silent_sends: bool,
    /// Frames to push to the connected client.
    outbound: Vec<Frame>,
    push: Arc<tokio::sync::Notify>,
    /// The next N channel snapshots fail.
    fail_channel_snapshots: usize,
    /// The channels that a client asked a snapshot for, in order.
    channel_snapshots: Vec<ChannelId>,
    /// `Some(x)` forces `messages_has_more` in a channel snapshot.
    force_has_more: Option<bool>,
    /// Every `client_message_id` that `message.send` carried, in order.
    sent_ids: Vec<String>,
    /// The body of every `message.send`, in order.
    sent_bodies: Vec<String>,
    /// The messages of every `read.mark`.
    read_marks: Vec<MessageId>,
    /// The `after_seq` of every `sync.subscribe`.
    subscribe_after: Vec<u64>,
    /// The next N `read.mark` requests fail with an internal error.
    fail_read_marks: usize,
    /// The next N `message.send` requests store the message and then answer
    /// with an internal error, as a hub that fails after it wrote.
    internal_after_store: usize,
    /// `approval.answer` says the approval is answered already.
    answer_conflict: bool,
    /// Snapshots cut the brief and the review summary of `task` to a preview
    /// and name the task in `tasks_cut`, as the real hub does for long text.
    preview_task: bool,
    /// A task that no snapshot holds, and that `task.get` knows. When it is
    /// set, every snapshot says `tasks_has_more`.
    outside_task: Option<Task>,
    /// The task ID of every `task.get`, in order.
    task_gets: Vec<TaskId>,
    /// The next `task.get` for `task`: the hub changes the task to this one
    /// and sends the feed entry, and then answers with the task as it was.
    /// The answer is older than the feed entry that the client got first.
    task_get_race: Option<Task>,
    /// The request type and the body of every group, move and archive
    /// request, in order.
    recorded: Vec<(String, serde_json::Value)>,
    /// `approval.answer` records the answer and stays connected, as the hub
    /// does for a hub card.
    record_answers: bool,
    /// Answer every skill request with `unsupported`, as a hub without the
    /// skill pipeline does. Off by default: the hub supports skills.
    skills_unsupported: bool,
    /// The skill directories that the hub searches.
    skill_sources: Vec<SkillDirectorySource>,
    /// The skills that `skill.install` made, oldest first.
    skills: Vec<Skill>,
    /// The audit log of connector calls, newest first.
    calls: Vec<ConnectorCall>,
    /// The tokens that `auth.http_token` gave out, oldest first.
    http_tokens: Vec<String>,
    /// The blobs, by hash.
    blobs: std::collections::HashMap<String, Vec<u8>>,
    /// Every blob request: the method and the hash.
    blob_log: Vec<(String, String)>,
    /// The attachments of every `message.send`, in order.
    sent_attachments: Vec<Vec<consort_proto::model::BlobRef>>,
    /// The channel files.
    files: Vec<consort_proto::model::ChannelFile>,
    /// The routines and their runs.
    routines: Vec<consort_proto::model::Routine>,
    runs: Vec<consort_proto::model::RoutineRun>,
    /// The memory notes, also the deleted ones, and every version.
    notes: Vec<consort_proto::model::MemoryNote>,
    versions: Vec<consort_proto::model::MemoryNote>,
    /// Phase 4: see `fake_hub_parts/phase4.rs`.
    phase4: phase4::State,
    voice: voice::State,
    /// The hub epoch that `hello` sends.
    hub_epoch: Option<String>,
    /// While set, each channel snapshot waits until this fires.
    hold_channel_snapshots: Option<Arc<tokio::sync::Notify>>,
}

fn note_hash(content: &str) -> consort_proto::model::Sha256Hash {
    use sha2::{Digest, Sha256};
    consort_proto::model::Sha256Hash::from_bytes(Sha256::digest(content.as_bytes()).into())
}

fn stale() -> ErrorBody {
    ErrorBody::new(
        ErrorCode::StaleWrite,
        "The note changed since you read it. Read it again, then retry the write.",
    )
}

/// What the real hub writes at the end of a cut text.
const CUT_MARK: &str = "\n\n[The hub cut this text here, because it is too long.]";

/// A text that is longer than the 1 KiB preview.
fn long_text(word: &str) -> String {
    format!("{word} ").repeat(500)
}

/// The preview of a long text, as the real hub cuts it.
fn preview_of(text: &str) -> String {
    format!("{}{CUT_MARK}", &text[..1024 - CUT_MARK.len()])
}

/// The tasks, `tasks_cut` and `tasks_has_more` of a snapshot.
fn snapshot_tasks(h: &Hub) -> (Vec<Task>, Vec<TaskId>, bool) {
    let mut tasks: Vec<Task> = h.task.clone().into_iter().collect();
    let mut cut = Vec::new();
    if h.preview_task {
        for task in &mut tasks {
            task.brief = preview_of(&task.brief);
            task.review_summary = task.review_summary.as_deref().map(preview_of);
            cut.push(task.id);
        }
    }
    (tasks, cut, h.outside_task.is_some())
}

type Shared = Arc<Mutex<Hub>>;

fn ts(ms: i64) -> Timestamp {
    Timestamp::from_millis(ms)
}

fn seed(hub: &mut Hub) {
    hub.skill_sources = vec![
        SkillDirectorySource {
            id: "smithery".into(),
            kind: SkillDirectoryKind::Smithery,
            name: "Smithery".into(),
            url: "https://smithery.ai".into(),
            removable: false,
        },
        SkillDirectorySource {
            id: "anthropic".into(),
            kind: SkillDirectoryKind::Anthropic,
            name: "Anthropic".into(),
            url: "https://github.com/anthropics/skills".into(),
            removable: false,
        },
        SkillDirectorySource {
            id: MARKETPLACE_URL.into(),
            kind: SkillDirectoryKind::Marketplace,
            name: "Team plugins".into(),
            url: MARKETPLACE_URL.into(),
            removable: true,
        },
    ];
    let channel_id = ChannelId::new();
    let card = MessageId::new();
    let task_id = TaskId::new();
    let now = Timestamp::now().as_millis();
    hub.channel = Some(Channel {
        id: channel_id,
        kind: ChannelKind::Tutti,
        name: "Tutti".into(),
        icon: None,
        color: None,
        group_id: None,
        position: 0,
        purpose: "All channels, one memory".into(),
        instructions: String::new(),
        coordinator_selection: None,
        task_selection: None,
        preferred_workers: vec![],
        repo_ids: vec![],
        default_isolation: Isolation::Worktree,
        default_approval_mode: ApprovalMode::AutoEdits,
        allowed_local_mcp_servers: vec![],
        memory_enabled: true,
        disabled_connections: Vec::new(),
        disabled_skills: Vec::new(),
        archived_at: None,
        created_at: ts(now - 10_000),
        updated_at: ts(now - 10_000),
        tags: Vec::new(),
        proactive_tasks: Default::default(),
        proactivity: None,
    });
    hub.messages.push(Message {
        id: card,
        channel_id,
        thread_root_id: None,
        author: Author::Assistant,
        author_session_id: None,
        author_task_id: Some(task_id),
        author_routine_id: None,
        kind: MessageKind::TaskCard,
        body: String::new(),
        attachments: vec![],
        in_timeline: false,
        client_message_id: None,
        reply_count: 0,
        last_reply_at: None,
        reactions: vec![],
        edited_at: None,
        deleted_at: None,
        created_at: ts(now - 5_000),
        proactive: None,
        suggestions: Vec::new(),
        source_message_id: None,
    });
    hub.task = Some(Task {
        id: task_id,
        channel_id,
        title: "Build the Windows engine".into(),
        brief: "Build it.".into(),
        status: PS::NeedsInput,
        status_reason: None,
        created_by: TaskCreator::Assistant,
        card_message_id: card,
        model_selection: None,
        placement: PlacementRequest::default(),
        approval_mode: ApprovalMode::Ask,
        start_at: None,
        current_session_id: None,
        worker_id: None,
        review_summary: None,
        resolved_at: None,
        session_ended: false,
        archived_at: None,
        created_at: ts(now - 5_000),
        updated_at: ts(now - 5_000),
    });
    hub.approval = Some(Approval {
        id: ApprovalId::new(),
        channel_id,
        session_id: SessionId::new(),
        task_id: Some(task_id),
        message_id: MessageId::new(),
        request_id: RequestId::new("req-1"),
        summary: "Codex wants to run a command".into(),
        details: ApprovalDetails::Command {
            command: "cargo build --release".into(),
            cwd: Some("C:\\Code\\sayso".into()),
        },
        options: vec![
            RequestOption {
                id: "allow".into(),
                label: "Allow once".into(),
                kind: RequestOptionKind::AllowOnce,
                description: None,
            },
            RequestOption {
                id: "deny".into(),
                label: "Deny".into(),
                kind: RequestOptionKind::DenyOnce,
                description: None,
            },
        ],
        status: ApprovalStatus::Pending,
        answer_option_id: None,
        answered_at: None,
        answered_by_client_id: None,
        created_at: ts(now - 4_000),
    });
}

/// The marketplace that the fake hub has from the start.
const MARKETPLACE_URL: &str = "https://plugins.example.com/marketplace.json";
/// The most entries or calls in one page of the fake hub, whatever `limit`
/// says. It is small, so a test can reach a second page.
const FAKE_PAGE: usize = 2;
/// The commit that every fake preview pins.
const FAKE_COMMIT: &str = "3f2a91c0d4e5b6a7980123456789abcdef012345";

fn hash_of(digit: char) -> consort_proto::model::Sha256Hash {
    digit.to_string().repeat(64).parse().unwrap()
}

/// The skill directory of the fake hub: three entries in three directories.
fn skill_catalog(h: &Hub) -> Vec<SkillDirectoryEntry> {
    let entry = |entry_id: &str,
                 directory,
                 source_id: Option<&str>,
                 name: &str,
                 description: &str,
                 repo: &str,
                 path: &str,
                 status| SkillDirectoryEntry {
        entry_id: entry_id.into(),
        directory,
        source_id: source_id.map(Into::into),
        name: name.into(),
        description: description.into(),
        author: Some("acme".into()),
        url: Some(format!("{repo}/tree/main/{path}")),
        repo_url: Some(repo.into()),
        path: Some(path.into()),
        license: Some("MIT".into()),
        installs: Some(1_200),
        audits: vec![SkillAudit {
            source: "Socket".into(),
            status,
            summary: Some("No known problems.".into()),
            url: None,
        }],
        installed_skill_id: h.skills.iter().find(|s| s.name == name).map(|s| s.id),
    };
    vec![
        entry(
            "anthropic:pdf-forms",
            SkillDirectoryKind::Anthropic,
            None,
            "pdf-forms",
            "Fill in PDF forms.",
            "https://github.com/anthropics/skills",
            "pdf",
            AuditStatus::Passed,
        ),
        entry(
            "smithery:rust-review",
            SkillDirectoryKind::Smithery,
            None,
            "rust-review",
            "Review Rust code for bugs.",
            "https://github.com/acme/rust-skills",
            "review",
            AuditStatus::Warning,
        ),
        entry(
            "market:rust-test-writer",
            SkillDirectoryKind::Marketplace,
            Some(MARKETPLACE_URL),
            "rust-test-writer",
            "Write Rust tests and run them.",
            "https://github.com/acme/team-plugins",
            "rust-test-writer",
            AuditStatus::Passed,
        ),
    ]
}

/// The source ID that a `source` filter of a search compares with.
fn entry_source(entry: &SkillDirectoryEntry) -> String {
    match entry.directory {
        SkillDirectoryKind::Smithery => "smithery".into(),
        SkillDirectoryKind::Anthropic => "anthropic".into(),
        _ => entry.source_id.clone().unwrap_or_default(),
    }
}

/// `skill_directory.search`: filters by words and by source, and pages by an
/// offset in `cursor`. The first page names one directory that did not
/// answer.
fn search_skills(
    h: &Hub,
    body: &api::SkillDirectorySearch,
) -> Result<api::SkillDirectorySearchResult, ErrorBody> {
    let offset = match body.cursor.as_deref() {
        None => 0,
        Some(cursor) => cursor.parse::<usize>().map_err(|_| {
            ErrorBody::new(
                ErrorCode::Invalid,
                "The cursor is not from this hub. Search again.",
            )
        })?,
    };
    let words = body.search.as_deref().unwrap_or("").to_lowercase();
    let matching: Vec<SkillDirectoryEntry> = skill_catalog(h)
        .into_iter()
        .filter(|e| {
            e.name.to_lowercase().contains(&words) || e.description.to_lowercase().contains(&words)
        })
        .filter(|e| {
            body.source
                .as_deref()
                .is_none_or(|source| entry_source(e) == source)
        })
        .collect();
    let end = (offset + FAKE_PAGE).min(matching.len());
    let entries = matching
        .get(offset..end)
        .map(<[SkillDirectoryEntry]>::to_vec)
        .unwrap_or_default();
    Ok(api::SkillDirectorySearchResult {
        entries,
        next_cursor: (end < matching.len()).then(|| end.to_string()),
        unavailable: if offset == 0 {
            vec!["skills.sh".into()]
        } else {
            Vec::new()
        },
    })
}

/// What the fake hub fetched for a source: the preview and the source as
/// the hub stores it.
struct Fetched {
    preview: api::SkillPreviewResult,
    source: SkillSource,
}

/// `skill.preview` and the first step of `skill.install`. A git URL that
/// holds `broken` is not a skill (`invalid`), `flaky` makes the hub fail
/// (`internal`), and `blocked` gives a skill that the hub blocks.
fn fetch_skill(h: &Hub, source: &api::SkillInstallSource) -> Result<Fetched, ErrorBody> {
    let file = |path: &str, size: u64, executable| SkillFileInfo {
        path: path.into(),
        sha256: hash_of('d'),
        size,
        executable,
    };
    let (name, description, source, scripts, has_hooks, blocked_reason, license, audits) =
        match source {
            api::SkillInstallSource::Directory { entry_id, .. } => {
                let entry = skill_catalog(h)
                    .into_iter()
                    .find(|e| &e.entry_id == entry_id)
                    .ok_or_else(|| {
                        ErrorBody::new(ErrorCode::NotFound, "That directory entry does not exist.")
                    })?;
                let scripted = entry.name == "rust-test-writer";
                (
                    entry.name.clone(),
                    entry.description.clone(),
                    SkillSource::Directory {
                        directory: entry.directory,
                        entry_id: entry.entry_id.clone(),
                        url: entry.repo_url.clone().unwrap_or_default(),
                        path: entry.path.clone(),
                    },
                    if scripted {
                        vec!["scripts/run-tests.sh".to_owned()]
                    } else {
                        Vec::new()
                    },
                    scripted,
                    None,
                    entry.license.clone(),
                    entry.audits.clone(),
                )
            }
            api::SkillInstallSource::Git { url, path, git_ref } => {
                if url.contains("broken") {
                    return Err(ErrorBody::new(
                        ErrorCode::Invalid,
                        "The hub found no skill at that address. Check the URL, then try again.",
                    ));
                }
                if url.contains("flaky") {
                    return Err(ErrorBody::new(
                        ErrorCode::Internal,
                        "The fake hub fails this preview.",
                    ));
                }
                let name = path
                    .as_deref()
                    .filter(|p| !p.is_empty())
                    .unwrap_or(url)
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("skill")
                    .to_owned();
                let blocked = url.contains("blocked").then(|| {
                    "The audit failed: the skill reads files outside its folder.".to_owned()
                });
                (
                    name,
                    format!("A skill from {url}."),
                    SkillSource::Git {
                        url: url.clone(),
                        path: path.clone(),
                        git_ref: git_ref.clone(),
                    },
                    Vec::new(),
                    false,
                    blocked,
                    Some("Apache-2.0".to_owned()),
                    vec![SkillAudit {
                        source: "Socket".into(),
                        status: AuditStatus::Unknown,
                        summary: None,
                        url: None,
                    }],
                )
            }
            api::SkillInstallSource::Upload {
                blob_sha256,
                size,
                file_name,
            } => {
                // As the real hub: the zip must be on the hub first.
                if h.blobs.get(blob_sha256.as_str()).map(|b| b.len() as u64) != Some(*size) {
                    return Err(ErrorBody::new(
                        ErrorCode::Invalid,
                        "The zip file is not on the hub. Upload it first.",
                    ));
                }
                (
                    file_name.trim_end_matches(".zip").to_owned(),
                    "A skill from a zip file.".to_owned(),
                    SkillSource::Upload {
                        file_name: file_name.clone(),
                    },
                    Vec::new(),
                    false,
                    None,
                    None,
                    Vec::new(),
                )
            }
        };
    let mut files = vec![file("SKILL.md", 1_204, false)];
    files.extend(scripts.iter().map(|path| file(path, 380, true)));
    Ok(Fetched {
        preview: api::SkillPreviewResult {
            installed_skill_id: h.skills.iter().find(|s| s.name == name).map(|s| s.id),
            name,
            description,
            commit: Some(FAKE_COMMIT.into()),
            content_hash: hash_of('e'),
            license,
            scripts,
            audits,
            files,
            has_hooks,
            blocked_reason,
        },
        source,
    })
}

/// `skill.install`: the skill that the hub would make. A skill with scripts
/// is off, whatever `enabled` says.
fn install_skill(h: &Hub, body: &api::SkillInstall) -> Result<Skill, ErrorBody> {
    let Fetched { preview, source } = fetch_skill(h, &body.source)?;
    if let Some(reason) = preview.blocked_reason {
        return Err(ErrorBody::new(
            ErrorCode::SkillBlocked,
            format!("The hub blocks this skill. {reason}"),
        ));
    }
    if preview.installed_skill_id.is_some() {
        return Err(ErrorBody::new(
            ErrorCode::Conflict,
            format!("A skill named {} is installed already.", preview.name),
        ));
    }
    let has_scripts = !preview.scripts.is_empty();
    Ok(Skill {
        id: consort_proto::ids::SkillId::new(),
        name: preview.name,
        description: preview.description,
        source,
        commit: preview.commit,
        content_hash: preview.content_hash,
        license: preview.license,
        scripts: preview.scripts,
        audits: preview.audits,
        enabled: !has_scripts && body.enabled.unwrap_or(true),
        use_in: body.use_in.clone(),
        update: Default::default(),
        blocked_reason: None,
        hooks_removed: preview.has_hooks,
        file_count: u32::try_from(preview.files.len()).unwrap(),
        size_bytes: preview.files.iter().map(|f| f.size).sum(),
        installed_at: Timestamp::now(),
        updated_at: Timestamp::now(),
    })
}

/// `n` calls of a connection, newest first, each one minute older than the
/// one before. The first three results are 2,048, 412 and 12,700 bytes.
fn connector_calls(
    connection_id: consort_proto::ids::ConnectionId,
    n: usize,
) -> Vec<ConnectorCall> {
    let now = Timestamp::now().as_millis();
    (0..n)
        .map(|i| ConnectorCall {
            id: consort_proto::ids::ConnectorCallId::new(),
            connection_id,
            channel_id: None,
            session_id: None,
            task_id: None,
            tool: format!("GMAIL_TOOL_{i}"),
            tool_kind: if i == 0 {
                ToolKind::Change
            } else {
                ToolKind::Read
            },
            arguments_sha256: hash_of('b'),
            decision: if i == 0 {
                CallDecision::DeniedByUser
            } else {
                CallDecision::AllowedByUser
            },
            outcome: if i == 0 {
                CallOutcome::NotRun
            } else {
                CallOutcome::Succeeded
            },
            error: None,
            composio_log_id: None,
            result_bytes: [2_048, 412, 12_700].get(i).copied().unwrap_or(1),
            duration_ms: 120,
            at: ts(now - 60_000 * i as i64),
        })
        .collect()
}

async fn pair_client(
    State(hub): State<Shared>,
    Json(body): Json<PairClientRequest>,
) -> impl IntoResponse {
    if body.code.canonical() != CODE.replace('-', "") {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": {"code": "unauthorized", "message": "The code is not valid."}})),
        )
            .into_response();
    }
    let mut hub = hub.lock().unwrap();
    hub.key = Some(VerifyingKey::from_bytes(body.public_key.as_bytes()).unwrap());
    let id = ClientId::new();
    hub.client_id = Some(id);
    Json(PairClientResponse { client_id: id }).into_response()
}

/// Whether a blob request carries a token that `auth.http_token` gave out.
fn blob_authorized(hub: &Hub, headers: &axum::http::HeaderMap) -> bool {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|token| hub.http_tokens.iter().any(|t| t == token))
}

async fn get_blob(
    State(hub): State<Shared>,
    axum::extract::Path(sha): axum::extract::Path<String>,
    method: axum::http::Method,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    let mut h = hub.lock().unwrap();
    h.blob_log.push((method.to_string(), sha.clone()));
    if !blob_authorized(&h, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match h.blobs.get(&sha) {
        Some(bytes) => (StatusCode::OK, bytes.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn put_blob(
    State(hub): State<Shared>,
    axum::extract::Path(sha): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    use sha2::Digest as _;
    let mut h = hub.lock().unwrap();
    h.blob_log.push(("PUT".into(), sha.clone()));
    if !blob_authorized(&h, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let digest: [u8; 32] = sha2::Sha256::digest(&body).into();
    if consort_proto::model::Sha256Hash::from_bytes(digest).as_str() != sha {
        return StatusCode::BAD_REQUEST.into_response();
    }
    h.blobs.insert(sha, body.to_vec());
    StatusCode::OK.into_response()
}

async fn ws(State(hub): State<Shared>, upgrade: WebSocketUpgrade) -> impl IntoResponse {
    if hub.lock().unwrap().down {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    upgrade
        .on_upgrade(move |socket| session(hub, socket))
        .into_response()
}

async fn send(socket: &mut WebSocket, frame: Frame) {
    socket
        .send(WsMessage::Text(frame.to_text().unwrap().into()))
        .await
        .unwrap();
}

async fn next_request(socket: &mut WebSocket) -> Option<RequestFrame> {
    while let Some(Ok(msg)) = socket.recv().await {
        if let WsMessage::Text(text) = msg
            && let Ok(Frame::Request(req)) = Frame::parse(text.as_str())
        {
            return Some(req);
        }
    }
    None
}

fn feed(hub: &mut Hub, payload: EntityPayload) -> Frame {
    hub.seq += 1;
    Frame::event(&FeedEntry::upsert(hub.seq, Timestamp::now(), payload)).unwrap()
}

async fn session(hub: Shared, mut socket: WebSocket) {
    let nonce = Nonce::from_bytes([9u8; 32]);
    let index = {
        let mut h = hub.lock().unwrap();
        h.sessions.push(Vec::new());
        h.sessions.len() - 1
    };
    send(&mut socket, Frame::event(&AuthChallenge { nonce }).unwrap()).await;
    let (kick, mute, push) = {
        let h = hub.lock().unwrap();
        (h.kick.clone(), h.mute.clone(), h.push.clone())
    };
    loop {
        let req = tokio::select! {
            req = next_request(&mut socket) => req,
            _ = kick.notified() => {
                let _ = socket.send(WsMessage::Close(None)).await;
                return;
            }
            _ = mute.notified() => {
                // Stop reading. The client's pings get no answer.
                std::future::pending::<()>().await;
                return;
            }
            _ = push.notified() => {
                let frames = std::mem::take(&mut hub.lock().unwrap().outbound);
                for frame in frames {
                    send(&mut socket, frame).await;
                }
                continue;
            }
        };
        let Some(req) = req else { return };
        hub.lock().unwrap().sessions[index].push(req.kind.clone());
        let reply = match req.kind.as_str() {
            "auth.response" => {
                let auth: AuthResponse = req.decode_as().unwrap();
                let h = hub.lock().unwrap();
                let payload = auth_signing_payload(&h.origin, &nonce);
                let sig = Signature::from_bytes(auth.signature.as_bytes());
                match h.key.map(|k| k.verify(&payload, &sig)) {
                    Some(Ok(())) => req.respond_ok(&Empty {}).unwrap(),
                    _ => {
                        req.respond_error(ErrorBody::new(ErrorCode::Unauthorized, "Bad signature."))
                    }
                }
            }
            "hello" => {
                let (feed_seq, hub_epoch) = {
                    let h = hub.lock().unwrap();
                    (h.seq, h.hub_epoch.clone())
                };
                req.respond_ok(&ClientHelloResult {
                    protocol_version: 1,
                    hub_version: "test".into(),
                    feed_seq,
                    hub_epoch,
                })
                .unwrap()
            }
            "sync.snapshot" => {
                let body: SyncSnapshot = req.decode_as().unwrap();
                let gate = hub.lock().unwrap().hold_channel_snapshots.clone();
                if let (SnapshotScope::Channel(_), Some(gate)) = (&body.scope, gate) {
                    gate.notified().await;
                }
                let mut h = hub.lock().unwrap();
                // As the real hub: scope `all` has no messages. A channel
                // snapshot has the newest top-level messages of one channel.
                match body.scope {
                    SnapshotScope::All => {
                        let (tasks, tasks_cut, tasks_has_more) = snapshot_tasks(&h);
                        req.respond_ok(&Snapshot {
                            seq: h.seq,
                            channels: h
                                .channel
                                .clone()
                                .into_iter()
                                .chain(h.extra.clone())
                                .collect(),
                            tasks,
                            tasks_cut,
                            tasks_has_more,
                            approvals: h.approval.clone().into_iter().collect(),
                            routines: h.routines.clone(),
                            ..Snapshot::default()
                        })
                        .unwrap()
                    }
                    SnapshotScope::Channel(_) if h.fail_channel_snapshots > 0 => {
                        h.fail_channel_snapshots -= 1;
                        req.respond_error(ErrorBody::new(
                            ErrorCode::Internal,
                            "The fake hub fails this snapshot.",
                        ))
                    }
                    SnapshotScope::Channel(channel_id) => {
                        h.channel_snapshots.push(channel_id);
                        let mut top: Vec<Message> = h
                            .messages
                            .iter()
                            .filter(|m| m.channel_id == channel_id && m.thread_root_id.is_none())
                            .cloned()
                            .collect();
                        top.sort_by_key(|m| m.created_at);
                        let skip = top.len().saturating_sub(SNAPSHOT_MESSAGE_LIMIT);
                        let has_more = h.force_has_more.unwrap_or(skip > 0);
                        let (tasks, tasks_cut, tasks_has_more) = snapshot_tasks(&h);
                        req.respond_ok(&Snapshot {
                            seq: h.seq,
                            messages: top.split_off(skip),
                            messages_has_more: has_more,
                            tasks,
                            tasks_cut,
                            tasks_has_more,
                            approvals: h.approval.clone().into_iter().collect(),
                            ..Snapshot::default()
                        })
                        .unwrap()
                    }
                }
            }
            "message.list" => {
                let body: MessageList = req.decode_as().unwrap();
                let h = hub.lock().unwrap();
                let mut page: Vec<Message> = h
                    .messages
                    .iter()
                    .filter(|m| {
                        m.channel_id == body.channel_id && m.thread_root_id == body.thread_root_id
                    })
                    .cloned()
                    .collect();
                page.sort_by_key(|m| m.created_at);
                if let Some(before) = body.before {
                    let at = h
                        .messages
                        .iter()
                        .find(|m| m.id == before)
                        .map(|m| m.created_at)
                        .unwrap();
                    page.retain(|m| m.created_at < at);
                }
                let limit = body.limit.unwrap_or(50) as usize;
                let has_more = page.len() > limit;
                let skip = page.len().saturating_sub(limit);
                req.respond_ok(&MessageListResult {
                    messages: page.split_off(skip),
                    has_more,
                })
                .unwrap()
            }
            "sync.subscribe" => {
                let body: SyncSubscribe = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                h.subscribe_after.push(body.after_seq);
                if index > 0 && h.resync_on_reconnect && !h.resync_sent {
                    h.resync_sent = true;
                    req.respond_error(ErrorBody::new(
                        ErrorCode::ResyncRequired,
                        "The feed position is older than the oldest kept entry.",
                    ))
                } else {
                    req.respond_ok(&Empty {}).unwrap()
                }
            }
            "auth.http_token" => {
                let mut h = hub.lock().unwrap();
                let token = format!("tok-{}", h.http_tokens.len() + 1);
                h.http_tokens.push(token.clone());
                req.respond_ok(&consort_proto::auth::HttpToken {
                    token,
                    expires_at: ts(Timestamp::now().as_millis() + 600_000),
                })
                .unwrap()
            }
            "files.list" => {
                let body: api::FilesList = req.decode_as().unwrap();
                let h = hub.lock().unwrap();
                req.respond_ok(&consort_proto::tools::FilesListResult {
                    files: h
                        .files
                        .iter()
                        .filter(|f| f.channel_id == body.channel_id)
                        .cloned()
                        .collect(),
                })
                .unwrap()
            }
            "files.put" => {
                let body: api::FilesPut = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                if !h.blobs.contains_key(body.blob_sha256.as_str()) {
                    req.respond_error(ErrorBody::new(
                        ErrorCode::Invalid,
                        "The blob is not on the hub. Upload the blob first.",
                    ))
                } else {
                    let file = consort_proto::model::ChannelFile {
                        channel_id: body.channel_id,
                        path: body.path.clone(),
                        blob_sha256: body.blob_sha256,
                        size: body.size,
                        mime: body.mime,
                        task_id: None,
                        title: None,
                        updated_by: "user".into(),
                        updated_at: Timestamp::now(),
                        deleted_at: None,
                    };
                    h.files
                        .retain(|f| !(f.channel_id == body.channel_id && f.path == body.path));
                    h.files.push(file.clone());
                    req.respond_ok(&api::ChannelFileResult { file }).unwrap()
                }
            }
            "files.move" => {
                let body: api::FilesMove = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                match h
                    .files
                    .iter_mut()
                    .find(|f| f.channel_id == body.channel_id && f.path == body.from_path)
                {
                    Some(file) => {
                        file.path = body.to_path.clone();
                        let file = file.clone();
                        req.respond_ok(&api::ChannelFileResult { file }).unwrap()
                    }
                    None => req.respond_error(ErrorBody::new(
                        ErrorCode::NotFound,
                        "That file does not exist.",
                    )),
                }
            }
            "routine.create" => {
                let body: api::RoutineCreate = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                let routine = consort_proto::model::Routine {
                    id: consort_proto::ids::RoutineId::new(),
                    channel_id: body.channel_id,
                    name: body.name,
                    schedule: body.schedule,
                    prompt: body.prompt,
                    action: body.action,
                    enabled: body.enabled,
                    next_run_at: Some(ts(Timestamp::now().as_millis() + 3_600_000)),
                    last_run_at: None,
                    last_run_status: None,
                    created_at: None,
                    kind: Default::default(),
                    origin: Default::default(),
                };
                h.routines.push(routine.clone());
                req.respond_ok(&consort_proto::tools::RoutineResult {
                    routine,
                    notes: Vec::new(),
                })
                .unwrap()
            }
            "routine.update" => {
                let body: api::RoutineUpdate = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                let routine = h
                    .routines
                    .iter_mut()
                    .find(|r| r.id == body.routine_id)
                    .unwrap();
                if let Some(enabled) = body.enabled {
                    routine.enabled = enabled;
                }
                if let Some(name) = body.name {
                    routine.name = name;
                }
                if let Some(channel_id) = body.channel_id {
                    routine.channel_id = channel_id;
                }
                let routine = routine.clone();
                req.respond_ok(&consort_proto::tools::RoutineResult {
                    routine,
                    notes: Vec::new(),
                })
                .unwrap()
            }
            "routine.delete" => {
                let body: api::RoutineDelete = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                h.routines.retain(|r| r.id != body.routine_id);
                req.respond_ok(&Empty {}).unwrap()
            }
            "routine.runs" => {
                let body: api::RoutineRuns = req.decode_as().unwrap();
                let h = hub.lock().unwrap();
                let mut runs: Vec<_> = h
                    .runs
                    .iter()
                    .filter(|r| r.routine_id == body.routine_id)
                    .cloned()
                    .collect();
                runs.reverse();
                req.respond_ok(&api::RoutineRunsResult { runs }).unwrap()
            }
            "routine.run_now" => {
                // The run starts, then its turn ends: two feed entries after
                // the answer.
                let body: api::RoutineRunNow = req.decode_as().unwrap();
                let events = {
                    let mut h = hub.lock().unwrap();
                    let routine = h
                        .routines
                        .iter()
                        .find(|r| r.id == body.routine_id)
                        .cloned()
                        .unwrap();
                    let mut run = consort_proto::model::RoutineRun {
                        id: h.runs.len() as u64 + 1,
                        routine_id: routine.id,
                        channel_id: routine.channel_id,
                        started_at: Timestamp::now(),
                        ended_at: None,
                        status: consort_proto::model::RoutineRunStatus::Running,
                        manual: true,
                        message_id: None,
                        task_id: None,
                        error: None,
                    };
                    let started = feed(&mut h, EntityPayload::RoutineRun(run.clone()));
                    run.status = consort_proto::model::RoutineRunStatus::Ok;
                    run.ended_at = Some(Timestamp::now());
                    run.message_id = Some(MessageId::new());
                    h.runs.push(run.clone());
                    let ended = feed(&mut h, EntityPayload::RoutineRun(run));
                    vec![started, ended]
                };
                send(&mut socket, req.respond_ok(&Empty {}).unwrap()).await;
                for event in events {
                    send(&mut socket, event).await;
                }
                continue;
            }
            "memory.list" => {
                let body: api::MemoryList = req.decode_as().unwrap();
                let h = hub.lock().unwrap();
                let notes = h
                    .notes
                    .iter()
                    .filter(|n| body.scope.is_none_or(|s| s == n.scope))
                    .filter(|n| n.deleted_at.is_some() == body.deleted)
                    .map(|n| n.summary())
                    .collect();
                req.respond_ok(&consort_proto::tools::MemoryListResult { notes })
                    .unwrap()
            }
            "memory.read" => {
                let body: api::MemoryRead = req.decode_as().unwrap();
                let h = hub.lock().unwrap();
                match h.notes.iter().find(|n| {
                    n.scope == body.scope && n.path == body.path && n.deleted_at.is_none()
                }) {
                    Some(note) => req
                        .respond_ok(&api::MemoryNoteResult { note: note.clone() })
                        .unwrap(),
                    None => req.respond_error(ErrorBody::new(ErrorCode::NotFound, "No note.")),
                }
            }
            "memory.write" => {
                let body: api::MemoryWrite = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                let current = h
                    .notes
                    .iter()
                    .find(|n| {
                        n.scope == body.scope && n.path == body.path && n.deleted_at.is_none()
                    })
                    .map(|n| n.sha256.clone());
                if !body.expected_sha256.matches(current.as_ref()) {
                    req.respond_error(stale())
                } else {
                    let version = h
                        .versions
                        .iter()
                        .filter(|v| v.scope == body.scope && v.path == body.path)
                        .count() as u64
                        + 1;
                    let note = consort_proto::model::MemoryNote {
                        scope: body.scope,
                        path: body.path.clone(),
                        sha256: note_hash(&body.content),
                        content: body.content,
                        version,
                        updated_by: consort_proto::model::MemoryAuthor::User,
                        updated_at: Timestamp::now(),
                        suggested_by: None,
                        deleted_at: None,
                    };
                    h.notes
                        .retain(|n| !(n.scope == body.scope && n.path == body.path));
                    h.notes.push(note.clone());
                    h.versions.push(note.clone());
                    req.respond_ok(&api::MemoryNoteResult { note }).unwrap()
                }
            }
            "memory.delete" => {
                let body: api::MemoryDelete = req.decode_as().unwrap();
                let event = {
                    let mut h = hub.lock().unwrap();
                    let note = h
                        .notes
                        .iter_mut()
                        .find(|n| n.scope == body.scope && n.path == body.path)
                        .unwrap();
                    note.deleted_at = Some(Timestamp::now());
                    let note = note.clone();
                    feed(&mut h, EntityPayload::MemoryNote(note))
                };
                send(&mut socket, req.respond_ok(&Empty {}).unwrap()).await;
                event
            }
            "memory.restore" => {
                let body: api::MemoryRestore = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                let note = h
                    .notes
                    .iter_mut()
                    .find(|n| n.scope == body.scope && n.path == body.path)
                    .unwrap();
                note.deleted_at = None;
                let note = note.clone();
                req.respond_ok(&api::MemoryNoteResult { note }).unwrap()
            }
            "memory.move" => {
                let body: api::MemoryMove = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                let note = h
                    .notes
                    .iter_mut()
                    .find(|n| n.scope == body.scope && n.path == body.from_path)
                    .unwrap();
                note.path = body.to_path.clone();
                let note = note.clone();
                for v in h.versions.iter_mut().filter(|v| v.path == body.from_path) {
                    v.path = body.to_path.clone();
                }
                req.respond_ok(&api::MemoryNoteResult { note }).unwrap()
            }
            "memory.history" => {
                let body: api::MemoryHistory = req.decode_as().unwrap();
                let h = hub.lock().unwrap();
                let mut versions: Vec<_> = h
                    .versions
                    .iter()
                    .filter(|v| v.scope == body.scope && v.path == body.path)
                    .cloned()
                    .collect();
                versions.reverse();
                req.respond_ok(&api::MemoryHistoryResult { versions })
                    .unwrap()
            }
            "files.delete" => {
                let body: api::FilesDelete = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                h.files
                    .retain(|f| !(f.channel_id == body.channel_id && f.path == body.path));
                req.respond_ok(&Empty {}).unwrap()
            }
            "message.send" => {
                let send_body: MessageSend = req.decode_as().unwrap();
                let internal = {
                    let mut h = hub.lock().unwrap();
                    if h.internal_after_store > 0 {
                        h.internal_after_store -= 1;
                        true
                    } else {
                        false
                    }
                };
                let (known, existing, silent) = {
                    let mut h = hub.lock().unwrap();
                    h.sent_ids.push(send_body.client_message_id.clone());
                    h.sent_bodies.push(send_body.body.clone());
                    h.sent_attachments.push(send_body.attachments.clone());
                    let known = h
                        .channel
                        .iter()
                        .chain(h.extra.iter())
                        .any(|c| c.id == send_body.channel_id);
                    // As the real hub: the first copy of a `client_message_id`
                    // wins.
                    let existing = h
                        .messages
                        .iter()
                        .find(|m| {
                            m.client_message_id.as_deref()
                                == Some(send_body.client_message_id.as_str())
                        })
                        .cloned();
                    (known, existing, h.silent_sends)
                };
                if !known {
                    send(
                        &mut socket,
                        req.respond_error(ErrorBody::new(
                            ErrorCode::NotFound,
                            "That channel does not exist.",
                        )),
                    )
                    .await;
                    continue;
                }
                if internal && existing.is_some() {
                    send(
                        &mut socket,
                        req.respond_error(ErrorBody::new(
                            ErrorCode::Internal,
                            "The fake hub fails after it wrote.",
                        )),
                    )
                    .await;
                    continue;
                }
                if let Some(message) = existing {
                    send(
                        &mut socket,
                        req.respond_ok(&MessageResult { message }).unwrap(),
                    )
                    .await;
                    continue;
                }
                let message = Message {
                    id: MessageId::new(),
                    channel_id: send_body.channel_id,
                    thread_root_id: send_body.thread_root_id,
                    author: Author::User,
                    author_session_id: None,
                    author_task_id: None,
                    author_routine_id: None,
                    kind: MessageKind::Text,
                    body: send_body.body,
                    attachments: send_body.attachments.clone(),
                    in_timeline: false,
                    client_message_id: Some(send_body.client_message_id),
                    reply_count: 0,
                    last_reply_at: None,
                    reactions: vec![],
                    edited_at: None,
                    deleted_at: None,
                    created_at: Timestamp::now(),
                    proactive: None,
                    suggestions: Vec::new(),
                    source_message_id: None,
                };
                if silent {
                    // The hub stores the message, but the answer and the
                    // feed entry never reach the client.
                    hub.lock().unwrap().messages.push(message);
                    continue;
                }
                if internal {
                    // The hub wrote the message, then failed. No feed entry
                    // and no good answer reach the client.
                    hub.lock().unwrap().messages.push(message);
                    send(
                        &mut socket,
                        req.respond_error(ErrorBody::new(
                            ErrorCode::Internal,
                            "The fake hub fails after it wrote.",
                        )),
                    )
                    .await;
                    continue;
                }
                let event = {
                    let mut h = hub.lock().unwrap();
                    h.messages.push(message.clone());
                    feed(&mut h, EntityPayload::Message(message.clone()))
                };
                send(
                    &mut socket,
                    req.respond_ok(&MessageResult { message }).unwrap(),
                )
                .await;
                event
            }
            "task.get" => {
                let body: TaskGet = req.decode_as().unwrap();
                let (found, race) = {
                    let mut h = hub.lock().unwrap();
                    h.task_gets.push(body.task_id);
                    let found = h
                        .task
                        .iter()
                        .chain(h.outside_task.iter())
                        .find(|t| t.id == body.task_id)
                        .cloned();
                    (found, h.task_get_race.take())
                };
                match found {
                    None => req.respond_error(ErrorBody::new(
                        ErrorCode::NotFound,
                        "That task does not exist.",
                    )),
                    Some(task) => {
                        if let Some(newer) = race {
                            // The change reaches the client first. The answer
                            // below still holds the task as it was.
                            let event = {
                                let mut h = hub.lock().unwrap();
                                h.task = Some(newer.clone());
                                feed(&mut h, EntityPayload::Task(newer))
                            };
                            send(&mut socket, event).await;
                        }
                        req.respond_ok(&TaskResult { task }).unwrap()
                    }
                }
            }
            "group.create" => {
                let body: consort_proto::client_api::GroupCreate = req.decode_as().unwrap();
                hub.lock()
                    .unwrap()
                    .recorded
                    .push((req.kind.clone(), req.body.clone()));
                req.respond_ok(&consort_proto::client_api::GroupResult {
                    group: consort_proto::model::Group {
                        id: consort_proto::ids::GroupId::new(),
                        name: body.name,
                        position: 0,
                        collapsed: false,
                    },
                })
                .unwrap()
            }
            "channel.move" | "channel.archive" => {
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                let channel = h.channel.clone().unwrap();
                req.respond_ok(&consort_proto::client_api::ChannelResult { channel })
                    .unwrap()
            }
            "task.update" | "task.retry" => {
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                let task = h.task.clone().unwrap();
                req.respond_ok(&TaskResult { task }).unwrap()
            }
            "draft.edit" => {
                let body: api::DraftEdit = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                let mut message = h.messages[0].clone();
                message.id = body.message_id;
                message.kind = MessageKind::Notice;
                message.in_timeline = true;
                message.body = serde_json::json!({
                    "kind": "draft",
                    "text": "A draft.",
                    "title": body.title.unwrap_or_else(|| "Claim".into()),
                    "body": body.body.clone().unwrap_or_default(),
                    "original_body": "Dear Dr. Meyer,",
                })
                .to_string();
                req.respond_ok(&MessageResult { message }).unwrap()
            }
            "task.archive" => {
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                let task = h.task.clone().unwrap();
                req.respond_ok(&TaskResult { task }).unwrap()
            }
            "read.mark" => {
                let mark: consort_proto::client_api::ReadMark = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                if h.fail_read_marks > 0 {
                    h.fail_read_marks -= 1;
                    req.respond_error(ErrorBody::new(
                        ErrorCode::Internal,
                        "The fake hub fails this read mark.",
                    ))
                } else {
                    h.read_marks.push(mark.last_read_message_id);
                    req.respond_ok(&Empty {}).unwrap()
                }
            }
            "approval.answer" if hub.lock().unwrap().record_answers => {
                let answer: ApprovalAnswer = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                let mut approval = h.approval.clone().unwrap();
                approval.status = ApprovalStatus::Allowed;
                approval.answer_option_id = Some(answer.option_id);
                h.approval = Some(approval.clone());
                let event = feed(&mut h, EntityPayload::Approval(approval.clone()));
                h.outbound.push(event);
                h.push.notify_one();
                req.respond_ok(&ApprovalResult { approval }).unwrap()
            }
            "channel.set_connection_enabled" => {
                let body: consort_proto::client_api::ChannelSetConnectionEnabled =
                    req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                let mut channel = h.channel.clone().unwrap();
                channel
                    .disabled_connections
                    .retain(|c| *c != body.connection_id);
                if !body.enabled {
                    channel.disabled_connections.push(body.connection_id);
                }
                h.channel = Some(channel.clone());
                req.respond_ok(&consort_proto::client_api::ChannelResult { channel })
                    .unwrap()
            }
            "connection.connect" => {
                let body: consort_proto::client_api::ConnectionConnect = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                let mut connection = consort_proto::model::Connection {
                    id: consort_proto::ids::ConnectionId::new(),
                    name: body.name.clone(),
                    handle: consort_proto::model::connection_handle(&body.name),
                    service: body.service.clone(),
                    service_name: "GitHub".into(),
                    logo_url: None,
                    identity: None,
                    use_in: body.use_in.clone(),
                    approvals: body.approvals,
                    status: consort_proto::model::ConnectionStatus::Connecting {
                        redirect_url: Some("https://connect.composio.dev/link/abc".into()),
                        expires_at: ts(10_000_000),
                    },
                    last_used_at: None,
                    created_at: ts(1),
                    updated_at: ts(1),
                };
                let attempt = consort_proto::client_api::ConnectAttempt {
                    connection: connection.clone(),
                    redirect_url: Some("https://connect.composio.dev/link/abc".into()),
                    expires_at: Some(ts(10_000_000)),
                };
                // The browser sign-in ends a moment later: the feed says so.
                connection.status = consort_proto::model::ConnectionStatus::Active;
                connection.identity = Some("chris@work.example".into());
                connection.updated_at = ts(2);
                let event = feed(&mut h, EntityPayload::Connection(connection));
                h.outbound.push(event);
                h.push.notify_one();
                req.respond_ok(&attempt).unwrap()
            }
            "skill_directory.search"
            | "skill_directory.sources"
            | "skill_directory.add_source"
            | "skill_directory.remove_source"
            | "skill.preview"
            | "skill.install"
                if hub.lock().unwrap().skills_unsupported =>
            {
                hub.lock()
                    .unwrap()
                    .recorded
                    .push((req.kind.clone(), req.body.clone()));
                req.respond_error(ErrorBody::new(
                    ErrorCode::Unsupported,
                    format!("Not built yet: {}.", req.kind),
                ))
            }
            "skill_directory.search" => {
                let body: api::SkillDirectorySearch = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                match search_skills(&h, &body) {
                    Ok(result) => req.respond_ok(&result).unwrap(),
                    Err(err) => req.respond_error(err),
                }
            }
            "skill_directory.sources" => {
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                req.respond_ok(&api::SkillDirectorySourcesResult {
                    sources: h.skill_sources.clone(),
                })
                .unwrap()
            }
            "skill_directory.add_source" => {
                let body: api::SkillDirectoryAddSource = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                let url = body.url.trim().to_owned();
                if !url.starts_with("https://") || url.contains(' ') {
                    req.respond_error(ErrorBody::new(
                        ErrorCode::Invalid,
                        format!(
                            "{url:?} is not a link to a marketplace.json file. Paste an https:// link, then try again."
                        ),
                    ))
                } else if h.skill_sources.iter().any(|s| s.id == url) {
                    req.respond_error(ErrorBody::new(
                        ErrorCode::Conflict,
                        "This marketplace is in the list already.",
                    ))
                } else {
                    h.skill_sources.push(SkillDirectorySource {
                        id: url.clone(),
                        kind: SkillDirectoryKind::Marketplace,
                        name: url.trim_start_matches("https://").to_owned(),
                        url,
                        removable: true,
                    });
                    req.respond_ok(&api::SkillDirectorySourcesResult {
                        sources: h.skill_sources.clone(),
                    })
                    .unwrap()
                }
            }
            "skill_directory.remove_source" => {
                let body: api::SkillDirectoryRemoveSource = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                match h.skill_sources.iter().find(|s| s.id == body.id).cloned() {
                    None => req.respond_error(ErrorBody::new(
                        ErrorCode::NotFound,
                        "That directory is not in the list.",
                    )),
                    Some(source) if !source.removable => req.respond_error(ErrorBody::new(
                        ErrorCode::Invalid,
                        format!("{} is built in and stays in the list.", source.name),
                    )),
                    Some(_) => {
                        h.skill_sources.retain(|s| s.id != body.id);
                        req.respond_ok(&api::SkillDirectorySourcesResult {
                            sources: h.skill_sources.clone(),
                        })
                        .unwrap()
                    }
                }
            }
            "skill.preview" => {
                let body: api::SkillPreview = req.decode_as().unwrap();
                let (dropped, result) = {
                    let mut h = hub.lock().unwrap();
                    h.recorded.push((req.kind.clone(), req.body.clone()));
                    let dropped = matches!(
                        &body.source,
                        api::SkillInstallSource::Git { url, .. } if url.contains("dropped")
                    );
                    (dropped, fetch_skill(&h, &body.source))
                };
                if dropped {
                    // The link breaks before the answer.
                    let _ = socket.send(WsMessage::Close(None)).await;
                    return;
                }
                match result {
                    Ok(fetched) => req.respond_ok(&fetched.preview).unwrap(),
                    Err(err) => req.respond_error(err),
                }
            }
            "skill.install" => {
                let body: api::SkillInstall = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                // An install from a card answers the card, so the card must
                // exist before the hub installs anything.
                let card = body
                    .approval_id
                    .map(|id| h.approval.clone().filter(|a| a.id == id));
                match (install_skill(&h, &body), card) {
                    (Err(err), _) => req.respond_error(err),
                    (Ok(_), Some(None)) => req.respond_error(ErrorBody::new(
                        ErrorCode::NotFound,
                        "That install card does not exist.",
                    )),
                    (Ok(skill), card) => {
                        h.skills.push(skill.clone());
                        if let Some(Some(mut approval)) = card {
                            // The card is allowed, and it names the skill.
                            approval.status = ApprovalStatus::Allowed;
                            approval.answer_option_id = Some("allow".into());
                            approval.answered_at = Some(Timestamp::now());
                            if let ApprovalDetails::SkillInstall(card) = &mut approval.details {
                                card.skill_id = Some(skill.id);
                            }
                            h.approval = Some(approval.clone());
                            let event = feed(&mut h, EntityPayload::Approval(approval));
                            h.outbound.push(event);
                            h.push.notify_one();
                        }
                        req.respond_ok(&api::SkillResult { skill }).unwrap()
                    }
                }
            }
            "connection.calls" => {
                let body: api::ConnectionCalls = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                let mine: Vec<ConnectorCall> = h
                    .calls
                    .iter()
                    .filter(|c| c.connection_id == body.connection_id)
                    .cloned()
                    .collect();
                let start = match body.before {
                    None => 0,
                    Some(before) => mine
                        .iter()
                        .position(|c| c.id == before)
                        .map_or(mine.len(), |at| at + 1),
                };
                let end = (start + FAKE_PAGE).min(mine.len());
                req.respond_ok(&api::ConnectionCallsResult {
                    calls: mine[start..end].to_vec(),
                    has_more: end < mine.len(),
                })
                .unwrap()
            }
            "approval.answer" if hub.lock().unwrap().answer_conflict => req.respond_error(
                ErrorBody::new(ErrorCode::Conflict, "This approval was already answered."),
            ),
            "approval.answer" => {
                let answer: ApprovalAnswer = req.decode_as().unwrap();
                let (approval, events) = {
                    let mut h = hub.lock().unwrap();
                    let mut approval = h.approval.clone().unwrap();
                    assert_eq!(answer.option_id, "allow");
                    approval.status = ApprovalStatus::Allowed;
                    approval.answer_option_id = Some(answer.option_id);
                    h.approval = Some(approval.clone());
                    let mut task = h.task.clone().unwrap();
                    task.status = PS::Working;
                    h.task = Some(task.clone());
                    let a = feed(&mut h, EntityPayload::Approval(approval.clone()));
                    let t = feed(&mut h, EntityPayload::Task(task));
                    (approval, vec![a, t])
                };
                send(
                    &mut socket,
                    req.respond_ok(&ApprovalResult { approval }).unwrap(),
                )
                .await;
                for event in events {
                    send(&mut socket, event).await;
                }
                // Drop the link, so the client must reconnect and catch up.
                let _ = socket.send(WsMessage::Close(None)).await;
                return;
            }
            "hub.status" => {
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                let running = (h.backup_polls_left > 0).then(|| {
                    h.backup_polls_left -= 1;
                    api::BackupProgress {
                        trigger: api::BackupTrigger::Manual,
                        started_at: Timestamp::now(),
                        bytes_done: 10,
                        bytes_total: 40,
                    }
                });
                let finished =
                    h.recorded.iter().any(|(k, _)| k == "hub.backup") && running.is_none();
                req.respond_ok(&api::HubStatusResult {
                    status: api::HubInfo {
                        hub_url: "https://hub.example.com".into(),
                        hub_version: "0.6.0".into(),
                        protocol_versions: vec![1],
                        data_dir: "/var/lib/consort".into(),
                        storage: api::HubStorage {
                            database_bytes: 300,
                            blobs_bytes: 600,
                            skills_bytes: 100,
                            free_bytes: Some(5_000),
                        },
                        backup: api::BackupInfo {
                            settings: h.backup_settings.clone().unwrap_or(api::BackupSettings {
                                schedule: api::BackupSchedule::default(),
                                folder: "/var/lib/consort/backups".into(),
                                keep: 7,
                            }),
                            last: finished.then(|| api::BackupRecord {
                                file_name: "consort-2026-10-29-0300.tar".into(),
                                path: "/var/lib/consort/backups/consort-2026-10-29-0300.tar".into(),
                                size_bytes: 40,
                                trigger: api::BackupTrigger::Manual,
                                started_at: Timestamp::now(),
                                finished_at: Timestamp::now(),
                                error: None,
                            }),
                            running,
                        },
                    },
                })
                .unwrap()
            }
            "hub.backup" => {
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                h.backup_polls_left = 2;
                req.respond_ok(&api::HubBackupResult {
                    progress: api::BackupProgress {
                        trigger: api::BackupTrigger::Manual,
                        started_at: Timestamp::now(),
                        bytes_done: 0,
                        bytes_total: 40,
                    },
                })
                .unwrap()
            }
            "hub.backup_settings" => {
                let body: api::HubBackupSettings = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                let mut settings = h.backup_settings.clone().unwrap_or(api::BackupSettings {
                    schedule: api::BackupSchedule::default(),
                    folder: "/var/lib/consort/backups".into(),
                    keep: 7,
                });
                if let Some(folder) = body.folder.filter(|f| !f.starts_with('/')) {
                    req.respond_error(ErrorBody::new(
                        ErrorCode::Invalid,
                        format!("The backup folder must be a full path on the hub machine. \"{folder}\" is not."),
                    ))
                } else {
                    if let Some(schedule) = body.schedule {
                        settings.schedule = schedule;
                    }
                    if let Some(keep) = body.keep {
                        settings.keep = keep;
                    }
                    h.backup_settings = Some(settings.clone());
                    req.respond_ok(&api::HubBackupSettingsResult { settings })
                        .unwrap()
                }
            }
            "client.list" => {
                let h = hub.lock().unwrap();
                let me = h.client_id.unwrap();
                let mine = consort_proto::model::Client {
                    id: me,
                    name: "MacBook Pro".into(),
                    kind: consort_proto::model::ClientKind::Desktop,
                    public_key: consort_proto::auth::PublicKey::from_bytes([1; 32]),
                    push_provider: None,
                    paired_at: Timestamp::now(),
                    last_seen_at: Some(Timestamp::now()),
                    revoked_at: None,
                    os: Some(consort_proto::model::Os::Macos),
                    paired_by: Some(consort_proto::model::PairedBy::HubMachine),
                };
                let mut clients = vec![mine];
                clients.extend(h.other_client.clone());
                req.respond_ok(&api::ClientListResult {
                    clients,
                    online: vec![me],
                })
                .unwrap()
            }
            "client.revoke" => {
                let body: api::ClientRevoke = req.decode_as().unwrap();
                let mut h = hub.lock().unwrap();
                h.recorded.push((req.kind.clone(), req.body.clone()));
                if Some(body.client_id) == h.client_id {
                    req.respond_error(ErrorBody::new(
                        ErrorCode::Conflict,
                        "You cannot revoke the client that you are using.",
                    ))
                } else {
                    req.respond_ok(&Empty {}).unwrap()
                }
            }
            "client.pairing_code" => req
                .respond_ok(&api::ClientPairingCodeResult {
                    hub_url: "https://hub.example.com".into(),
                    code: "KQ7M-4XPD".parse().unwrap(),
                    expires_at: Timestamp::now().plus_millis(600_000),
                })
                .unwrap(),
            kind if voice::handles(kind) => voice::answer(&hub, &req),
            kind if phase4::handles(kind) => {
                let (reply, events) = phase4::answer(&hub, &req);
                send(&mut socket, reply).await;
                for event in events {
                    send(&mut socket, event).await;
                }
                continue;
            }
            other => req.respond_error(ErrorBody::new(
                ErrorCode::Invalid,
                format!("The fake hub has no {other}."),
            )),
        };
        send(&mut socket, reply).await;
    }
}

async fn start_hub() -> (String, Shared) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let url = format!("http://{addr}");
    let hub = Arc::new(Mutex::new(Hub {
        origin: normalize_origin(&url).unwrap(),
        ..Hub::default()
    }));
    seed(&mut hub.lock().unwrap());
    let app = Router::new()
        .route("/v1/pair/client", post(pair_client))
        .route("/v1/client", get(ws))
        .route("/v1/blobs/{sha256}", get(get_blob).put(put_blob))
        .with_state(hub.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, hub)
}

async fn wait_for(
    client: &Client,
    what: &str,
    pred: impl Fn(&ViewState) -> bool,
) -> Arc<ViewState> {
    let mut sub = client.subscribe();
    let current = sub.current();
    if pred(&current) {
        return current;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match tokio::time::timeout_at(deadline, sub.next()).await {
            Ok(Some(state)) if pred(&state) => return state,
            Ok(Some(_)) => {}
            _ => panic!(
                "timed out waiting for {what}; last state: {:?}",
                sub.current().connection
            ),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairs_syncs_sends_answers_and_reconnects() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    let client = Client::start_with(Some(store.clone()), ClientOptions::desktop()).unwrap();
    assert_eq!(client.state().connection, Connection::NotPaired);

    // A wrong code is refused with a clear error.
    let err = client
        .pair(url.clone(), "ZZZZ-ZZZZ".into())
        .await
        .unwrap_err();
    assert_eq!(err, consort_client::ClientError::BadCode);

    client.pair(url.clone(), CODE.into()).await.unwrap();
    assert!(store.load().unwrap().is_some(), "the identity is saved");
    let state = wait_for(&client, "the snapshot", |s| {
        s.connection == Connection::Online && s.tutti().is_some()
    })
    .await;
    let tutti = state.tutti().unwrap().id.clone();
    let card = state
        .timeline(&tutti)
        .and_then(|t| {
            t.items.iter().find_map(|i| match i {
                TimelineItem::TaskCard(c) => Some(c.clone()),
                _ => None,
            })
        })
        .expect("the task card is in the timeline");
    let approval = card
        .approval
        .clone()
        .expect("the approval shows on the card");
    assert_eq!(approval.folder.as_deref(), Some("C:\\Code\\sayso"));
    assert_eq!(state.tasks_panel.need_you, 1);

    client
        .send_message(tutti.clone(), "Hello hub".into())
        .await
        .unwrap();
    wait_for(&client, "the sent message", |s| {
        s.timeline(&tutti).is_some_and(|t| {
            t.items.iter().any(
                |i| matches!(i, TimelineItem::Message(m) if m.body == "Hello hub" && !m.pending),
            )
        })
    })
    .await;

    client
        .answer_approval(approval.id.clone(), ApprovalChoice::AllowOnce)
        .await
        .unwrap();
    let task_id = card.task_id.clone();
    wait_for(&client, "the task to work", |s| {
        s.task(&task_id)
            .is_some_and(|t| t.status == TaskStatus::Working)
    })
    .await;

    // The hub dropped the link. The client comes back and catches up from
    // its feed position instead of loading a new snapshot.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let sessions = hub.lock().unwrap().sessions.clone();
        if sessions.len() >= 2 && sessions[1].iter().any(|k| k == "sync.subscribe") {
            assert_eq!(
                sessions[1],
                vec!["auth.response", "hello", "sync.subscribe"]
            );
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no reconnect: {sessions:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    wait_for(&client, "online again", |s| {
        s.connection == Connection::Online
    })
    .await;
    assert_eq!(
        hub.lock().unwrap().sessions[0][..6],
        [
            "auth.response",
            "hello",
            "sync.snapshot",
            "sync.snapshot",
            // The proactivity log loads before the subscription. This fake
            // hub sends no proactivity settings, so the client sends no
            // time zone.
            "proactivity.log",
            "sync.subscribe"
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resync_required_reloads_the_snapshot() {
    let (url, hub) = start_hub().await;
    hub.lock().unwrap().resync_on_reconnect = true;
    let dir = tempfile::tempdir().unwrap();
    let client =
        Client::start_with(Some(Store::new(dir.path())), ClientOptions::desktop()).unwrap();
    client.pair(url, CODE.into()).await.unwrap();
    wait_for(&client, "the first snapshot", |s| {
        s.connection == Connection::Online
            && s.tutti().is_some_and(|c| c.name == consort_client::TUTTI)
    })
    .await;

    // While the client is away, the hub changes and forgets old feed
    // entries. The client must load a new snapshot instead of catching up.
    let kick = {
        let mut h = hub.lock().unwrap();
        if let Some(channel) = h.channel.as_mut() {
            channel.purpose = "Renamed while the client was away".into();
        }
        h.seq = 40;
        h.kick.clone()
    };
    kick.notify_one();
    let state = wait_for(&client, "the new snapshot", |s| {
        s.connection == Connection::Online
            && s.tutti()
                .is_some_and(|c| c.purpose == "Renamed while the client was away")
    })
    .await;
    assert!(state.tutti().is_some());
    let sessions = hub.lock().unwrap().sessions.clone();
    assert_eq!(
        sessions[1],
        vec![
            "auth.response",
            "hello",
            "sync.subscribe",
            "sync.snapshot",
            "sync.snapshot",
            "proactivity.log",
            "sync.subscribe"
        ]
    );
}

fn text_message(channel_id: ChannelId, body: &str, at: i64, root: Option<MessageId>) -> Message {
    Message {
        id: MessageId::new(),
        channel_id,
        thread_root_id: root,
        author: Author::Assistant,
        author_session_id: None,
        author_task_id: None,
        author_routine_id: None,
        kind: MessageKind::Text,
        body: body.into(),
        attachments: vec![],
        in_timeline: false,
        client_message_id: None,
        reply_count: 0,
        last_reply_at: None,
        reactions: vec![],
        edited_at: None,
        deleted_at: None,
        created_at: ts(at),
        proactive: None,
        suggestions: Vec::new(),
        source_message_id: None,
    }
}

fn timeline_bodies(state: &ViewState, channel: &str) -> Vec<String> {
    state
        .timeline(channel)
        .map(|t| {
            t.items
                .iter()
                .filter_map(|i| match i {
                    TimelineItem::Message(m) => Some(m.body.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_loads_history_threads_and_older_pages() {
    let (url, hub) = start_hub().await;
    let (channel_id, card, task_id) = {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        let card = h.task.as_ref().unwrap().card_message_id;
        let task_id = h.task.as_ref().unwrap().id;
        let base = Timestamp::now().as_millis() - 1_000_000;
        // 60 older timeline messages: more than one channel snapshot holds.
        for i in 0..60 {
            let m = text_message(channel_id, &format!("history {i}"), base + i * 1_000, None);
            h.messages.push(m);
        }
        for (i, body) in ["First reply", "Second reply"].iter().enumerate() {
            let m = text_message(channel_id, body, base + 900_000 + i as i64, Some(card));
            h.messages.push(m);
        }
        (channel_id, card, task_id)
    };
    let dir = tempfile::tempdir().unwrap();
    let first = Client::start_with(Some(Store::new(dir.path())), ClientOptions::desktop()).unwrap();
    first.pair(url, CODE.into()).await.unwrap();
    wait_for(&first, "the first sync", |s| {
        s.connection == Connection::Online
    })
    .await;
    drop(first);

    // A new client with the same identity: the app after a restart.
    let client =
        Client::start_with(Some(Store::new(dir.path())), ClientOptions::desktop()).unwrap();
    let tutti = channel_id.to_string();
    let state = wait_for(&client, "the old messages", |s| {
        timeline_bodies(s, &tutti).iter().any(|b| b == "history 59")
    })
    .await;
    let bodies = timeline_bodies(&state, &tutti);
    assert!(
        !bodies.iter().any(|b| b == "history 0"),
        "one snapshot holds the newest 50"
    );
    assert!(state.timeline(&tutti).unwrap().has_older);
    assert!(
        !bodies.iter().any(|b| b.ends_with("reply")),
        "thread replies stay out of the timeline"
    );

    client.load_older(tutti.clone()).await.unwrap();
    let state = wait_for(&client, "the older page", |s| {
        timeline_bodies(s, &tutti).iter().any(|b| b == "history 0")
    })
    .await;
    assert!(!state.timeline(&tutti).unwrap().has_older);

    client.load_thread(task_id.to_string()).await.unwrap();
    let state = wait_for(&client, "the thread replies", |s| {
        s.thread(&task_id.to_string())
            .is_some_and(|t| t.posts.len() >= 2)
    })
    .await;
    let thread = state.thread(&task_id.to_string()).unwrap();
    let texts: Vec<String> = thread
        .posts
        .iter()
        .filter_map(|p| match &p.bodies[0] {
            consort_client::PostBody::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["First reply", "Second reply"]);
    let _ = card;
}

// ---------------------------------------------------------------------------
// Helpers for the tests below.

/// Short limits, so a test that waits for a timeout stays quick.
fn quick() -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(2),
        handshake: Duration::from_secs(2),
        call: Duration::from_millis(600),
        idle: Duration::from_millis(900),
        retry: Duration::from_millis(50),
        clock: Duration::from_secs(60),
    }
}

fn options(timeouts: Timeouts) -> ClientOptions {
    let mut options = ClientOptions::desktop();
    options.timeouts = timeouts;
    options
}

async fn paired_client(url: &str, dir: &std::path::Path, options: ClientOptions) -> Client {
    let client = Client::start_with(Some(Store::new(dir)), options).unwrap();
    client.pair(url.to_owned(), CODE.into()).await.unwrap();
    wait_for(&client, "online", |s| {
        s.connection == Connection::Online && s.tutti().is_some()
    })
    .await;
    client
}

fn push(hub: &Shared, frames: Vec<Frame>) {
    let notify = {
        let mut h = hub.lock().unwrap();
        h.outbound.extend(frames);
        h.push.clone()
    };
    notify.notify_one();
}

async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn bodies_of(state: &ViewState, channel: &str) -> Vec<(String, bool)> {
    state
        .timeline(channel)
        .map(|t| {
            t.items
                .iter()
                .filter_map(|i| match i {
                    TimelineItem::Message(m) => Some((m.body.clone(), m.pending)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn tutti_id(client: &Client) -> String {
    client.state().tutti().unwrap().id.clone()
}

// ---------------------------------------------------------------------------
// Review findings: feed, loading, links, outbox.

/// Finding 2: a feed entry that does not decode used to end the link with
/// `Lost`, and `seq` stayed put, so the reconnect replayed the entry for ever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreadable_feed_entry_is_skipped_and_never_replayed() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    let (good_seq, frames) = {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        h.seq += 1;
        let bad = Frame::Event(EventFrame {
            kind: "feed".into(),
            body: serde_json::json!({
                "seq": h.seq, "at": 1, "channel_id": channel_id,
                "entity": "widget", "entity_id": "x", "op": "upsert", "payload": {}
            }),
        });
        let m = text_message(
            channel_id,
            "after the bad entry",
            Timestamp::now().as_millis(),
            None,
        );
        h.messages.push(m.clone());
        let good = feed(&mut h, EntityPayload::Message(m));
        (h.seq, vec![bad, good])
    };
    push(&hub, frames);
    wait_for(&client, "the entry after the bad one", |s| {
        bodies_of(s, &tutti)
            .iter()
            .any(|(b, _)| b == "after the bad entry")
    })
    .await;
    // The link stayed up, and the client loaded the channel again.
    until("the channel loads again", || {
        hub.lock().unwrap().channel_snapshots.len() >= 2
    })
    .await;
    assert_eq!(hub.lock().unwrap().sessions.len(), 1, "no reconnect");

    // A reconnect asks for the feed after the last entry, not before the bad
    // one.
    let kick = hub.lock().unwrap().kick.clone();
    kick.notify_one();
    until("a second subscribe", || {
        hub.lock().unwrap().subscribe_after.len() >= 2
    })
    .await;
    assert_eq!(hub.lock().unwrap().subscribe_after[1], good_seq);
}

/// The hub sends `feed.skipped` for an entry that is too large. The client
/// moves past it and loads the channel again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn feed_skipped_moves_past_the_entry_and_loads_the_channel_again() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    let (seq, frame) = {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        // A message that the client never got in the feed.
        let m = text_message(
            channel_id,
            "too large for the feed",
            Timestamp::now().as_millis(),
            None,
        );
        h.messages.push(m);
        h.seq += 1;
        let frame = Frame::event(&consort_proto::client_api::FeedSkipped {
            seq: h.seq,
            channel_id: Some(channel_id),
            entity: consort_proto::client_api::EntityKind::Message,
            entity_id: "msg".into(),
        })
        .unwrap();
        (h.seq, frame)
    };
    push(&hub, vec![frame]);
    wait_for(&client, "the message from the snapshot", |s| {
        bodies_of(s, &tutti)
            .iter()
            .any(|(b, _)| b == "too large for the feed")
    })
    .await;
    assert_eq!(hub.lock().unwrap().sessions.len(), 1, "no reconnect");
    let kick = hub.lock().unwrap().kick.clone();
    kick.notify_one();
    until("a second subscribe", || {
        hub.lock().unwrap().subscribe_after.len() >= 2
    })
    .await;
    assert_eq!(hub.lock().unwrap().subscribe_after[1], seq);
}

/// Finding 4: one channel snapshot that fails left that channel without
/// messages for ever, because the cache counted as loaded already.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_channel_snapshot_that_fails_is_tried_again() {
    let (url, hub) = start_hub().await;
    {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        let m = text_message(
            channel_id,
            "history",
            Timestamp::now().as_millis() - 5_000,
            None,
        );
        h.messages.push(m);
        h.fail_channel_snapshots = 1;
    }
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    wait_for(&client, "the second try", |s| {
        bodies_of(s, &tutti).iter().any(|(b, _)| b == "history")
    })
    .await;
    // The retry can arrive before the state is read, so the hub is the proof
    // that the first try failed.
    assert_eq!(
        hub.lock().unwrap().fail_channel_snapshots,
        0,
        "it failed once"
    );
    assert_eq!(hub.lock().unwrap().sessions.len(), 1, "no reconnect");
}

/// Finding 4: a channel that is unarchived later never loaded its messages.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_channel_that_is_unarchived_gets_its_snapshot() {
    let (url, hub) = start_hub().await;
    let archived_id = {
        let mut h = hub.lock().unwrap();
        let mut channel = h.channel.clone().unwrap();
        channel.id = ChannelId::new();
        channel.kind = ChannelKind::Standard;
        channel.name = "Old".into();
        channel.archived_at = Some(ts(5));
        let m = text_message(
            channel.id,
            "from the old channel",
            Timestamp::now().as_millis() - 5_000,
            None,
        );
        h.messages.push(m);
        let id = channel.id;
        h.extra.push(channel);
        id
    };
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    assert!(
        !hub.lock().unwrap().channel_snapshots.contains(&archived_id),
        "an archived channel loads no snapshot"
    );
    // Someone unarchives it: a feed entry, and no snapshot from the hub.
    let upsert = {
        let mut h = hub.lock().unwrap();
        h.extra[0].archived_at = None;
        let open = h.extra[0].clone();
        feed(&mut h, EntityPayload::Channel(open))
    };
    push(&hub, vec![upsert]);
    let id = archived_id.to_string();
    wait_for(&client, "the unarchived channel", |s| {
        bodies_of(s, &id)
            .iter()
            .any(|(b, _)| b == "from the old channel")
    })
    .await;
}

/// Finding 9 and 5: a send in flight when the link fails stays pending, and
/// goes out again with the same `client_message_id`. The hub keeps one copy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_send_that_gets_no_answer_stays_pending_and_is_not_doubled() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let tutti = tutti_id(&client);
    hub.lock().unwrap().silent_sends = true;
    // The hub stores the message and never answers. The call times out, the
    // link ends, and the message stays pending: the caller sees no error.
    client
        .send_message(tutti.clone(), "Hello hub".into())
        .await
        .unwrap();
    assert!(
        bodies_of(&client.state(), &tutti)
            .iter()
            .any(|(b, pending)| b == "Hello hub" && *pending),
        "the message shows as pending"
    );
    hub.lock().unwrap().silent_sends = false;
    wait_for(&client, "the message after the reconnect", |s| {
        bodies_of(s, &tutti)
            .iter()
            .any(|(b, pending)| b == "Hello hub" && !*pending)
    })
    .await;
    let h = hub.lock().unwrap();
    assert!(h.sessions.len() >= 2, "the link restarted");
    assert_eq!(h.sent_ids.len(), 2, "sent twice");
    assert_eq!(h.sent_ids[0], h.sent_ids[1], "with the same ID");
    let stored = h.messages.iter().filter(|m| m.body == "Hello hub").count();
    assert_eq!(stored, 1, "the hub keeps one copy");
}

/// Finding 5: one command that waits for the hub does not hold up the rest.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stuck_command_does_not_block_the_next_one() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let mut timeouts = quick();
    timeouts.call = Duration::from_secs(5);
    timeouts.idle = Duration::from_secs(30);
    let client = paired_client(&url, dir.path(), options(timeouts)).await;
    let tutti = tutti_id(&client);
    hub.lock().unwrap().silent_sends = true;
    let stuck = {
        let client = client.clone();
        let tutti = tutti.clone();
        tokio::spawn(async move { client.send_message(tutti, "Stuck".into()).await })
    };
    until("the hub got the send", || {
        !hub.lock().unwrap().sent_ids.is_empty()
    })
    .await;
    let started = std::time::Instant::now();
    let task_id = client.state().tasks[0].id.clone();
    let err = client.resolve_task(task_id).await.unwrap_err();
    assert!(matches!(err, ClientError::Hub { .. }), "{err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the second command waited for the first"
    );
    assert!(!stuck.is_finished(), "the first command still waits");
}

/// Finding 5: a hub that stops answering ends the link, even with no request
/// in flight.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hub_that_goes_silent_ends_the_link() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let mute = hub.lock().unwrap().mute.clone();
    mute.notify_one();
    until("a second connection", || {
        hub.lock().unwrap().sessions.len() >= 2
    })
    .await;
    wait_for(&client, "online again", |s| {
        s.connection == Connection::Online
    })
    .await;
}

/// Finding 5: dropping the client never waits for a call that is stuck.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_client_does_not_wait_for_a_stuck_call() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let mut timeouts = quick();
    timeouts.call = Duration::from_secs(20);
    timeouts.idle = Duration::from_secs(60);
    let client = paired_client(&url, dir.path(), options(timeouts)).await;
    let tutti = tutti_id(&client);
    hub.lock().unwrap().silent_sends = true;
    let _ = tokio::time::timeout(
        Duration::from_millis(200),
        client.send_message(tutti, "Stuck".into()),
    )
    .await;
    let started = std::time::Instant::now();
    drop(client);
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "drop waited {:?}",
        started.elapsed()
    );
}

/// Findings 6, 7 and 8: the outbox survives a quit, goes out in the order of
/// the queue, and a message that the hub refuses leaves it and shows as a
/// failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_outbox_survives_a_restart_and_goes_out_in_order() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let first = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&first);
    drop(first);
    let store = Store::new(dir.path());
    let client_id = store.load().unwrap().unwrap().config.client_id;
    let pending = |id: &str, channel: &str, body: &str| consort_client::cache::PendingSend {
        client_message_id: id.into(),
        channel_id: channel.into(),
        thread_root_id: None,
        body: body.into(),
        created_at: Timestamp::now().as_millis(),
        attachments: Vec::new(),
    };
    let missing = ChannelId::new().to_string();
    store
        .save_outbox(
            &client_id,
            &[
                pending("c-1", &tutti, "one"),
                pending("c-2", &missing, "lost"),
                pending("c-3", &tutti, "two"),
                pending("c-4", &tutti, "three"),
            ],
        )
        .unwrap();

    let client =
        Client::start_with(Some(Store::new(dir.path())), ClientOptions::desktop()).unwrap();
    let state = wait_for(&client, "the refused message", |s| {
        !s.send_failures.is_empty()
    })
    .await;
    assert_eq!(state.send_failures[0].body, "lost");
    assert!(
        state.send_failures[0].message.contains("does not exist"),
        "{}",
        state.send_failures[0].message
    );
    wait_for(&client, "the sent messages", |s| {
        let b = bodies_of(s, &tutti);
        ["one", "two", "three"]
            .iter()
            .all(|body| b.iter().any(|(x, pending)| x == body && !pending))
    })
    .await;
    assert_eq!(
        hub.lock().unwrap().sent_bodies,
        vec!["one", "lost", "two", "three"],
        "in the order of the queue"
    );
    until("the outbox file is gone", || {
        !dir.path().join("outbox.json").exists()
    })
    .await;
    client
        .run(Command::DismissSendFailure { id: "c-2".into() })
        .await
        .unwrap();
    wait_for(&client, "the dismissed failure", |s| {
        s.send_failures.is_empty()
    })
    .await;
}

/// A queued message is written to disk at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_queued_message_is_saved_in_the_outbox_file() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let tutti = tutti_id(&client);
    hub.lock().unwrap().silent_sends = true;
    client.send_message(tutti, "Keep me".into()).await.unwrap();
    let file = std::fs::read_to_string(dir.path().join("outbox.json")).unwrap();
    assert!(file.contains("Keep me"), "{file}");
}

/// Finding 12: the snapshot says whether older messages exist. A page can be
/// short when messages are large, or full of replies in the timeline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn has_older_follows_messages_has_more() {
    let (url, hub) = start_hub().await;
    {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        let m = text_message(
            channel_id,
            "only one",
            Timestamp::now().as_millis() - 5_000,
            None,
        );
        h.messages.push(m);
        h.force_has_more = Some(true);
    }
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    wait_for(&client, "the message", |s| {
        bodies_of(s, &tutti).iter().any(|(b, _)| b == "only one")
    })
    .await;
    assert!(
        client.state().timeline(&tutti).unwrap().has_older,
        "a short page with has_more means older messages"
    );

    // A full page with has_more false: nothing older.
    let (url, hub) = start_hub().await;
    {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        let base = Timestamp::now().as_millis() - 1_000_000;
        for i in 0..SNAPSHOT_MESSAGE_LIMIT as i64 {
            let m = text_message(channel_id, &format!("m {i}"), base + i, None);
            h.messages.push(m);
        }
        h.force_has_more = Some(false);
    }
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    wait_for(&client, "the messages", |s| {
        bodies_of(s, &tutti).iter().any(|(b, _)| b == "m 49")
    })
    .await;
    assert!(!client.state().timeline(&tutti).unwrap().has_older);
}

/// Finding 23: a thread with more than 200 replies loads in pages.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_thread_loads_in_pages() {
    let (url, hub) = start_hub().await;
    let (task_id, count) = {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        let card = h.task.as_ref().unwrap().card_message_id;
        let base = Timestamp::now().as_millis() - 1_000_000;
        for i in 0..450 {
            let m = text_message(channel_id, &format!("reply {i}"), base + i, Some(card));
            h.messages.push(m);
        }
        (h.task.as_ref().unwrap().id.to_string(), 450)
    };
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    client.load_thread(task_id.clone()).await.unwrap();
    let state = wait_for(&client, "every reply", |s| {
        s.thread(&task_id).is_some_and(|t| t.posts.len() >= count)
    })
    .await;
    assert_eq!(state.thread(&task_id).unwrap().posts.len(), count);
}

/// Finding 11: MarkRead goes out once for the newest message, and a new
/// message in the open channel needs a new one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mark_read_is_sent_once_for_each_newest_message() {
    let (url, hub) = start_hub().await;
    {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        let m = text_message(
            channel_id,
            "unread",
            Timestamp::now().as_millis() - 5_000,
            None,
        );
        h.messages.push(m);
    }
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    let state = wait_for(&client, "the unread message", |s| {
        s.sidebar.tutti.unread >= 1
    })
    .await;
    assert!(state.sidebar.tutti.unread >= 1);
    client
        .run(Command::MarkRead {
            channel_id: tutti.clone(),
        })
        .await
        .unwrap();
    wait_for(&client, "nothing unread", |s| s.sidebar.tutti.unread == 0).await;
    assert_eq!(hub.lock().unwrap().read_marks.len(), 1);
    client
        .run(Command::MarkRead {
            channel_id: tutti.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        hub.lock().unwrap().read_marks.len(),
        1,
        "the newest message is read already"
    );
    // A new message from the assistant is unread, and a new mark follows.
    let frame = {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        let m = text_message(channel_id, "newer", Timestamp::now().as_millis(), None);
        h.messages.push(m.clone());
        feed(&mut h, EntityPayload::Message(m))
    };
    push(&hub, vec![frame]);
    wait_for(&client, "one unread", |s| s.sidebar.tutti.unread == 1).await;
    client
        .run(Command::MarkRead {
            channel_id: tutti.clone(),
        })
        .await
        .unwrap();
    assert_eq!(hub.lock().unwrap().read_marks.len(), 2);
}

/// Finding 18: a burst of feed entries makes a few view states, not one per
/// entry, and a change that leaves the state as it was publishes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_burst_of_feed_entries_publishes_few_states() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    let frames = {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        let base = Timestamp::now().as_millis();
        (0..300)
            .map(|i| {
                let m = text_message(channel_id, &format!("burst {i}"), base + i, None);
                h.messages.push(m.clone());
                feed(&mut h, EntityPayload::Message(m))
            })
            .collect::<Vec<_>>()
    };
    let mut sub = client.subscribe();
    sub.current();
    push(&hub, frames);
    let mut states = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let Ok(Some(state)) = tokio::time::timeout_at(deadline, sub.next()).await else {
            panic!("timed out; {states} states");
        };
        states += 1;
        if bodies_of(&state, &tutti)
            .iter()
            .any(|(b, _)| b == "burst 299")
        {
            break;
        }
    }
    assert!(states <= 30, "{states} states for 300 entries");

    // Nothing changed, so nothing is published.
    client.run(Command::CancelWorkerPairing).await.unwrap();
    let quiet = tokio::time::timeout(Duration::from_millis(300), sub.next()).await;
    assert!(quiet.is_err(), "an unchanged state was published");
}

// ---------------------------------------------------------------------------
// Review cycle 2.

/// A body of `kib` KiB of plain letters.
fn text_of(kib: usize) -> String {
    "a".repeat(kib * 1024)
}

/// Finding 1: a message over the hub's limit failed to encode, the link
/// counted that as a lost link, and the outbox sent the message again for
/// ever. Now the client refuses the text, the link stays up and the caller
/// keeps its draft.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_over_the_hub_limit_is_refused_before_it_is_sent() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);

    let err = client
        .send_message(tutti.clone(), text_of(300))
        .await
        .unwrap_err();
    let ClientError::Hub { code, message } = err else {
        panic!("expected a hub error, got {err:?}");
    };
    assert_eq!(code, "invalid");
    assert!(message.contains("256 KiB"), "{message}");
    assert!(
        message.contains("Shorten"),
        "it says what to do next: {message}"
    );
    let edit = client
        .edit_message(MessageId::new().to_string(), text_of(300))
        .await
        .unwrap_err();
    assert!(edit.to_string().contains("256 KiB"), "{edit}");

    // Nothing was queued, nothing was sent, and the link never broke.
    assert!(client.state().timeline(&tutti).is_some_and(|t| {
        !t.items
            .iter()
            .any(|i| matches!(i, TimelineItem::Message(m) if m.pending))
    }));
    assert!(
        !dir.path().join("outbox.json").exists() || {
            std::fs::read_to_string(dir.path().join("outbox.json"))
                .unwrap()
                .len()
                < 1024
        }
    );
    assert!(hub.lock().unwrap().sent_ids.is_empty());
    assert_eq!(hub.lock().unwrap().sessions.len(), 1, "no reconnect");

    // A body at the limit goes out.
    client
        .send_message(tutti.clone(), text_of(256))
        .await
        .unwrap();
    until("the large message arrives", || {
        hub.lock().unwrap().sent_bodies.len() == 1
    })
    .await;
}

/// Finding 1: a request that does not fit in one frame fails by itself. It
/// does not end the link.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_too_large_for_a_frame_fails_alone_and_the_link_stays_up() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    // The client does not measure a channel purpose, so this reaches the
    // link and fails to encode there.
    let err = client
        .create_channel(consort_client::NewChannel {
            name: "big".into(),
            purpose: text_of(1100),
            ..consort_client::NewChannel::default()
        })
        .await
        .unwrap_err();
    let ClientError::Hub { code, message } = err else {
        panic!("expected a hub error, got {err:?}");
    };
    assert_eq!(code, "invalid");
    assert!(message.contains("too large"), "{message}");
    assert!(message.contains("Shorten"), "{message}");
    assert_eq!(
        client.state().connection,
        Connection::Online,
        "the link stays up"
    );
    assert_eq!(hub.lock().unwrap().sessions.len(), 1, "no reconnect");
    // The next request works.
    client
        .send_message(tutti_id(&client), "Still here".into())
        .await
        .unwrap();
    until("the next message arrives", || {
        hub.lock().unwrap().sent_bodies == vec!["Still here"]
    })
    .await;
}

/// Finding 1: an outbox file from the old version may hold a body that is
/// too large. It fails once, with a text, and does not come back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_that_is_too_large_in_an_old_outbox_fails_once() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let first = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&first);
    drop(first);
    let store = Store::new(dir.path());
    let client_id = store.load().unwrap().unwrap().config.client_id;
    store
        .save_outbox(
            &client_id,
            &[consort_client::cache::PendingSend {
                client_message_id: "c-big".into(),
                channel_id: tutti,
                thread_root_id: None,
                body: text_of(1100),
                created_at: Timestamp::now().as_millis(),
                attachments: Vec::new(),
            }],
        )
        .unwrap();
    let client =
        Client::start_with(Some(Store::new(dir.path())), ClientOptions::desktop()).unwrap();
    let state = wait_for(&client, "the failed send", |s| !s.send_failures.is_empty()).await;
    assert!(
        state.send_failures[0].message.contains("256 KiB"),
        "{}",
        state.send_failures[0].message
    );
    assert_eq!(
        state.send_failures[0].body.len(),
        1100 * 1024,
        "the text is kept"
    );
    assert!(hub.lock().unwrap().sent_ids.is_empty());
    assert_eq!(
        hub.lock().unwrap().sessions.len(),
        2,
        "one link for each start, and no reconnect loop"
    );
}

/// Finding 4b: a redirect from the pairing endpoint could send the pairing
/// code to any address.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_does_not_follow_a_redirect() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let hits = Arc::new(AtomicUsize::new(0));
    let target = {
        let hits = hits.clone();
        let app = Router::new().route(
            "/v1/pair/client",
            post(move || {
                let hits = hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/v1/pair/client")
    };
    let redirecting = {
        let app = Router::new().route(
            "/v1/pair/client",
            post(move || {
                let target = target.clone();
                async move {
                    (
                        StatusCode::TEMPORARY_REDIRECT,
                        [(axum::http::header::LOCATION, target)],
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    };
    let dir = tempfile::tempdir().unwrap();
    let client =
        Client::start_with(Some(Store::new(dir.path())), ClientOptions::desktop()).unwrap();
    let err = client.pair(redirecting, CODE.into()).await.unwrap_err();
    let text = err.to_string();
    assert!(text.contains("redirect"), "{text}");
    assert!(
        text.contains("Enter the final address"),
        "it says what to do: {text}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "the code went nowhere else");
    assert!(
        Store::new(dir.path()).load().unwrap().is_none(),
        "nothing was saved"
    );
}

/// Finding 4c: a host name over plain `http` is looked up and checked. A
/// name that points to this machine works for pairing and for the link.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hub_name_on_this_machine_works_over_http() {
    let (url, hub) = start_hub().await;
    let port = url.rsplit(':').next().unwrap().to_owned();
    let named = format!("http://localhost:{port}");
    // The hub checks the signed origin, which is what the client uses.
    hub.lock().unwrap().origin = normalize_origin(&named).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&named, dir.path(), ClientOptions::desktop()).await;
    assert_eq!(client.state().connection, Connection::Online);
    assert_eq!(
        Store::new(dir.path())
            .load()
            .unwrap()
            .unwrap()
            .config
            .hub_url,
        named
    );
}

/// Finding 6: a failed MarkRead reached nobody. The error goes to the caller
/// now, and the marker moves only when the hub took it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_mark_read_is_an_error_and_can_be_tried_again() {
    let (url, hub) = start_hub().await;
    {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        let m = text_message(
            channel_id,
            "unread",
            Timestamp::now().as_millis() - 5_000,
            None,
        );
        h.messages.push(m);
        h.fail_read_marks = 1;
    }
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    wait_for(&client, "the unread message", |s| {
        s.sidebar.tutti.unread >= 1
    })
    .await;
    let err = client
        .run(Command::MarkRead {
            channel_id: tutti.clone(),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::Hub { .. }), "{err:?}");
    assert!(hub.lock().unwrap().read_marks.is_empty());
    assert!(
        client.state().sidebar.tutti.unread >= 1,
        "still unread after the failure"
    );
    client
        .run(Command::MarkRead {
            channel_id: tutti.clone(),
        })
        .await
        .unwrap();
    assert_eq!(hub.lock().unwrap().read_marks.len(), 1);
    wait_for(&client, "read", |s| s.sidebar.tutti.unread == 0).await;
}

/// Finding 7: a channel that never loaded showed as empty, and the first
/// live message looked like the newest one. Nothing is marked until the
/// channel has loaded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_channel_that_never_loaded_is_not_marked_read() {
    let (url, hub) = start_hub().await;
    {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        let m = text_message(
            channel_id,
            "history the user never saw",
            Timestamp::now().as_millis() - 5_000,
            None,
        );
        h.messages.push(m);
        h.fail_channel_snapshots = 1_000;
    }
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    assert!(
        !client.state().timeline(&tutti).unwrap().loaded,
        "the snapshot failed"
    );
    // One live message arrives. It must not become "the newest message".
    let frame = {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        let m = text_message(channel_id, "live", Timestamp::now().as_millis(), None);
        h.messages.push(m.clone());
        feed(&mut h, EntityPayload::Message(m))
    };
    push(&hub, vec![frame]);
    wait_for(&client, "the live message", |s| {
        bodies_of(s, &tutti).iter().any(|(b, _)| b == "live")
    })
    .await;
    let err = client
        .run(Command::MarkRead {
            channel_id: tutti.clone(),
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not loaded"), "{err}");
    assert!(hub.lock().unwrap().read_marks.is_empty(), "nothing marked");

    // The loader gets the snapshot later. Then marking works.
    hub.lock().unwrap().fail_channel_snapshots = 0;
    wait_for(&client, "the channel to load", |s| {
        s.timeline(&tutti).is_some_and(|t| t.loaded)
    })
    .await;
    client
        .run(Command::MarkRead { channel_id: tutti })
        .await
        .unwrap();
    assert_eq!(hub.lock().unwrap().read_marks.len(), 1);
}

/// Finding 10: after a skipped change, a failed reload left the cache
/// without it for as long as the link stayed up. The link ends, and the next
/// one loads everything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_reload_after_a_skipped_change_ends_the_link_to_load_again() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let tutti = tutti_id(&client);
    let frame = {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        let m = text_message(
            channel_id,
            "missed in the feed",
            Timestamp::now().as_millis(),
            None,
        );
        h.messages.push(m);
        h.seq += 1;
        h.fail_channel_snapshots = 1;
        Frame::event(&consort_proto::client_api::FeedSkipped {
            seq: h.seq,
            channel_id: Some(channel_id),
            entity: consort_proto::client_api::EntityKind::Message,
            entity_id: "msg".into(),
        })
        .unwrap()
    };
    push(&hub, vec![frame]);
    wait_for(&client, "the message after the second link", |s| {
        bodies_of(s, &tutti)
            .iter()
            .any(|(b, _)| b == "missed in the feed")
    })
    .await;
    let h = hub.lock().unwrap();
    assert!(h.sessions.len() >= 2, "the link ended");
    assert!(
        h.sessions[1].iter().any(|k| k == "sync.snapshot"),
        "the second link loaded a snapshot: {:?}",
        h.sessions[1]
    );
}

/// Finding 11a: an answer that does not say what happened (an internal
/// error) may come after the hub stored the message. The outbox sends it
/// again with the same ID, and the hub keeps one copy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unclear_hub_error_sends_the_message_again_with_the_same_id() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let tutti = tutti_id(&client);
    hub.lock().unwrap().internal_after_store = 2;
    client
        .send_message(tutti.clone(), "Hello twice".into())
        .await
        .unwrap();
    wait_for(&client, "the message stored", |s| {
        bodies_of(s, &tutti)
            .iter()
            .any(|(b, pending)| b == "Hello twice" && !*pending)
    })
    .await;
    let h = hub.lock().unwrap();
    assert_eq!(h.sent_ids.len(), 3, "two unclear answers, then the third");
    assert!(h.sent_ids.iter().all(|id| *id == h.sent_ids[0]));
    assert_eq!(
        h.messages
            .iter()
            .filter(|m| m.body == "Hello twice")
            .count(),
        1
    );
    assert!(client.state().send_failures.is_empty());
    assert_eq!(h.sessions.len(), 1, "no reconnect");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unclear_hub_error_is_final_after_a_few_tries_and_keeps_the_text() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let tutti = tutti_id(&client);
    hub.lock().unwrap().internal_after_store = usize::MAX;
    client
        .send_message(tutti, "Never confirmed".into())
        .await
        .unwrap();
    let state = wait_for(&client, "the failed send", |s| !s.send_failures.is_empty()).await;
    assert_eq!(state.send_failures[0].body, "Never confirmed");
    assert!(
        state.send_failures[0].message.contains("may have stored"),
        "it warns about a copy: {}",
        state.send_failures[0].message
    );
    let h = hub.lock().unwrap();
    assert_eq!(h.sent_ids.len(), 4, "a few tries, not for ever");
    assert_eq!(
        h.messages
            .iter()
            .filter(|m| m.body == "Never confirmed")
            .count(),
        1
    );
}

/// A hub decision (here: the channel does not exist) is final at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clear_refusal_is_not_tried_again() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let err = client
        .send_message(ChannelId::new().to_string(), "Nowhere".into())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("does not exist"), "{err}");
    assert_eq!(hub.lock().unwrap().sent_ids.len(), 1);
}

/// Finding 11b: pairing again reset the cache and the outbox file, and the
/// text of a message that waited vanished.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_again_keeps_the_text_of_unsent_messages() {
    let (url_a, _hub_a) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let first = paired_client(&url_a, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&first);
    drop(first);
    let store = Store::new(dir.path());
    let client_id = store.load().unwrap().unwrap().config.client_id;
    store
        .save_outbox(
            &client_id,
            &[consort_client::cache::PendingSend {
                client_message_id: "c-wait".into(),
                channel_id: tutti,
                thread_root_id: None,
                body: "Text that waited".into(),
                created_at: Timestamp::now().as_millis(),
                attachments: Vec::new(),
            }],
        )
        .unwrap();
    // The saved hub is gone, so the message waits.
    let config = std::fs::read_to_string(dir.path().join("client.json")).unwrap();
    let dead = config.replace(&url_a, "http://127.0.0.1:1");
    assert_ne!(config, dead);
    std::fs::write(dir.path().join("client.json"), dead).unwrap();
    let client =
        Client::start_with(Some(Store::new(dir.path())), ClientOptions::desktop()).unwrap();

    let (url_b, hub_b) = start_hub().await;
    client.pair(url_b, CODE.into()).await.unwrap();
    let state = wait_for(&client, "the failed send", |s| !s.send_failures.is_empty()).await;
    assert_eq!(state.send_failures[0].body, "Text that waited");
    assert!(
        state.send_failures[0].message.contains("paired"),
        "{}",
        state.send_failures[0].message
    );
    wait_for(&client, "online", |s| s.connection == Connection::Online).await;
    assert!(
        hub_b.lock().unwrap().sent_bodies.is_empty(),
        "the text did not go to the other hub"
    );
    assert_eq!(
        client.state().send_failures.len(),
        1,
        "it stays until dismissed"
    );
}

/// Finding 13: the hub says an approval was answered already. The card
/// stops asking, and the error carries the hub's own words.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answering_an_approval_that_is_closed_closes_its_card() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    let card_approval = |s: &ViewState| {
        s.timeline(&tutti).and_then(|t| {
            t.items.iter().find_map(|i| match i {
                TimelineItem::TaskCard(c) => c.approval.clone(),
                _ => None,
            })
        })
    };
    let approval = card_approval(&client.state()).expect("a pending approval");
    hub.lock().unwrap().answer_conflict = true;
    let err = client
        .answer_approval(approval.id.clone(), ApprovalChoice::AllowOnce)
        .await
        .unwrap_err();
    let ClientError::Hub { code, message } = err else {
        panic!("expected a hub error");
    };
    assert_eq!(code, "conflict");
    assert_eq!(message, "This approval was already answered.");
    wait_for(&client, "the card to stop asking", |s| {
        card_approval(s).is_none()
    })
    .await;
}

/// Finding 12: labels such as "Now" and "2 min" stayed as they were, because
/// the state is made only when something changes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn labels_that_follow_the_clock_are_made_again_without_a_change() {
    let (url, hub) = start_hub().await;
    {
        let mut h = hub.lock().unwrap();
        let now = Timestamp::now().as_millis();
        let mut task = h.task.clone().unwrap();
        // Two seconds from its first minute.
        task.updated_at = ts(now - 58_000);
        h.task = Some(task);
    }
    let mut opts = options(quick());
    opts.timeouts.clock = Duration::from_millis(100);
    opts.timeouts.idle = Duration::from_secs(30);
    opts.timeouts.call = Duration::from_secs(10);
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), opts).await;
    wait_for(&client, "a task that is a minute old", |s| {
        s.tasks.first().is_some_and(|t| t.age_label == "1 min")
    })
    .await;
}

// ---------------------------------------------------------------------------
// Tasks in a snapshot: previews, `task.get` and tasks outside the snapshot.

/// The first task card in the timeline of a channel.
fn task_card(state: &ViewState, channel: &str) -> Option<TaskCardView> {
    state.timeline(channel).and_then(|t| {
        t.items.iter().find_map(|i| match i {
            TimelineItem::TaskCard(c) => Some(c.clone()),
            _ => None,
        })
    })
}

/// Makes the task of the fake hub a task in review, with a long brief and a
/// long review summary that the snapshots cut to previews. Returns the task
/// ID and the two full texts.
fn previewed_task(hub: &Shared) -> (TaskId, String, String) {
    let mut h = hub.lock().unwrap();
    h.approval = None;
    h.preview_task = true;
    let task = h.task.as_mut().unwrap();
    task.status = PS::Review;
    task.brief = long_text("brief");
    task.review_summary = Some(long_text("summary"));
    (task.id, task.brief.clone(), long_text("summary"))
}

/// Opening a thread needs the full task. The snapshot holds a preview, so the
/// client asks the hub for the task once, and the card shows the full text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_the_thread_of_a_previewed_task_loads_the_full_task() {
    let (url, hub) = start_hub().await;
    let (task_id, _brief, summary) = previewed_task(&hub);
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);

    // The card keeps the preview. Nothing asked for the full text yet.
    let state = wait_for(&client, "the card", |s| {
        task_card(s, &tutti).is_some_and(|c| c.latest.is_some())
    })
    .await;
    let preview = task_card(&state, &tutti).unwrap().latest.unwrap();
    assert_eq!(preview, preview_of(&summary));
    assert!(preview.ends_with(CUT_MARK));
    assert!(hub.lock().unwrap().task_gets.is_empty());

    client.load_thread(task_id.to_string()).await.unwrap();
    wait_for(&client, "the full review summary", |s| {
        task_card(s, &tutti).is_some_and(|c| c.latest.as_deref() == Some(summary.as_str()))
    })
    .await;
    let asked = hub.lock().unwrap().task_gets.clone();
    assert!(!asked.is_empty() && asked.iter().all(|id| *id == task_id));

    // The task is whole now, so another open asks for nothing.
    client.load_thread(task_id.to_string()).await.unwrap();
    assert_eq!(hub.lock().unwrap().task_gets, asked);
}

/// A feed entry always holds the full task. It ends the preview state, so the
/// thread opens with no `task.get`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_feed_entry_ends_the_preview_of_a_task() {
    let (url, hub) = start_hub().await;
    let (task_id, _brief, summary) = previewed_task(&hub);
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    wait_for(&client, "the preview on the card", |s| {
        task_card(s, &tutti)
            .is_some_and(|c| c.latest.as_deref() == Some(preview_of(&summary).as_str()))
    })
    .await;

    let frame = {
        let mut h = hub.lock().unwrap();
        let task = h.task.clone().unwrap();
        feed(&mut h, EntityPayload::Task(task))
    };
    push(&hub, vec![frame]);
    wait_for(&client, "the full text from the feed", |s| {
        task_card(s, &tutti).is_some_and(|c| c.latest.as_deref() == Some(summary.as_str()))
    })
    .await;

    client.load_thread(task_id.to_string()).await.unwrap();
    assert!(
        hub.lock().unwrap().task_gets.is_empty(),
        "the client asked for a task that it holds in full"
    );
}

/// The hub answers `task.get` with the task as it was, after the client got
/// a feed entry with a newer task. The older answer must not win.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_older_task_get_answer_does_not_overwrite_a_newer_feed_entry() {
    let (url, hub) = start_hub().await;
    let (task_id, _brief, _summary) = previewed_task(&hub);
    {
        let mut h = hub.lock().unwrap();
        let mut newer = h.task.clone().unwrap();
        newer.review_summary = Some("The summary after the change.".into());
        h.task_get_race = Some(newer);
    }
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    wait_for(&client, "the card", |s| task_card(s, &tutti).is_some()).await;

    client.load_thread(task_id.to_string()).await.unwrap();
    let newer = |s: &ViewState| {
        task_card(s, &tutti)
            .is_some_and(|c| c.latest.as_deref() == Some("The summary after the change."))
    };
    wait_for(&client, "the newer summary", newer).await;
    // Give an answer that was merged by mistake time to show.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(newer(&client.state()), "the older answer won");
}

/// A card names a task that no snapshot holds, and the snapshots say that
/// tasks were left out. The client asks the hub for the task.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_card_for_a_task_outside_the_snapshot_loads_the_task() {
    let (url, hub) = start_hub().await;
    let outside = {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        let (id, card_id) = (TaskId::new(), MessageId::new());
        let mut card = text_message(channel_id, "", Timestamp::now().as_millis() - 50_000, None);
        card.id = card_id;
        card.kind = MessageKind::TaskCard;
        card.author_task_id = Some(id);
        h.messages.push(card);
        let mut task = h.task.clone().unwrap();
        task.id = id;
        task.card_message_id = card_id;
        task.title = "Port the old engine".into();
        task.status = PS::Resolved;
        h.outside_task = Some(task);
        id
    };
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    wait_for(&client, "the card of the task outside the snapshot", |s| {
        s.timeline(&tutti).is_some_and(|t| {
            t.items
                .iter()
                .any(|i| matches!(i, TimelineItem::TaskCard(c) if c.title == "Port the old engine"))
        })
    })
    .await;
    assert_eq!(hub.lock().unwrap().task_gets, vec![outside]);
}

/// Without `tasks_has_more`, a task that is not in the cache does not exist
/// on the hub, and the client does not ask for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_card_for_an_unknown_task_is_not_fetched_when_no_task_was_left_out() {
    let (url, hub) = start_hub().await;
    {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        let mut card = text_message(channel_id, "", Timestamp::now().as_millis() - 50_000, None);
        card.kind = MessageKind::TaskCard;
        card.author_task_id = Some(TaskId::new());
        h.messages.push(card);
    }
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    wait_for(&client, "the channel to load", |s| {
        s.timeline(&tutti).is_some_and(|t| t.loaded)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(hub.lock().unwrap().task_gets.is_empty());
}

/// A thread names a task that the cache does not hold, and tasks were left
/// out of the snapshot. Opening the thread loads the task and its replies.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_a_thread_of_a_task_outside_the_snapshot_loads_the_task_and_its_replies() {
    let (url, hub) = start_hub().await;
    let outside = {
        let mut h = hub.lock().unwrap();
        let channel_id = h.channel.as_ref().unwrap().id;
        let (id, card_id) = (TaskId::new(), MessageId::new());
        let at = Timestamp::now().as_millis() - 50_000;
        // The card is not in the timeline, only its replies are on the hub.
        h.messages.push(text_message(
            channel_id,
            "A reply on an old task",
            at,
            Some(card_id),
        ));
        let mut task = h.task.clone().unwrap();
        task.id = id;
        task.card_message_id = card_id;
        task.title = "An old task".into();
        task.status = PS::Resolved;
        h.outside_task = Some(task);
        id
    };
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    assert!(client.state().thread(&outside.to_string()).is_none());

    client.load_thread(outside.to_string()).await.unwrap();
    let state = wait_for(&client, "the thread of the old task", |s| {
        s.thread(&outside.to_string())
            .is_some_and(|t| !t.posts.is_empty())
    })
    .await;
    let thread = state.thread(&outside.to_string()).unwrap();
    assert_eq!(thread.title, "An old task");
    assert!(matches!(
        &thread.posts[0].bodies[0],
        PostBody::Text(t) if t == "A reply on an old task"
    ));
    assert_eq!(hub.lock().unwrap().task_gets, vec![outside]);
}

// ---------------------------------------------------------------------------
// Review cycle 3: briefs, groups, archive, reconnect, empty channel.

/// Finding 1: the thread holds the whole brief and the whole review
/// summary. Before the client has the full task, it says that the text is a
/// preview.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_thread_of_a_previewed_task_shows_the_full_brief_and_summary_after_it_loads() {
    let (url, hub) = start_hub().await;
    let (task_id, brief, summary) = previewed_task(&hub);
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let id = task_id.to_string();

    let state = wait_for(&client, "the thread", |s| s.thread(&id).is_some()).await;
    let thread = state.thread(&id).unwrap();
    assert!(thread.text_is_preview, "the snapshot cut the text");
    assert!(thread.brief.ends_with(CUT_MARK));
    assert!(thread.brief.len() < brief.len());

    client.load_thread(id.clone()).await.unwrap();
    let state = wait_for(&client, "the full text", |s| {
        s.thread(&id).is_some_and(|t| !t.text_is_preview)
    })
    .await;
    let thread = state.thread(&id).unwrap();
    assert_eq!(thread.brief, brief);
    assert_eq!(thread.review_summary.as_deref(), Some(summary.as_str()));
    assert_eq!(thread.approval_mode, consort_client::ApprovalMode::Ask);
}

/// Finding 8: a group can be made, a channel can go into it, and a channel
/// can leave its group.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_group_can_be_made_and_a_channel_can_leave_its_group() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let channel = tutti_id(&client);

    client
        .create_group("  Home projects ".into(), Some(channel.clone()))
        .await
        .unwrap();
    client.move_channel(channel.clone(), None).await.unwrap();
    let err = client.create_group("  ".into(), None).await.unwrap_err();
    assert!(
        err.to_string().contains("Enter a name for the group"),
        "{err}"
    );

    let recorded = hub.lock().unwrap().recorded.clone();
    let kinds: Vec<&str> = recorded.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kinds, ["group.create", "channel.move", "channel.move"]);
    assert_eq!(recorded[0].1["name"], "Home projects");
    // The channel goes into the group that the hub just made.
    assert_eq!(recorded[1].1["channel_id"], channel.as_str());
    assert!(recorded[1].1["group_id"].is_string());
    // "No group" is a move with no group.
    assert_eq!(recorded[2].1["channel_id"], channel.as_str());
    assert!(recorded[2].1["group_id"].is_null());
}

/// Finding 8: a new channel can name a new group. The client makes the group
/// first, and a second try with the same name uses it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_channel_made_with_a_new_group_makes_the_group_first() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    // The fake hub has no channel.create. The group request goes out first,
    // then the channel request fails with the fake's own answer.
    let new = consort_client::NewChannel {
        name: "garden".into(),
        new_group: Some("Home".into()),
        ..consort_client::NewChannel::default()
    };
    let result = client.create_channel(new).await;
    assert!(result.is_err());
    let recorded = hub.lock().unwrap().recorded.clone();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].0, "group.create");
    assert_eq!(recorded[0].1["name"], "Home");
}

/// Finding 7: archive a task, and bring an archived channel back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_task_can_be_archived_and_a_channel_can_be_unarchived() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let channel = tutti_id(&client);
    let task_id = hub.lock().unwrap().task.as_ref().unwrap().id;

    client.archive_task(task_id.to_string()).await.unwrap();
    client.unarchive_channel(channel.clone()).await.unwrap();
    let recorded = hub.lock().unwrap().recorded.clone();
    assert_eq!(recorded[0].0, "task.archive");
    assert_eq!(recorded[0].1["task_id"], task_id.to_string());
    assert_eq!(recorded[0].1["archived"], true);
    assert_eq!(recorded[1].0, "channel.archive");
    assert_eq!(recorded[1].1["channel_id"], channel.as_str());
    assert_eq!(recorded[1].1["archived"], false);
}

/// Finding 7: a channel that someone archived leaves the channel list and
/// shows in the archived list, and comes back when someone unarchives it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_archived_channel_shows_in_the_archived_list_and_comes_back() {
    let (url, hub) = start_hub().await;
    let archived_id = {
        let mut h = hub.lock().unwrap();
        let mut channel = h.channel.clone().unwrap();
        channel.id = ChannelId::new();
        channel.kind = ChannelKind::Standard;
        channel.name = "Garden".into();
        channel.archived_at = Some(ts(5));
        let id = channel.id;
        h.extra.push(channel);
        id
    };
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let id = archived_id.to_string();
    let state = wait_for(&client, "the archived list", |s| {
        s.archived_channels.iter().any(|c| c.id == id)
    })
    .await;
    assert!(state.channel(&id).is_none());
    assert_eq!(state.archived_channels[0].name, "Garden");

    let upsert = {
        let mut h = hub.lock().unwrap();
        h.extra[0].archived_at = None;
        let open = h.extra[0].clone();
        feed(&mut h, EntityPayload::Channel(open))
    };
    push(&hub, vec![upsert]);
    let state = wait_for(&client, "the channel back in the list", |s| {
        s.channel(&id).is_some()
    })
    .await;
    assert!(state.archived_channels.is_empty());
}

/// Finding 6: a channel ID that is empty names no channel. The client
/// refuses it, and queues nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_for_no_channel_is_refused_and_not_queued() {
    // A paired client whose hub is not there: a cold start while the hub is
    // down, as the user sees it.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("client.json"),
        r#"{"hub_url":"http://127.0.0.1:1","client_id":"cli_00000000-0000-0000-0000-000000000001","name":"Test"}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("client.key"), "11".repeat(32)).unwrap();
    let client =
        Client::start_with(Some(Store::new(dir.path())), ClientOptions::desktop()).unwrap();
    wait_for(&client, "offline", |s| {
        matches!(s.connection, Connection::Offline { .. })
    })
    .await;
    assert!(client.state().channels.is_empty());

    let err = client
        .send_message(String::new(), "my careful message".into())
        .await
        .unwrap_err();
    assert_eq!(err, ClientError::NoChannel);
    assert!(err.to_string().contains("try again"));
    // The same through the command.
    let err = client
        .run(Command::SendMessage {
            channel_id: " ".into(),
            body: "again".into(),
            client_message_id: "cm-1".into(),
            attachments: Vec::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(err, ClientError::NoChannel);
    assert!(
        !dir.path().join("outbox.json").exists(),
        "nothing waits in the outbox"
    );
}

/// Finding 10: "Try again" connects at once. A hub that drops every link
/// makes the retry delay grow, and the command ends the wait.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn try_again_connects_without_waiting_for_the_retry_timer() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    {
        let accepted = accepted.clone();
        tokio::spawn(async move {
            // Take each connection and drop it, so the handshake fails.
            while let Ok((socket, _)) = listener.accept().await {
                accepted.fetch_add(1, Ordering::SeqCst);
                drop(socket);
            }
        });
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("client.json"),
        format!(
            r#"{{"hub_url":"http://127.0.0.1:{port}","client_id":"cli_00000000-0000-0000-0000-000000000001","name":"Test"}}"#
        ),
    )
    .unwrap();
    std::fs::write(dir.path().join("client.key"), "11".repeat(32)).unwrap();
    let client =
        Client::start_with(Some(Store::new(dir.path())), ClientOptions::desktop()).unwrap();
    // Wait until the delay is long enough to tell the difference.
    let state = wait_for(
        &client,
        "a long retry delay",
        |s| matches!(s.connection, Connection::Offline { retry_in_ms, .. } if retry_in_ms >= 1_400),
    )
    .await;
    assert!(matches!(state.connection, Connection::Offline { .. }));
    let before = accepted.load(Ordering::SeqCst);
    client.reconnect().await.unwrap();
    until("a new attempt before the timer ends", || {
        accepted.load(Ordering::SeqCst) > before
    })
    .await;
    // The command was not a wait for the timer: it came well before the
    // delay that the state announced.
    assert!(matches!(
        client.state().connection,
        Connection::Offline { .. } | Connection::Connecting
    ));
}

/// Finding 12: a channel that someone deletes takes its waiting messages
/// out of the outbox, and the user keeps the text as a failed send. The
/// caller that waited for the send does not hang.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_a_channel_keeps_the_text_of_its_waiting_messages() {
    let (url, hub) = start_hub().await;
    let doomed = {
        let mut h = hub.lock().unwrap();
        let mut channel = h.channel.clone().unwrap();
        channel.id = ChannelId::new();
        channel.kind = ChannelKind::Standard;
        channel.name = "scratch".into();
        let id = channel.id;
        h.extra.push(channel);
        id
    };
    let dir = tempfile::tempdir().unwrap();
    // Long limits: the call must not end by its own timeout. Only the
    // delete may end the wait.
    let patient = Timeouts {
        call: Duration::from_secs(60),
        idle: Duration::from_secs(120),
        ..quick()
    };
    let client = paired_client(&url, dir.path(), options(patient)).await;
    let id = doomed.to_string();
    wait_for(&client, "the scratch channel", |s| s.channel(&id).is_some()).await;
    // The hub takes the message and never answers, so it stays in the outbox.
    hub.lock().unwrap().silent_sends = true;
    let sender = {
        let client = client.clone();
        let id = id.clone();
        tokio::spawn(async move { client.send_message(id, "A long message".into()).await })
    };
    wait_for(&client, "the message waits", |s| {
        bodies_of(s, &id)
            .iter()
            .any(|(b, pending)| b == "A long message" && *pending)
    })
    .await;
    let delete = {
        let mut h = hub.lock().unwrap();
        h.seq += 1;
        Frame::event(&FeedEntry::delete(
            h.seq,
            Timestamp::now(),
            consort_proto::client_api::EntityKind::Channel,
            id.clone(),
            Some(doomed),
        ))
        .unwrap()
    };
    push(&hub, vec![delete]);
    let state = wait_for(&client, "the failed send", |s| !s.send_failures.is_empty()).await;
    let failure = &state.send_failures[0];
    assert_eq!(failure.body, "A long message");
    assert_eq!(failure.channel_name, "scratch");
    assert!(
        failure.message.starts_with("The channel was deleted."),
        "{}",
        failure.message
    );
    // The caller that waited for the send is not stuck.
    let done = tokio::time::timeout(Duration::from_secs(5), sender).await;
    assert!(
        done.is_ok(),
        "the send call hangs after the channel was deleted"
    );
}

/// Step 2: a hub card is answered with `approval.answer` and the hub's
/// option ID, a channel switch sends `channel.set_connection_enabled`, the
/// connect flow follows the connection's status from the feed, and the
/// skill directory shows an `unsupported` hub as such.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connectors_and_hub_cards_use_their_requests() {
    use consort_proto::model::{ConnectorCard, DisplayField, ToolKind};
    let (url, hub) = start_hub().await;
    {
        let mut h = hub.lock().unwrap();
        h.record_answers = true;
        // This test checks the message for a hub without the skill pipeline.
        h.skills_unsupported = true;
        let approval = h.approval.as_mut().unwrap();
        let details = ApprovalDetails::Connector(Box::new(ConnectorCard {
            connection_id: consort_proto::ids::ConnectionId::new(),
            connection_name: "Gmail Work".into(),
            service_name: "Gmail".into(),
            tool: "GMAIL_SEND_EMAIL".into(),
            tool_kind: ToolKind::Change,
            fields: vec![DisplayField {
                label: "To".into(),
                value: "dana@acme.dev".into(),
            }],
            arguments: serde_json::json!({"to": "dana@acme.dev"}),
            outgoing: None,
            editable: Vec::new(),
            voice: None,
            flags: Vec::new(),
            draft_arguments: None,
            not_my_voice: false,
            ask_rule: None,
            replaced_by: None,
        }));
        approval.options = details.hub_card_options().unwrap();
        approval.details = details;
        approval.request_id = consort_proto::model::hub_card_request_id(approval.id);
    }
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let channel = tutti_id(&client);
    let approval_id = hub
        .lock()
        .unwrap()
        .approval
        .as_ref()
        .unwrap()
        .id
        .to_string();

    // The card reads as a connector card with the hub's labels.
    let state = wait_for(&client, "the card", |s| {
        s.timelines
            .iter()
            .flat_map(|t| t.items.iter())
            .any(|i| matches!(i, TimelineItem::TaskCard(c) if c.approval.is_some()))
    })
    .await;
    let card = state
        .timelines
        .iter()
        .flat_map(|t| t.items.iter())
        .find_map(|i| match i {
            TimelineItem::TaskCard(c) => c.approval.clone(),
            _ => None,
        })
        .expect("the approval shows on the task card");
    assert!(matches!(
        card.card.as_deref(),
        Some(consort_client::HubCardView::Connector(_))
    ));
    assert_eq!(card.choice_label(ApprovalChoice::AllowOnce), "Allow");
    assert_eq!(card.choice_label(ApprovalChoice::Deny), "Don't");

    client
        .answer_approval(approval_id.clone(), ApprovalChoice::AllowOnce)
        .await
        .unwrap();
    let connection_id = consort_proto::ids::ConnectionId::new().to_string();
    client
        .run(Command::SetChannelConnectionEnabled {
            channel_id: channel.clone(),
            connection_id: connection_id.clone(),
            enabled: false,
        })
        .await
        .unwrap();
    let started = client
        .run_for(Command::Connect(consort_client::NewConnection {
            service: "github".into(),
            name: "GitHub Work".into(),
            use_in: consort_client::UseInView::Tags(vec!["work".into()]),
            ..consort_client::NewConnection::default()
        }))
        .await
        .unwrap();
    let consort_client::CommandOutput::ConnectStarted {
        connection_id: new_id,
        redirect_url,
    } = started
    else {
        panic!("a connect attempt");
    };
    assert_eq!(
        redirect_url.as_deref(),
        Some("https://connect.composio.dev/link/abc")
    );
    wait_for(&client, "the connection turns active", |s| {
        s.connection(&new_id).is_some_and(|c| c.status.is_active())
    })
    .await;

    let recorded = hub.lock().unwrap().recorded.clone();
    let kinds: Vec<&str> = recorded.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "approval.answer",
            "channel.set_connection_enabled",
            "connection.connect"
        ]
    );
    assert_eq!(recorded[0].1["approval_id"], approval_id.as_str());
    assert_eq!(recorded[0].1["option_id"], "allow");
    assert_eq!(recorded[1].1["channel_id"], channel.as_str());
    assert_eq!(recorded[1].1["connection_id"], connection_id.as_str());
    assert_eq!(recorded[1].1["enabled"], false);
    assert_eq!(recorded[2].1["use_in"]["kind"], "tags");
    assert_eq!(recorded[2].1["approvals"]["change"], "ask_every_time");

    // A hub without the skill pipeline.
    client
        .run(Command::SearchSkillDirectory {
            search: "rust".into(),
            source: None,
        })
        .await
        .unwrap();
    let state = wait_for(&client, "the directory answer", |s| {
        s.skill_directory.load != consort_client::LoadState::NotLoaded
            && s.skill_directory.load != consort_client::LoadState::Loading
    })
    .await;
    assert_eq!(
        state.skill_directory.load,
        consort_client::LoadState::Unsupported(
            "This hub cannot search skill directories yet. Update the hub, then try again.".into()
        )
    );
    let err = client
        .run(Command::PreviewSkill {
            source: consort_client::SkillInstallInput::Git {
                url: "https://github.com/anthropics/skills".into(),
                path: None,
                git_ref: None,
            },
        })
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "This hub cannot add skills yet. Update the hub, then try again."
    );
}

// ---------------------------------------------------------------------------
// Step 3: the Add skill flow against a fake hub that supports skills.

/// The words of an `unsupported` answer, as the user reads them.
fn cannot(what: &str) -> String {
    format!("This hub cannot {what} yet. Update the hub, then try again.")
}

/// The requests of one type that the hub recorded, in order.
fn recorded_of(hub: &Shared, kind: &str) -> Vec<serde_json::Value> {
    hub.lock()
        .unwrap()
        .recorded
        .iter()
        .filter(|(k, _)| k == kind)
        .map(|(_, body)| body.clone())
        .collect()
}

fn entry_names(state: &ViewState) -> Vec<&str> {
    state
        .skill_directory
        .entries
        .iter()
        .map(|e| e.name.as_str())
        .collect()
}

fn directory_input(entry_id: &str) -> consort_client::SkillInstallInput {
    consort_client::SkillInstallInput::Directory {
        source: None,
        entry_id: entry_id.into(),
    }
}

fn git_input(url: &str, path: Option<&str>) -> consort_client::SkillInstallInput {
    consort_client::SkillInstallInput::Git {
        url: url.into(),
        path: path.map(Into::into),
        git_ref: None,
    }
}

async fn preview_of_skill(
    client: &Client,
    source: consort_client::SkillInstallInput,
) -> Result<consort_client::SkillPreviewView, ClientError> {
    match client.run_for(Command::PreviewSkill { source }).await? {
        consort_client::CommandOutput::SkillPreview(view) => Ok(view),
        other => panic!("a skill preview, not {other:?}"),
    }
}

async fn install_skill_input(
    client: &Client,
    source: consort_client::SkillInstallInput,
    enabled: Option<bool>,
    approval_id: Option<String>,
) -> Result<String, ClientError> {
    let output = client
        .run_for(Command::InstallSkill {
            source,
            use_in: consort_client::UseInView::Tags(vec!["work".into()]),
            enabled,
            approval_id,
        })
        .await?;
    match output {
        consort_client::CommandOutput::SkillInstalled(id) => Ok(id),
        other => panic!("an installed skill, not {other:?}"),
    }
}

/// The first page has a cursor, "load more" adds the second page, and a new
/// search starts again. `search` and `source` reach the hub.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skill_search_pages_and_load_more_fill_the_directory() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;

    client
        .run(Command::SearchSkillDirectory {
            search: String::new(),
            source: None,
        })
        .await
        .unwrap();
    let state = wait_for(&client, "the first page", |s| {
        s.skill_directory.load == consort_client::LoadState::Loaded
    })
    .await;
    assert_eq!(entry_names(&state), ["pdf-forms", "rust-review"]);
    assert_eq!(state.skill_directory.next_cursor.as_deref(), Some("2"));
    assert_eq!(state.skill_directory.unavailable, ["skills.sh"]);
    let review = &state.skill_directory.entries[1];
    assert_eq!(review.entry_id, "smithery:rust-review");
    assert_eq!(review.directory, consort_client::DirectoryKind::Smithery);
    assert_eq!(
        review.audits[0].status,
        consort_client::AuditStatusView::Warning
    );

    client.run(Command::LoadMoreSkillDirectory).await.unwrap();
    let state = wait_for(&client, "the second page", |s| {
        s.skill_directory.entries.len() == 3
    })
    .await;
    assert_eq!(
        entry_names(&state),
        ["pdf-forms", "rust-review", "rust-test-writer"]
    );
    assert_eq!(state.skill_directory.next_cursor, None, "the last page");
    assert_eq!(
        state.skill_directory.more,
        consort_client::LoadState::NotLoaded
    );
    assert_eq!(
        state.skill_directory.entries[2].source_id.as_deref(),
        Some(MARKETPLACE_URL)
    );
    // With no next page, "load more" asks the hub for nothing.
    client.run(Command::LoadMoreSkillDirectory).await.unwrap();

    let searches = recorded_of(&hub, "skill_directory.search");
    assert_eq!(searches.len(), 2);
    assert!(searches[0]["search"].is_null(), "no words: {}", searches[0]);
    assert!(searches[0]["cursor"].is_null());
    assert_eq!(searches[1]["cursor"], "2");

    // New words replace the old results.
    client
        .run(Command::SearchSkillDirectory {
            search: "rust".into(),
            source: None,
        })
        .await
        .unwrap();
    let state = wait_for(&client, "the rust search", |s| {
        s.skill_directory.search == "rust"
            && s.skill_directory.load == consort_client::LoadState::Loaded
    })
    .await;
    assert_eq!(entry_names(&state), ["rust-review", "rust-test-writer"]);
    assert_eq!(state.skill_directory.next_cursor, None);

    // A search for one source sends it, and the hub honours it.
    client
        .run(Command::SearchSkillDirectory {
            search: String::new(),
            source: Some("anthropic".into()),
        })
        .await
        .unwrap();
    let state = wait_for(&client, "the anthropic search", |s| {
        s.skill_directory.source.as_deref() == Some("anthropic")
            && s.skill_directory.load == consort_client::LoadState::Loaded
    })
    .await;
    assert_eq!(entry_names(&state), ["pdf-forms"]);
    let searches = recorded_of(&hub, "skill_directory.search");
    assert_eq!(searches.len(), 4);
    assert_eq!(searches[2]["search"], "rust");
    assert!(searches[2]["source"].is_null());
    assert_eq!(searches[3]["source"], "anthropic");

    // Words and a source together: nothing matches, and that is not an error.
    client
        .run(Command::SearchSkillDirectory {
            search: "rust".into(),
            source: Some("anthropic".into()),
        })
        .await
        .unwrap();
    let state = wait_for(&client, "the empty search", |s| {
        s.skill_directory.search == "rust"
            && s.skill_directory.source.as_deref() == Some("anthropic")
            && s.skill_directory.load == consort_client::LoadState::Loaded
    })
    .await;
    assert!(state.skill_directory.entries.is_empty());
}

/// A preview shows the facts of a directory entry and of a git URL. A
/// blocked skill keeps the reason of the hub.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skill_preview_shows_the_facts_of_an_entry_and_of_a_git_url() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;

    let entry = preview_of_skill(&client, directory_input("market:rust-test-writer"))
        .await
        .unwrap();
    assert_eq!(entry.name, "rust-test-writer");
    assert_eq!(entry.description, "Write Rust tests and run them.");
    assert_eq!(
        entry.commit.as_deref(),
        Some("3f2a91c"),
        "cut to 7 characters"
    );
    assert_eq!(entry.license.as_deref(), Some("MIT"));
    assert_eq!(entry.scripts, ["scripts/run-tests.sh"]);
    assert_eq!(entry.file_count, 2);
    assert!(entry.has_hooks);
    assert_eq!(entry.blocked_reason, None);
    assert_eq!(entry.installed_skill_id, None);
    assert_eq!(entry.audits.len(), 1);
    assert_eq!(entry.audits[0].source, "Socket");
    assert_eq!(
        entry.audits[0].status,
        consort_client::AuditStatusView::Passed
    );

    let git = preview_of_skill(
        &client,
        git_input("https://github.com/acme/rust-skills", Some("review")),
    )
    .await
    .unwrap();
    assert_eq!(git.name, "review");
    assert_eq!(
        git.description,
        "A skill from https://github.com/acme/rust-skills."
    );
    assert_eq!(git.license.as_deref(), Some("Apache-2.0"));
    assert!(git.scripts.is_empty());
    assert_eq!(git.file_count, 1);
    assert!(!git.has_hooks);
    assert_eq!(git.blocked_reason, None);

    let blocked = preview_of_skill(
        &client,
        git_input("https://github.com/acme/blocked-skill", None),
    )
    .await
    .unwrap();
    assert_eq!(blocked.name, "blocked-skill");
    assert_eq!(
        blocked.blocked_reason.as_deref(),
        Some("The audit failed: the skill reads files outside its folder.")
    );

    let sent = recorded_of(&hub, "skill.preview");
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[0]["source"]["kind"], "directory");
    assert_eq!(sent[0]["source"]["entry_id"], "market:rust-test-writer");
    assert_eq!(sent[1]["source"]["kind"], "git");
    assert_eq!(
        sent[1]["source"]["url"],
        "https://github.com/acme/rust-skills"
    );
    assert_eq!(sent[1]["source"]["path"], "review");
    assert!(
        hub.lock().unwrap().skills.is_empty(),
        "a preview installs nothing"
    );
    assert!(client.state().skills.is_empty());
}

/// An install returns the skill ID and adds the skill to the list, with the
/// scope and the switch that the user chose. A skill with scripts stays off.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installing_a_skill_returns_its_id_and_adds_it_to_the_list() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;

    let id = install_skill_input(
        &client,
        directory_input("anthropic:pdf-forms"),
        Some(true),
        None,
    )
    .await
    .unwrap();
    let state = wait_for(&client, "the skill in the list", |s| s.skill(&id).is_some()).await;
    let skill = state.skill(&id).unwrap();
    assert_eq!(skill.name, "pdf-forms");
    assert!(skill.enabled);
    assert_eq!(
        skill.use_in,
        consort_client::UseInView::Tags(vec!["work".into()])
    );
    assert_eq!(skill.commit.as_deref(), Some("3f2a91c"));
    assert_eq!(skill.license.as_deref(), Some("MIT"));
    assert!(matches!(
        &skill.source,
        consort_client::SkillSourceView::Directory { entry_id, .. } if entry_id == "anthropic:pdf-forms"
    ));
    assert_eq!(skill.file_count, 1);
    assert_eq!(state.skills.len(), 1);

    let sent = recorded_of(&hub, "skill.install");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["source"]["kind"], "directory");
    assert_eq!(sent[0]["source"]["entry_id"], "anthropic:pdf-forms");
    assert_eq!(sent[0]["use_in"]["kind"], "tags");
    assert_eq!(sent[0]["use_in"]["tags"][0], "work");
    assert_eq!(sent[0]["enabled"], true);
    assert!(sent[0]["approval_id"].is_null());

    // The preview now names the installed skill.
    let preview = preview_of_skill(&client, directory_input("anthropic:pdf-forms"))
        .await
        .unwrap();
    assert_eq!(preview.installed_skill_id.as_deref(), Some(id.as_str()));

    // A git skill that the user turns off stays off.
    let off = install_skill_input(
        &client,
        git_input("https://github.com/acme/rust-skills", Some("review")),
        Some(false),
        None,
    )
    .await
    .unwrap();
    let state = wait_for(&client, "the skill that is off", |s| {
        s.skill(&off).is_some()
    })
    .await;
    assert!(!state.skill(&off).unwrap().enabled);
    assert_eq!(recorded_of(&hub, "skill.install")[1]["enabled"], false);
    assert_eq!(
        recorded_of(&hub, "skill.install")[1]["source"]["kind"],
        "git"
    );

    // A skill with scripts is off, whatever the user asked.
    let scripted = install_skill_input(
        &client,
        directory_input("market:rust-test-writer"),
        Some(true),
        None,
    )
    .await
    .unwrap();
    let state = wait_for(&client, "the skill with scripts", |s| {
        s.skill(&scripted).is_some()
    })
    .await;
    let scripted = state.skill(&scripted).unwrap();
    assert!(scripted.has_scripts());
    assert!(!scripted.enabled);
    assert!(scripted.hooks_removed);

    // The same skill again, and a blocked skill, are refused in the hub's
    // words. The list does not change.
    let err = install_skill_input(&client, directory_input("anthropic:pdf-forms"), None, None)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "A skill named pdf-forms is installed already."
    );
    let err = install_skill_input(
        &client,
        git_input("https://github.com/acme/blocked-skill", None),
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("The hub blocks this skill."),
        "{err}"
    );
    assert_eq!(client.state().skills.len(), 3);
}

fn install_card_state(state: &ViewState) -> Option<consort_client::ApprovalView> {
    state
        .timelines
        .iter()
        .flat_map(|t| t.items.iter())
        .find_map(|i| match i {
            TimelineItem::Approval(c) => Some(c.approval.clone()),
            _ => None,
        })
        .filter(|a| {
            matches!(
                a.card.as_deref(),
                Some(consort_client::HubCardView::SkillInstall(_))
            )
        })
}

/// The install card reaches the timeline as an install card. An install
/// with its `approval_id` closes it and names the new skill.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_install_card_closes_when_the_skill_is_added_from_it() {
    use consort_proto::model::SkillInstallCard;
    let (url, hub) = start_hub().await;
    let card_id = {
        let mut h = hub.lock().unwrap();
        // The hub posts its card as an approval message of Tutti, with no
        // task. The timeline shows the card from that message.
        let (message_id, channel_id) = {
            let approval = h.approval.as_ref().unwrap();
            (approval.message_id, approval.channel_id)
        };
        h.messages.push(Message {
            id: message_id,
            channel_id,
            thread_root_id: None,
            author: Author::Assistant,
            author_session_id: None,
            author_task_id: None,
            author_routine_id: None,
            kind: MessageKind::Approval,
            body: String::new(),
            attachments: vec![],
            in_timeline: false,
            client_message_id: None,
            reply_count: 0,
            last_reply_at: None,
            reactions: vec![],
            edited_at: None,
            deleted_at: None,
            created_at: ts(Timestamp::now().as_millis() - 3_000),
            proactive: None,
            suggestions: Vec::new(),
            source_message_id: None,
        });
        let approval = h.approval.as_mut().unwrap();
        approval.task_id = None;
        let details = ApprovalDetails::SkillInstall(Box::new(SkillInstallCard {
            name: "pdf-forms".into(),
            description: "Fill in PDF forms.".into(),
            source: SkillSource::Directory {
                directory: SkillDirectoryKind::Anthropic,
                entry_id: "anthropic:pdf-forms".into(),
                url: "https://github.com/anthropics/skills".into(),
                path: Some("pdf".into()),
            },
            commit: Some(FAKE_COMMIT.into()),
            license: Some("MIT".into()),
            scripts: Vec::new(),
            audits: Vec::new(),
            use_in: consort_proto::model::UseIn::EveryChannel,
            blocked_reason: None,
            skill_id: None,
        }));
        approval.options = details.hub_card_options().unwrap();
        approval.details = details;
        approval.request_id = consort_proto::model::hub_card_request_id(approval.id);
        approval.id.to_string()
    };
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;

    let state = wait_for(&client, "the install card", |s| {
        install_card_state(s).is_some()
    })
    .await;
    let card = install_card_state(&state).unwrap();
    assert_eq!(card.id, card_id);
    assert_eq!(card.state, consort_client::RequestState::Pending);
    let Some(consort_client::HubCardView::SkillInstall(view)) = card.card.as_deref() else {
        panic!("an install card");
    };
    assert_eq!(view.name, "pdf-forms");
    assert_eq!(view.license.as_deref(), Some("MIT"));
    assert_eq!(view.commit.as_deref(), Some("3f2a91c"));
    assert_eq!(view.skill_id, None);
    assert_eq!(
        card.button_labels
            .iter()
            .map(|(_, label)| label.as_str())
            .collect::<Vec<_>>(),
        ["Add", "Not now"]
    );

    // A card that the hub does not know: the install fails, and no skill
    // is added.
    let err = install_skill_input(
        &client,
        directory_input("anthropic:pdf-forms"),
        None,
        Some(ApprovalId::new().to_string()),
    )
    .await
    .unwrap_err();
    assert_eq!(err.to_string(), "That install card does not exist.");
    assert!(client.state().skills.is_empty());
    assert_eq!(
        install_card_state(&client.state()).unwrap().state,
        consort_client::RequestState::Pending,
        "the card still waits"
    );

    let id = install_skill_input(
        &client,
        directory_input("anthropic:pdf-forms"),
        None,
        Some(card_id.clone()),
    )
    .await
    .unwrap();
    let sent = recorded_of(&hub, "skill.install");
    assert_eq!(sent.last().unwrap()["approval_id"], card_id.as_str());

    // The feed closes the card, and the card names the skill.
    // The feed entry of the card and the answer of the install arrive in
    // either order, so wait for both.
    let state = wait_for(&client, "the card closed and the skill added", |s| {
        install_card_state(s).is_some_and(|c| c.state != consort_client::RequestState::Pending)
            && s.skill(&id).is_some()
    })
    .await;
    let card = install_card_state(&state).unwrap();
    assert_eq!(card.state, consort_client::RequestState::Answered);
    assert_eq!(card.answer, Some(ApprovalChoice::AllowOnce));
    let Some(consort_client::HubCardView::SkillInstall(view)) = card.card.as_deref() else {
        panic!("an install card");
    };
    assert_eq!(view.skill_id.as_deref(), Some(id.as_str()));
    assert!(state.skill(&id).is_some());
}

/// The sources of the skill directories: list, add and remove. A bad URL
/// gives an error that says what happened.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skill_directory_sources_are_listed_added_and_removed() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let sources = |s: &ViewState| -> Vec<(String, bool)> {
        s.skill_directory
            .sources
            .iter()
            .map(|d| (d.id.clone(), d.removable))
            .collect()
    };

    client
        .run(Command::LoadSkillDirectorySources)
        .await
        .unwrap();
    let state = wait_for(&client, "the sources", |s| {
        !s.skill_directory.sources.is_empty()
    })
    .await;
    assert_eq!(
        sources(&state),
        [
            ("smithery".to_owned(), false),
            ("anthropic".to_owned(), false),
            (MARKETPLACE_URL.to_owned(), true),
        ]
    );
    assert_eq!(
        state.skill_directory.sources[0].kind,
        consort_client::DirectoryKind::Smithery
    );
    assert_eq!(
        state.skill_directory.sources[2].kind,
        consort_client::DirectoryKind::Marketplace
    );
    assert_eq!(state.skill_directory.sources[2].name, "Team plugins");

    client
        .run(Command::AddSkillDirectorySource {
            url: " https://example.com/more/marketplace.json ".into(),
        })
        .await
        .unwrap();
    let state = wait_for(&client, "the added source", |s| {
        s.skill_directory.sources.len() == 4
    })
    .await;
    let added = state.skill_directory.sources.last().unwrap();
    assert_eq!(added.id, "https://example.com/more/marketplace.json");
    assert_eq!(added.kind, consort_client::DirectoryKind::Marketplace);
    assert!(added.removable);
    assert_eq!(
        recorded_of(&hub, "skill_directory.add_source")[0]["url"],
        "https://example.com/more/marketplace.json",
        "the client sends the trimmed URL"
    );

    // The hub refuses a URL that is not a link. The list does not change.
    let err = client
        .run(Command::AddSkillDirectorySource {
            url: "not a url".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "\"not a url\" is not a link to a marketplace.json file. Paste an https:// link, then try again."
    );
    assert!(matches!(&err, ClientError::Hub { code, .. } if code == "invalid"));
    assert_eq!(client.state().skill_directory.sources.len(), 4);

    // An empty URL stops in the client. The hub sees no request.
    let before = recorded_of(&hub, "skill_directory.add_source").len();
    let err = client
        .run(Command::AddSkillDirectorySource { url: "  ".into() })
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Paste the URL of a git repo or a marketplace.json file."
    );
    assert_eq!(
        recorded_of(&hub, "skill_directory.add_source").len(),
        before
    );

    // A built-in source stays. An unknown one is not found.
    let err = client
        .run(Command::RemoveSkillDirectorySource {
            id: "smithery".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Smithery is built in and stays in the list."
    );
    let err = client
        .run(Command::RemoveSkillDirectorySource { id: "nope".into() })
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "That directory is not in the list.");
    assert_eq!(client.state().skill_directory.sources.len(), 4);

    client
        .run(Command::RemoveSkillDirectorySource {
            id: MARKETPLACE_URL.into(),
        })
        .await
        .unwrap();
    let state = wait_for(&client, "the source removed", |s| {
        s.skill_directory.sources.len() == 3
    })
    .await;
    assert_eq!(
        sources(&state),
        [
            ("smithery".to_owned(), false),
            ("anthropic".to_owned(), false),
            ("https://example.com/more/marketplace.json".to_owned(), true),
        ]
    );
    let removed = recorded_of(&hub, "skill_directory.remove_source");
    assert_eq!(removed.last().unwrap()["id"], MARKETPLACE_URL);
}

/// The audit log of a connection comes in pages, newest first, with the
/// facts of each call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_calls_come_in_pages_with_their_facts() {
    let (url, hub) = start_hub().await;
    let connection = consort_proto::ids::ConnectionId::new();
    let other = consort_proto::ids::ConnectionId::new();
    {
        let mut h = hub.lock().unwrap();
        h.calls = connector_calls(connection, 3);
    }
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let wanted = connection.to_string();

    let consort_client::CommandOutput::ConnectionCalls(first) = client
        .run_for(Command::LoadConnectionCalls {
            connection_id: wanted.clone(),
            before: None,
        })
        .await
        .unwrap()
    else {
        panic!("a page of calls");
    };
    assert_eq!(first.connection_id, wanted);
    assert!(first.has_more);
    assert_eq!(first.calls.len(), 2);
    let denied = &first.calls[0];
    assert_eq!(denied.tool, "GMAIL_TOOL_0");
    assert_eq!(denied.kind, consort_client::ToolKindView::Change);
    assert_eq!(denied.decision, "Denied by you");
    assert!(!denied.succeeded);
    assert_eq!(denied.size_label, "2.0 KB");
    assert!(
        denied.time_label.ends_with("AM") || denied.time_label.ends_with("PM"),
        "a clock time: {}",
        denied.time_label
    );
    assert!(!denied.day_label.is_empty());
    let allowed = &first.calls[1];
    assert_eq!(allowed.tool, "GMAIL_TOOL_1");
    assert_eq!(allowed.kind, consort_client::ToolKindView::Read);
    assert_eq!(allowed.decision, "Allowed by you");
    assert!(allowed.succeeded);
    assert_eq!(allowed.size_label, "412 B");

    // The second page starts after the last call of the first.
    let consort_client::CommandOutput::ConnectionCalls(second) = client
        .run_for(Command::LoadConnectionCalls {
            connection_id: wanted.clone(),
            before: Some(allowed.id.clone()),
        })
        .await
        .unwrap()
    else {
        panic!("a page of calls");
    };
    assert!(!second.has_more);
    assert_eq!(second.calls.len(), 1);
    assert_eq!(second.calls[0].tool, "GMAIL_TOOL_2");
    assert_eq!(second.calls[0].size_label, "12.4 KB");
    assert_ne!(second.calls[0].id, allowed.id);

    let sent = recorded_of(&hub, "connection.calls");
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0]["connection_id"], wanted.as_str());
    assert!(sent[0]["before"].is_null());
    assert_eq!(sent[1]["before"], allowed.id.as_str());

    // A connection with no calls gives an empty page.
    let consort_client::CommandOutput::ConnectionCalls(none) = client
        .run_for(Command::LoadConnectionCalls {
            connection_id: other.to_string(),
            before: None,
        })
        .await
        .unwrap()
    else {
        panic!("a page of calls");
    };
    assert!(none.calls.is_empty());
    assert!(!none.has_more);
}

/// A hub that answers `unsupported` gives the generic "cannot ... yet" text
/// for search, preview, install and the source requests.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unsupported_hub_gives_the_generic_text_for_every_skill_request() {
    let (url, hub) = start_hub().await;
    hub.lock().unwrap().skills_unsupported = true;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;

    client
        .run(Command::SearchSkillDirectory {
            search: "rust".into(),
            source: None,
        })
        .await
        .unwrap();
    let state = wait_for(&client, "the search answer", |s| {
        matches!(
            s.skill_directory.load,
            consort_client::LoadState::Unsupported(_)
        )
    })
    .await;
    assert_eq!(
        state.skill_directory.load,
        consort_client::LoadState::Unsupported(cannot("search skill directories"))
    );
    assert!(state.skill_directory.entries.is_empty());

    let err = preview_of_skill(&client, directory_input("anthropic:pdf-forms"))
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), cannot("add skills"));
    let err = install_skill_input(&client, directory_input("anthropic:pdf-forms"), None, None)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), cannot("add skills"));
    let err = client
        .run(Command::AddSkillDirectorySource {
            url: "https://example.com/marketplace.json".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), cannot("add skill directories"));
    let err = client
        .run(Command::RemoveSkillDirectorySource {
            id: MARKETPLACE_URL.into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), cannot("remove skill directories"));
    assert!(client.state().skills.is_empty());
}

/// An error that is not `unsupported` keeps the hub's own words. The
/// "cannot ... yet" text is only for an `unsupported` hub.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_skill_errors_do_not_give_the_unsupported_text() {
    let (url, _hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let generic = "Update the hub, then try again.";

    // An `invalid` answer.
    let err = preview_of_skill(&client, git_input("https://github.com/acme/broken", None))
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "The hub found no skill at that address. Check the URL, then try again."
    );
    assert!(matches!(&err, ClientError::Hub { code, .. } if code == "invalid"));
    assert!(!err.to_string().contains(generic));

    // An `internal` answer.
    let err = preview_of_skill(&client, git_input("https://github.com/acme/flaky", None))
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "The fake hub fails this preview.");
    assert!(matches!(&err, ClientError::Hub { code, .. } if code == "internal"));

    // The same for an install.
    let err = install_skill_input(
        &client,
        git_input("https://github.com/acme/broken", None),
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(&err, ClientError::Hub { code, .. } if code == "invalid"));
    assert!(!err.to_string().contains(generic));

    // A broken link: the hub closes the socket before it answers.
    let err = preview_of_skill(&client, git_input("https://github.com/acme/dropped", None))
        .await
        .unwrap_err();
    assert!(!err.to_string().contains(generic), "{err}");
    assert!(!matches!(&err, ClientError::Hub { code, .. } if code == "unsupported"));

    // The client connects again, and the next preview works.
    wait_for(&client, "online again", |s| {
        s.connection == Connection::Online
    })
    .await;
    let ok = preview_of_skill(&client, git_input("https://github.com/acme/fine", None))
        .await
        .unwrap();
    assert_eq!(ok.name, "fine");
}

// Phase 1: files and attachments --------------------------------------------

fn sha_of(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let digest: [u8; 32] = sha2::Sha256::digest(bytes).into();
    consort_proto::model::Sha256Hash::from_bytes(digest).to_string()
}

fn blob_log(hub: &Shared) -> Vec<(String, String)> {
    hub.lock().unwrap().blob_log.clone()
}

/// An attached file goes up once (`HEAD`, then `PUT` with the token from
/// `auth.http_token`), and `message.send` carries its blob. The same file a
/// second time needs no `PUT`. A token that the hub forgot is replaced.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attached_file_goes_up_once_and_the_message_carries_it() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    let file = dir.path().join("brightline-quote.pdf");
    std::fs::write(&file, b"%PDF the quote").unwrap();
    let sha = sha_of(b"%PDF the quote");
    let target = consort_client::UploadTarget::Composer {
        channel_id: tutti.clone(),
    };
    let ids = client
        .attach(
            target.clone(),
            vec![consort_client::AttachInput::Path(
                file.display().to_string(),
            )],
        )
        .await
        .unwrap();
    assert_eq!(ids.len(), 1);
    wait_for(&client, "the upload is ready", |s| {
        s.uploads
            .iter()
            .any(|u| u.status == consort_client::UploadStatus::Ready)
    })
    .await;
    assert_eq!(
        blob_log(&hub),
        [
            ("HEAD".to_owned(), sha.clone()),
            ("PUT".to_owned(), sha.clone())
        ]
    );
    client
        .send_message_with(tutti.clone(), "Here is the quote.".into(), ids)
        .await
        .unwrap();
    {
        let h = hub.lock().unwrap();
        let sent = h.sent_attachments.last().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].sha256.to_string(), sha);
        assert_eq!(sent[0].name, "brightline-quote.pdf");
        assert_eq!(sent[0].mime, "application/pdf");
        assert_eq!(sent[0].size, 14);
    }
    let state = wait_for(&client, "the message with its file", |s| {
        s.uploads.is_empty()
            && s.timeline(&tutti).is_some_and(|t| {
                t.items.iter().any(|i| {
                    matches!(i, TimelineItem::Message(m)
                        if m.attachments.iter().any(|a| a.name == "brightline-quote.pdf"))
                })
            })
    })
    .await;
    // The file that went up is in the local cache, so its view has a path.
    let local = state
        .timeline(&tutti)
        .and_then(|t| {
            t.items.iter().find_map(|i| match i {
                TimelineItem::Message(m) => m.attachments.first().cloned(),
                _ => None,
            })
        })
        .and_then(|a| a.local_path);
    assert!(local.is_some());

    // The hub forgets the token. The same file again: the client gets a
    // new token, asks, and sends nothing, because the hub has the blob.
    hub.lock().unwrap().http_tokens.clear();
    let before = blob_log(&hub).len();
    client
        .attach(
            target.clone(),
            vec![consort_client::AttachInput::Path(
                file.display().to_string(),
            )],
        )
        .await
        .unwrap();
    wait_for(&client, "the second upload is ready", |s| {
        s.uploads
            .iter()
            .any(|u| u.status == consort_client::UploadStatus::Ready)
    })
    .await;
    let log = blob_log(&hub);
    assert!(
        log[before..].iter().all(|(method, _)| method == "HEAD"),
        "{log:?}"
    );
    assert_eq!(hub.lock().unwrap().http_tokens.len(), 1, "a new token");
}

/// A send waits for its uploads. A failed upload stops it with a message
/// that says what to do, and the text does not go out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_upload_stops_the_send_until_it_is_removed() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    let target = consort_client::UploadTarget::Composer {
        channel_id: tutti.clone(),
    };
    let missing = dir.path().join("gone.heic").display().to_string();
    let ids = client
        .attach(
            target.clone(),
            vec![consort_client::AttachInput::Path(missing)],
        )
        .await
        .unwrap();
    let state = wait_for(&client, "the upload fails", |s| {
        s.uploads
            .iter()
            .any(|u| matches!(u.status, consort_client::UploadStatus::Failed(_)))
    })
    .await;
    let consort_client::UploadStatus::Failed(message) = &state.uploads[0].status else {
        unreachable!()
    };
    assert!(message.starts_with("Upload failed."), "{message}");
    let err = client
        .send_message_with(tutti.clone(), "With the photo".into(), ids.clone())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Try again"), "{err}");
    assert!(
        !hub.lock()
            .unwrap()
            .sent_bodies
            .contains(&"With the photo".to_owned())
    );
    client
        .run(Command::RemoveUpload { id: ids[0].clone() })
        .await
        .unwrap();
    client
        .send_message(tutti.clone(), "Without the photo".into())
        .await
        .unwrap();
    assert!(
        hub.lock()
            .unwrap()
            .sent_bodies
            .contains(&"Without the photo".to_owned())
    );
}

/// The Files tab: `files.list`, an upload into a folder that becomes a
/// channel file with `files.put`, a move and a delete. A download fills the
/// local cache.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_files_load_upload_move_download_and_delete() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    client
        .run(Command::LoadChannelFiles {
            channel_id: tutti.clone(),
        })
        .await
        .unwrap();
    wait_for(&client, "the files are loaded", |s| {
        s.files_loaded.contains(&tutti)
    })
    .await;
    let photo = dir.path().join("roof.png");
    std::fs::write(&photo, b"not really a png").unwrap();
    client
        .attach(
            consort_client::UploadTarget::ChannelFiles {
                channel_id: tutti.clone(),
                folder: "photos".into(),
            },
            vec![consort_client::AttachInput::Path(
                photo.display().to_string(),
            )],
        )
        .await
        .unwrap();
    let state = wait_for(&client, "the file is in the channel files", |s| {
        s.uploads.is_empty() && s.channel_files.iter().any(|f| f.path == "photos/roof.png")
    })
    .await;
    let file = state
        .channel_files
        .iter()
        .find(|f| f.path == "photos/roof.png")
        .unwrap()
        .clone();
    assert_eq!(file.folder, "photos");
    assert_eq!(file.mime, "image/png");
    assert_eq!(hub.lock().unwrap().files.len(), 1);

    // A download goes to the cache once, and a named copy can open.
    let cache = dir.path().join("blobs");
    std::fs::remove_dir_all(&cache).ok();
    let path = client
        .fetch_blob(file.sha256.clone(), file.size, Some("roof.png".into()))
        .await
        .unwrap();
    assert!(path.ends_with("roof.png"));
    assert_eq!(std::fs::read(&path).unwrap(), b"not really a png");
    let gets = |hub: &Shared| blob_log(hub).iter().filter(|(m, _)| m == "GET").count();
    assert_eq!(gets(&hub), 1);
    client
        .fetch_blob(file.sha256.clone(), file.size, None)
        .await
        .unwrap();
    assert_eq!(gets(&hub), 1, "the second read comes from the cache");

    client
        .run(Command::MoveChannelFile {
            channel_id: tutti.clone(),
            from_path: "photos/roof.png".into(),
            to_path: "roof-front.png".into(),
        })
        .await
        .unwrap();
    wait_for(&client, "the moved file", |s| {
        s.channel_files.len() == 1 && s.channel_files[0].path == "roof-front.png"
    })
    .await;
    client
        .run(Command::DeleteChannelFile {
            channel_id: tutti.clone(),
            path: "roof-front.png".into(),
        })
        .await
        .unwrap();
    wait_for(&client, "no file", |s| s.channel_files.is_empty()).await;
    assert!(hub.lock().unwrap().files.is_empty());
}

/// Add skill > Upload: the zip goes up through the blob upload first, and
/// `skill.preview` names it with `Upload { blob_sha256, size, file_name }`.
/// A file that is not a zip never leaves the computer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_skill_zip_goes_up_before_the_preview() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let zip = dir.path().join("porch-lights.zip");
    std::fs::write(&zip, b"PK zip bytes").unwrap();
    let output = client
        .run_for(Command::PreviewSkill {
            source: consort_client::SkillInstallInput::Upload {
                file_path: zip.display().to_string(),
            },
        })
        .await
        .unwrap();
    let consort_client::CommandOutput::SkillPreview(preview) = output else {
        panic!("a preview")
    };
    assert_eq!(preview.name, "porch-lights");
    let sha = sha_of(b"PK zip bytes");
    assert!(blob_log(&hub).contains(&("PUT".to_owned(), sha.clone())));
    let recorded = hub.lock().unwrap().recorded.clone();
    let (_, body) = recorded
        .iter()
        .rev()
        .find(|(kind, _)| kind == "skill.preview")
        .unwrap();
    assert_eq!(body["source"]["kind"], "upload");
    assert_eq!(body["source"]["blob_sha256"], sha);
    assert_eq!(body["source"]["size"], 12);
    assert_eq!(body["source"]["file_name"], "porch-lights.zip");

    let text = dir.path().join("notes.txt");
    std::fs::write(&text, b"hello").unwrap();
    let before = blob_log(&hub).len();
    let err = client
        .run_for(Command::PreviewSkill {
            source: consort_client::SkillInstallInput::Upload {
                file_path: text.display().to_string(),
            },
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not a zip file"), "{err}");
    assert_eq!(blob_log(&hub).len(), before);
}

/// Phase 3: Settings > Hub loads the status, Back up now follows the backup
/// until it ends, and a schedule change sends this computer's time zone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hub_page_loads_backs_up_and_changes_the_schedule() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    client.run(Command::LoadHubStatus).await.unwrap();
    let state = wait_for(&client, "the hub status", |s| {
        s.admin.hub.load == consort_client::LoadState::Loaded
    })
    .await;
    assert_eq!(state.admin.hub.address, "https://hub.example.com");
    assert_eq!(
        state.admin.hub.storage.summary,
        "1000 B used, 5 KB free on the hub disk"
    );
    assert_eq!(state.admin.hub.backup.keep_label, "The last 7 backups");

    client.run(Command::BackUpNow).await.unwrap();
    wait_for(&client, "a running backup", |s| s.admin.hub.backup.busy).await;
    let done = wait_for(&client, "the finished backup", |s| {
        matches!(
            s.admin.hub.backup.last,
            consort_client::admin::LastBackupView::Done { .. }
        )
    })
    .await;
    assert!(!done.admin.hub.backup.busy);
    let polls = hub
        .lock()
        .unwrap()
        .recorded
        .iter()
        .filter(|(k, _)| k == "hub.status")
        .count();
    assert!(polls >= 3, "the client asked {polls} times");

    client
        .run(Command::UpdateBackupSettings(
            consort_client::admin::BackupChanges {
                frequency: Some(consort_client::admin::BackupFrequency::Weekly),
                weekday: Some(consort_client::admin::Weekday::Friday),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    let sent = hub
        .lock()
        .unwrap()
        .recorded
        .iter()
        .rev()
        .find(|(k, _)| k == "hub.backup_settings")
        .map(|(_, body)| body.clone())
        .unwrap();
    assert_eq!(sent["schedule"]["frequency"], "weekly");
    assert_eq!(sent["schedule"]["weekday"], "friday");
    assert_eq!(sent["schedule"]["time"], "03:00");
    assert_eq!(
        sent["schedule"]["time_zone"],
        consort_client::admin::local_time_zone()
    );
    let state = wait_for(&client, "the weekly schedule", |s| {
        s.admin.hub.backup.frequency == consort_client::admin::BackupFrequency::Weekly
    })
    .await;
    assert_eq!(state.admin.hub.backup.weekday_label, "On Friday");

    // A folder that the hub refuses shows its reason.
    let err = client
        .run(Command::UpdateBackupSettings(
            consort_client::admin::BackupChanges {
                folder: Some("backups".into()),
                ..Default::default()
            },
        ))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("full path"), "{err}");
    let state = wait_for(&client, "the folder error", |s| {
        s.admin.hub.backup.error.is_some()
    })
    .await;
    assert!(
        state
            .admin
            .hub
            .backup
            .error
            .as_deref()
            .unwrap()
            .contains("full path")
    );
}

/// Phase 3: Settings > Clients lists the apps, revokes another app, and the
/// pairing dialog sees the app that paired with its code.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_clients_page_lists_revokes_and_sees_a_new_app() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let me = hub.lock().unwrap().client_id.unwrap();
    let other = consort_proto::model::Client {
        id: ClientId::new(),
        name: "bigboi".into(),
        kind: consort_proto::model::ClientKind::Desktop,
        public_key: consort_proto::auth::PublicKey::from_bytes([2; 32]),
        push_provider: None,
        paired_at: Timestamp::now(),
        last_seen_at: Some(Timestamp::now().plus_millis(-7_200_000)),
        revoked_at: None,
        os: Some(consort_proto::model::Os::Windows),
        paired_by: Some(consort_proto::model::PairedBy::Client { client_id: me }),
    };
    hub.lock().unwrap().other_client = Some(other.clone());
    client.run(Command::LoadClients).await.unwrap();
    let state = wait_for(&client, "the clients", |s| s.admin.clients.rows.len() == 2).await;
    let rows = &state.admin.clients.rows;
    assert!(rows[0].this_app && rows[0].online);
    assert_eq!(rows[1].name, "bigboi");
    assert!(
        rows[1]
            .detail
            .starts_with("Desktop app on Windows. Paired from MacBook Pro on ")
    );
    assert_eq!(rows[1].status, "Seen 2 hours ago");

    // The app that you use cannot go.
    let err = client
        .run(Command::RevokeClient {
            client_id: me.to_string(),
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("cannot revoke"), "{err}");
    client
        .run(Command::RevokeClient {
            client_id: other.id.to_string(),
        })
        .await
        .unwrap();
    wait_for(&client, "the row to go", |s| {
        s.admin.clients.rows.len() == 1
    })
    .await;

    // The pairing dialog waits, then names the new app from the feed.
    client.run(Command::ClientPairingCode).await.unwrap();
    let state = wait_for(&client, "a pairing code", |s| s.admin.pairing.is_some()).await;
    let pairing = state.admin.pairing.clone().unwrap();
    assert_eq!(pairing.code, "KQ7M-4XPD");
    assert!(pairing.qr_text.contains("https://hub.example.com"));
    assert!(pairing.paired.is_none());
    let new_app = consort_proto::model::Client {
        id: ClientId::new(),
        name: "ThinkPad".into(),
        paired_at: Timestamp::now().plus_millis(1_000),
        paired_by: Some(consort_proto::model::PairedBy::Client { client_id: me }),
        os: Some(consort_proto::model::Os::Linux),
        ..other
    };
    let frame = feed(&mut hub.lock().unwrap(), EntityPayload::Client(new_app));
    push(&hub, vec![frame]);
    let state = wait_for(&client, "the paired app", |s| {
        s.admin.pairing.as_ref().is_some_and(|p| p.paired.is_some())
    })
    .await;
    assert_eq!(
        state.admin.pairing.as_ref().unwrap().paired.as_deref(),
        Some("ThinkPad")
    );
    client.run(Command::CancelClientPairing).await.unwrap();
    wait_for(&client, "the dialog to close", |s| {
        s.admin.pairing.is_none()
    })
    .await;
}

// ---------------------------------------------------------------------------
// Phase 2: routines and memory.

fn tutti_channel_id(client: &Client) -> String {
    tutti_id(client)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn routines_save_run_turn_off_and_delete_with_their_runs_in_the_view() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let channel = tutti_channel_id(&client);

    let mut form =
        consort_client::routines::RoutineForm::new(&channel, consort_client::format::now_ms());
    form.name = "Sunset check".into();
    form.prompt = "Check the sunset time.".into();
    form.schedule.time = "17:30".into();
    form.schedule.time_zone = "Europe/Berlin".into();
    let saved = client
        .run_for(Command::SaveRoutine {
            routine_id: None,
            form,
        })
        .await
        .unwrap();
    let consort_client::CommandOutput::RoutineSaved(id) = saved else {
        panic!("no routine ID");
    };
    {
        let h = hub.lock().unwrap();
        assert_eq!(h.routines.len(), 1);
        assert_eq!(
            h.routines[0].schedule,
            consort_proto::model::RoutineSchedule::Cron {
                expression: "30 17 * * *".into(),
                time_zone: "Europe/Berlin".into(),
            }
        );
    }
    let state = wait_for(&client, "the routine", |s| s.routines.len() == 1).await;
    assert_eq!(state.routines[0].name, "Sunset check");

    // The runs load, then Run now brings a run that starts and ends.
    client
        .run(Command::LoadRoutineRuns {
            routine_id: id.clone(),
        })
        .await
        .unwrap();
    client
        .run(Command::RunRoutineNow {
            routine_id: id.clone(),
        })
        .await
        .unwrap();
    let state = wait_for(&client, "the run", |s| {
        s.routines.first().is_some_and(|r| {
            r.runs_loaded
                && r.runs.len() == 1
                && r.runs[0].status == consort_proto::model::RoutineRunStatus::Ok
        })
    })
    .await;
    assert!(state.routines[0].runs[0].manual);
    assert!(state.routines[0].runs[0].message_id.is_some());

    client
        .run(Command::SetRoutineEnabled {
            routine_id: id.clone(),
            enabled: false,
        })
        .await
        .unwrap();
    wait_for(&client, "the routine off", |s| {
        s.routines.first().is_some_and(|r| !r.enabled)
    })
    .await;
    client
        .run(Command::DeleteRoutine { routine_id: id })
        .await
        .unwrap();
    wait_for(&client, "no routine", |s| s.routines.is_empty()).await;
    assert!(hub.lock().unwrap().routines.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_loads_writes_with_the_stale_check_and_comes_back_from_recently_deleted() {
    let (url, _hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let global = consort_client::memory::GLOBAL.to_owned();

    client
        .run(Command::LoadMemory {
            scope: global.clone(),
            deleted: false,
        })
        .await
        .unwrap();
    let write = |content: &str, expected: Option<String>| Command::WriteMemoryNote {
        scope: global.clone(),
        path: "people/kevin".into(),
        content: content.into(),
        expected_sha256: expected,
    };
    let consort_client::CommandOutput::MemoryNote(first) =
        client.run_for(write("Kevin.", None)).await.unwrap()
    else {
        panic!("no note");
    };
    assert_eq!(first.path, "people/kevin.md");
    let state = wait_for(&client, "the note", |s| {
        s.memory_notes.iter().any(|n| n.path == "people/kevin.md")
    })
    .await;
    assert!(state.memory_loaded.contains(&global));

    // A second writer saved first: the old hash is stale, and the hub says so.
    client
        .run(write("Kevin likes tea.", Some(first.sha256.clone())))
        .await
        .unwrap();
    let stale = client
        .run(write("Kevin likes coffee.", Some(first.sha256.clone())))
        .await
        .unwrap_err();
    assert!(matches!(stale, ClientError::Hub { ref code, .. } if code == "stale_write"));

    // The history has both versions, newest first.
    let consort_client::CommandOutput::MemoryHistory(versions) = client
        .run_for(Command::LoadMemoryHistory {
            scope: global.clone(),
            path: "people/kevin.md".into(),
        })
        .await
        .unwrap()
    else {
        panic!("no history");
    };
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].content.as_deref(), Some("Kevin likes tea."));

    // Delete moves it to Recently deleted. Restore brings it back.
    let consort_client::CommandOutput::MemoryNote(current) = client
        .run_for(Command::ReadMemoryNote {
            scope: global.clone(),
            path: "people/kevin.md".into(),
        })
        .await
        .unwrap()
    else {
        panic!("no note");
    };
    client
        .run(Command::DeleteMemoryNote {
            scope: global.clone(),
            path: "people/kevin.md".into(),
            sha256: current.sha256.clone(),
        })
        .await
        .unwrap();
    wait_for(&client, "the deleted note", |s| {
        s.memory_notes
            .iter()
            .any(|n| n.path == "people/kevin.md" && n.deleted_at.is_some())
    })
    .await;
    client
        .run(Command::LoadMemory {
            scope: global.clone(),
            deleted: true,
        })
        .await
        .unwrap();
    client
        .run(Command::RestoreMemoryNote {
            scope: global.clone(),
            path: "people/kevin.md".into(),
        })
        .await
        .unwrap();
    wait_for(&client, "the restored note", |s| {
        s.memory_notes
            .iter()
            .any(|n| n.path == "people/kevin.md" && n.deleted_at.is_none())
    })
    .await;

    // A rename moves it.
    client
        .run(Command::MoveMemoryNote {
            scope: global.clone(),
            from_path: "people/kevin.md".into(),
            to_path: "family/kevin.md".into(),
            sha256: current.sha256,
        })
        .await
        .unwrap();
    let state = wait_for(&client, "the moved note", |s| {
        s.memory_notes.iter().any(|n| n.path == "family/kevin.md")
    })
    .await;
    assert!(
        !state
            .memory_notes
            .iter()
            .any(|n| n.path == "people/kevin.md")
    );
}

// ---------------------------------------------------------------------------
// The saved cache (phase 5): the app opens with its data while the hub is
// down, and a reconnect resumes from the saved feed position.

/// What the client saved in `dir`, or `None` while there is no whole save.
fn saved(dir: &std::path::Path) -> Option<consort_client::persist::Saved> {
    let store = Store::new(dir);
    let identity = store.load().ok()??;
    let owner = consort_client::persist::Owner::of(&identity);
    consort_client::persist::CacheFile::open(&store.cache_path(), &owner)
        .ok()?
        .1
}

/// Waits until the client saved its first whole load.
async fn until_saved(dir: &std::path::Path, channel: &str) -> consort_client::persist::Saved {
    until("the cache is saved", || {
        saved(dir).is_some_and(|s| s.cache.channels_loaded.contains(channel))
    })
    .await;
    saved(dir).unwrap()
}

fn add_message(hub: &Shared, body: &str) -> (Frame, ChannelId) {
    let mut h = hub.lock().unwrap();
    let channel_id = h.channel.as_ref().unwrap().id;
    let m = text_message(channel_id, body, Timestamp::now().as_millis(), None);
    h.messages.push(m.clone());
    (feed(&mut h, EntityPayload::Message(m)), channel_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_opens_with_saved_data_and_catches_up_from_the_saved_position() {
    let (url, hub) = start_hub().await;
    let (_, channel_id) = add_message(&hub, "Said before the restart");
    let dir = tempfile::tempdir().unwrap();
    let first = paired_client(&url, dir.path(), options(quick())).await;
    let tutti = tutti_id(&first);
    let saved_seq = until_saved(dir.path(), &tutti).await.cache.seq;
    drop(first);

    let client = Client::start_with(Some(Store::new(dir.path())), options(quick())).unwrap();
    // Before the first connect, the app has its data.
    let start = client.state();
    assert!(
        bodies_of(&start, &tutti)
            .iter()
            .any(|(b, _)| b == "Said before the restart"),
        "{:?}",
        bodies_of(&start, &tutti)
    );
    assert!(start.tutti().is_some());
    assert!(!start.tasks.is_empty(), "the tasks show too");

    wait_for(&client, "online", |s| s.connection == Connection::Online).await;
    // It caught up from its position. It did not load a snapshot.
    {
        let h = hub.lock().unwrap();
        assert_eq!(
            h.sessions[1],
            vec!["auth.response", "hello", "sync.subscribe"]
        );
        assert_eq!(h.subscribe_after[1], saved_seq);
    }
    // What the hub changed while the app was away arrives from the feed.
    let (frame, _) = add_message(&hub, "Said while the app was away");
    push(&hub, vec![frame]);
    wait_for(&client, "the missed message", |s| {
        bodies_of(s, &tutti)
            .iter()
            .any(|(b, _)| b == "Said while the app was away")
    })
    .await;
    let _ = channel_id;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_saved_position_that_the_hub_does_not_have_loads_a_new_snapshot() {
    let (url, hub) = start_hub().await;
    hub.lock().unwrap().resync_on_reconnect = true;
    let dir = tempfile::tempdir().unwrap();
    let first = paired_client(&url, dir.path(), options(quick())).await;
    let tutti = tutti_id(&first);
    until_saved(dir.path(), &tutti).await;
    drop(first);
    {
        let mut h = hub.lock().unwrap();
        if let Some(channel) = h.channel.as_mut() {
            channel.purpose = "Renamed while the app was away".into();
        }
    }

    let client = Client::start_with(Some(Store::new(dir.path())), options(quick())).unwrap();
    // The old data shows first.
    assert_ne!(
        client.state().tutti().unwrap().purpose,
        "Renamed while the app was away"
    );
    let state = wait_for(&client, "the new snapshot", |s| {
        s.connection == Connection::Online
            && s.tutti()
                .is_some_and(|c| c.purpose == "Renamed while the app was away")
    })
    .await;
    assert!(state.tutti().is_some());
    assert_eq!(
        hub.lock().unwrap().sessions[1],
        vec![
            "auth.response",
            "hello",
            "sync.subscribe",
            "sync.snapshot",
            "sync.snapshot",
            "proactivity.log",
            "sync.subscribe"
        ]
    );
    // The new data is saved, and it holds the new text.
    until("the new snapshot is saved", || {
        saved(dir.path()).is_some_and(|s| {
            s.cache
                .channels
                .get(&tutti)
                .is_some_and(|c| c.purpose == "Renamed while the app was away")
        })
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_damaged_cache_file_is_moved_aside_and_the_client_loads_a_snapshot() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let first = paired_client(&url, dir.path(), options(quick())).await;
    let tutti = tutti_id(&first);
    until_saved(dir.path(), &tutti).await;
    drop(first);
    let store = Store::new(dir.path());
    std::fs::write(store.cache_path(), "garbage ".repeat(2_000)).unwrap();

    let client = Client::start_with(Some(store.clone()), options(quick())).unwrap();
    // No saved data, and the app still starts.
    assert!(client.state().tutti().is_none());
    wait_for(&client, "a new snapshot", |s| {
        s.connection == Connection::Online && s.tutti().is_some()
    })
    .await;
    let sessions = hub.lock().unwrap().sessions.clone();
    assert!(
        sessions[1].iter().any(|k| k == "sync.snapshot"),
        "{:?}",
        sessions[1]
    );
    assert!(dir.path().join("cache.sqlite3.broken").exists());
    // The new file holds the new data.
    until_saved(dir.path(), &tutti).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unpairing_removes_the_saved_data_with_the_identity() {
    let (url, _hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let tutti = tutti_id(&client);
    let store = Store::new(dir.path());
    until_saved(dir.path(), &tutti).await;
    assert!(store.cache_path().exists());

    client.unpair().await.unwrap();
    assert!(!store.cache_path().exists(), "the saved data is gone");
    assert!(store.load().unwrap().is_none(), "the identity is gone");
    wait_for(&client, "not paired", |s| {
        s.connection == Connection::NotPaired && s.tutti().is_none()
    })
    .await;
    // The link ended, and a late save does not bring the file back.
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    assert!(!store.cache_path().exists());
    assert_eq!(client.state().connection, Connection::NotPaired);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_with_a_hub_again_removes_the_saved_data_of_the_old_hub() {
    let (url, _hub) = start_hub().await;
    let (other_url, _other) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    let tutti = tutti_id(&client);
    let old = until_saved(dir.path(), &tutti).await;
    assert!(old.cache.channels.contains_key(&tutti));

    client.pair(other_url, CODE.into()).await.unwrap();
    // Whatever the new hub sends, the file never holds the old hub's channel
    // for the new identity.
    wait_for(&client, "online at the new hub", |s| {
        s.connection == Connection::Online && s.tutti().is_some()
    })
    .await;
    let new_tutti = tutti_id(&client);
    let new = until_saved(dir.path(), &new_tutti).await;
    assert_eq!(new.cache.channels.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_app_opens_offline_with_saved_data_and_sends_the_queue_when_the_hub_is_back() {
    let (url, hub) = start_hub().await;
    add_message(&hub, "Said before the hub went down");
    let dir = tempfile::tempdir().unwrap();
    let first = paired_client(&url, dir.path(), options(quick())).await;
    let tutti = tutti_id(&first);
    let saved_at = until_saved(dir.path(), &tutti).await;
    assert!(saved_at.fresh_at > 0);
    drop(first);
    hub.lock().unwrap().down = true;

    let client = Client::start_with(Some(Store::new(dir.path())), options(quick())).unwrap();
    let state = wait_for(&client, "the offline state", |s| {
        matches!(s.connection, Connection::Offline { .. }) && s.saved_data.is_some()
    })
    .await;
    // The data shows, with the time it is from.
    assert!(
        bodies_of(&state, &tutti)
            .iter()
            .any(|(b, _)| b == "Said before the hub went down")
    );
    let from = state.saved_data.clone().unwrap();
    assert!(from.from_ms >= saved_at.fresh_at, "{from:?}");
    assert!(
        from.pill_text()
            .starts_with("Cannot reach the hub. You see saved data from "),
        "{}",
        from.pill_text()
    );
    // No worker reads as online, and approvals wait for the hub.
    assert!(state.workers.iter().all(|w| !w.online));
    assert_eq!(state.sidebar.workers_online, 0);
    assert!(state.answers_blocked().is_some());
    assert!(
        client
            .answer_approval("apr_none".into(), ApprovalChoice::AllowOnce)
            .await
            .is_err(),
        "an answer needs the hub"
    );

    // A message waits in the outbox, and the view says that it is pending.
    client
        .send_message(tutti.clone(), "Sent while the hub was down".into())
        .await
        .unwrap();
    let state = wait_for(&client, "the queued message", |s| {
        bodies_of(s, &tutti)
            .iter()
            .any(|(b, pending)| b == "Sent while the hub was down" && *pending)
    })
    .await;
    assert!(state.saved_data.is_some());
    assert!(dir.path().join("outbox.json").exists());
    assert!(hub.lock().unwrap().sent_bodies.is_empty());

    // The hub comes back. The queue goes out, and the screen is current.
    hub.lock().unwrap().down = false;
    client.reconnect().await.unwrap();
    let state = wait_for(&client, "the sent message", |s| {
        s.connection == Connection::Online
            && bodies_of(s, &tutti)
                .iter()
                .any(|(b, pending)| b == "Sent while the hub was down" && !*pending)
    })
    .await;
    assert!(state.saved_data.is_none());
    assert!(state.answers_blocked().is_none());
    assert_eq!(
        hub.lock().unwrap().sent_bodies,
        vec!["Sent while the hub was down"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hub_that_goes_down_while_the_app_runs_shows_the_time_of_the_last_update() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), options(quick())).await;
    assert!(client.state().saved_data.is_none(), "online: no pill");
    let before = Timestamp::now().as_millis();
    {
        let mut h = hub.lock().unwrap();
        h.down = true;
    }
    let kick = hub.lock().unwrap().kick.clone();
    kick.notify_one();
    let state = wait_for(&client, "the offline state", |s| {
        matches!(s.connection, Connection::Offline { .. }) && s.saved_data.is_some()
    })
    .await;
    let after = Timestamp::now().as_millis();
    let from = state.saved_data.clone().unwrap().from_ms;
    // The data was current until the link ended: a moment ago, not at
    // the start of the run.
    assert!(
        from >= before - 1_000 && from <= after,
        "{before} {from} {after}"
    );
    assert!(state.answers_blocked().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hub_that_refuses_the_app_takes_the_saved_data_with_it() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let first = paired_client(&url, dir.path(), options(quick())).await;
    let tutti = tutti_id(&first);
    until_saved(dir.path(), &tutti).await;
    drop(first);
    // The hub does not know this app any more.
    hub.lock().unwrap().key = None;

    let client = Client::start_with(Some(Store::new(dir.path())), options(quick())).unwrap();
    wait_for(&client, "the refusal", |s| {
        matches!(s.connection, Connection::Refused { .. })
    })
    .await;
    assert!(!Store::new(dir.path()).cache_path().exists());
}

// ---------------------------------------------------------------------------
// Review fixes: a new owner, a skipped change, a restored hub.

/// The IDs of the channels in the saved file.
fn saved_channels(dir: &std::path::Path) -> Vec<String> {
    saved(dir)
        .map(|s| s.cache.channels.keys().cloned().collect())
        .unwrap_or_default()
}

/// Review finding 8: a link to the old hub that was still in its first
/// sync when the user paired with a new hub saved the old hub's data under
/// the new owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn data_of_the_old_hub_never_lands_under_a_new_pairing() {
    let (url_a, hub_a) = start_hub().await;
    add_message(&hub_a, "From hub A");
    let gate = Arc::new(tokio::sync::Notify::new());
    let old_channel = {
        let mut h = hub_a.lock().unwrap();
        h.hold_channel_snapshots = Some(gate.clone());
        h.channel.as_ref().unwrap().id.to_string()
    };
    let dir = tempfile::tempdir().unwrap();
    let client =
        Client::start_with(Some(Store::new(dir.path())), ClientOptions::desktop()).unwrap();
    client.pair(url_a, CODE.into()).await.unwrap();
    // The link to A waits for its channel snapshot.
    until("the first sync of hub A", || {
        hub_a
            .lock()
            .unwrap()
            .sessions
            .first()
            .is_some_and(|s| s.iter().filter(|k| *k == "sync.snapshot").count() >= 2)
    })
    .await;

    let (url_b, hub_b) = start_hub().await;
    let new_channel = hub_b
        .lock()
        .unwrap()
        .channel
        .as_ref()
        .unwrap()
        .id
        .to_string();
    client.pair(url_b, CODE.into()).await.unwrap();
    // Now hub A answers the old link.
    gate.notify_waiters();
    wait_for(&client, "online with hub B", |s| {
        s.connection == Connection::Online && s.tutti().is_some_and(|t| t.id == new_channel)
    })
    .await;
    until("the save of hub B", || {
        saved_channels(dir.path()).contains(&new_channel)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    let state = client.state();
    assert!(
        bodies_of(&state, &old_channel).is_empty(),
        "no message of hub A shows"
    );
    assert!(
        !saved_channels(dir.path()).contains(&old_channel),
        "the file holds no channel of hub A"
    );
    let saved = saved(dir.path()).unwrap();
    assert!(
        saved
            .cache
            .messages
            .values()
            .all(|m| m.body != "From hub A"),
        "the file holds no message of hub A"
    );
}

/// Review finding 9: a skipped change moved the saved feed position before
/// its data arrived, so a quit lost the change for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_saved_position_waits_for_the_data_of_a_skipped_change() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let tutti = tutti_id(&client);
    let before = until_saved(dir.path(), &tutti).await.cache.seq;
    let gate = Arc::new(tokio::sync::Notify::new());
    let (skipped_seq, frames) = {
        let mut h = hub.lock().unwrap();
        h.hold_channel_snapshots = Some(gate.clone());
        let channel_id = h.channel.as_ref().unwrap().id;
        let m = text_message(
            channel_id,
            "the skipped change",
            Timestamp::now().as_millis(),
            None,
        );
        h.messages.push(m);
        h.seq += 1;
        let skipped = Frame::event(&consort_proto::client_api::FeedSkipped {
            seq: h.seq,
            channel_id: Some(channel_id),
            entity: consort_proto::client_api::EntityKind::Message,
            entity_id: "msg".into(),
        })
        .unwrap();
        let skipped_seq = h.seq;
        // A later change that asks for a save.
        let later = text_message(
            channel_id,
            "a later change",
            Timestamp::now().as_millis(),
            None,
        );
        h.messages.push(later.clone());
        let next = feed(&mut h, EntityPayload::Message(later));
        (skipped_seq, vec![skipped, next])
    };
    push(&hub, frames);
    wait_for(&client, "the later change", |s| {
        bodies_of(s, &tutti)
            .iter()
            .any(|(b, _)| b == "a later change")
    })
    .await;
    // Several save delays pass while the data of the skipped change waits.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let held = saved(dir.path()).unwrap().cache.seq;
    assert!(
        held < skipped_seq,
        "the file keeps a position before the gap: {held} (before {before}, gap {skipped_seq})"
    );
    gate.notify_waiters();
    until("the save after the data came", || {
        saved(dir.path()).is_some_and(|s| {
            s.cache.seq > skipped_seq
                && s.cache
                    .messages
                    .values()
                    .any(|m| m.body == "the skipped change")
        })
    })
    .await;
}

/// Review finding 15: a hub that was restored from a backup keeps its URL,
/// and its feed numbers name other changes. A new epoch makes the client
/// drop its saved position and load a snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hub_with_a_new_epoch_gets_a_snapshot_not_a_resume() {
    let (url, hub) = start_hub().await;
    hub.lock().unwrap().hub_epoch = Some("before-restore".into());
    let dir = tempfile::tempdir().unwrap();
    let first = paired_client(&url, dir.path(), options(quick())).await;
    let tutti = tutti_id(&first);
    let saved_first = until_saved(dir.path(), &tutti).await;
    assert_eq!(
        saved_first.cache.hub_epoch.as_deref(),
        Some("before-restore")
    );
    drop(first);

    hub.lock().unwrap().hub_epoch = Some("after-restore".into());
    let client = Client::start_with(Some(Store::new(dir.path())), options(quick())).unwrap();
    wait_for(&client, "online", |s| s.connection == Connection::Online).await;
    let session = hub.lock().unwrap().sessions[1].clone();
    assert_eq!(
        &session[..3],
        ["auth.response", "hello", "sync.snapshot"],
        "no resume from the old position: {session:?}"
    );
    until("the new epoch is saved", || {
        saved(dir.path()).is_some_and(|s| s.cache.hub_epoch.as_deref() == Some("after-restore"))
    })
    .await;
}

#[tokio::test]
async fn the_voice_page_and_the_draft_card_use_their_requests() {
    use consort_client::voice::{VoiceChanges, VoiceSampleState};
    use consort_proto::model::{
        CommitmentFlag, ConnectorCard, DisplayField, FlagKind, OutgoingKind, ToolKind, VoiceUse,
    };
    let (url, hub) = start_hub().await;
    {
        let mut h = hub.lock().unwrap();
        h.record_answers = true;
        let approval = h.approval.as_mut().unwrap();
        let body = "Friday at 3 PM works for me.";
        let details = ApprovalDetails::Connector(Box::new(ConnectorCard {
            connection_id: consort_proto::ids::ConnectionId::new(),
            connection_name: "Gmail Work".into(),
            service_name: "Gmail".into(),
            tool: "GMAIL_SEND_EMAIL".into(),
            tool_kind: ToolKind::Change,
            fields: vec![DisplayField {
                label: "Body".into(),
                value: body.into(),
            }],
            arguments: serde_json::json!({"to": "dana@acme.dev", "body": body}),
            outgoing: Some(OutgoingKind::Send),
            editable: vec!["body".into()],
            voice: Some(VoiceUse {
                context: "email.formal".into(),
            }),
            flags: vec![CommitmentFlag {
                field: "body".into(),
                label: "Body".into(),
                text: "Friday at 3 PM".into(),
                kind: FlagKind::Date,
                start: 0,
                end: 14,
                rule: None,
            }],
            draft_arguments: None,
            not_my_voice: false,
            ask_rule: None,
            replaced_by: None,
        }));
        approval.options = details.hub_card_options().unwrap();
        approval.details = details;
        approval.request_id = consort_proto::model::hub_card_request_id(approval.id);
    }
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let approval_id = hub
        .lock()
        .unwrap()
        .approval
        .as_ref()
        .unwrap()
        .id
        .to_string();

    // The draft card reads with the voice line and the flag.
    let state = wait_for(&client, "the draft card", |s| {
        s.timelines
            .iter()
            .flat_map(|t| t.items.iter())
            .any(|i| matches!(i, TimelineItem::TaskCard(c) if c.approval.is_some()))
    })
    .await;
    let card = state
        .timelines
        .iter()
        .flat_map(|t| t.items.iter())
        .find_map(|i| match i {
            TimelineItem::TaskCard(c) => c.approval.clone(),
            _ => None,
        })
        .unwrap();
    let Some(consort_client::HubCardView::Connector(connector)) = card.card.as_deref() else {
        panic!("not a connector card");
    };
    let draft = connector.draft.clone().unwrap();
    assert_eq!(draft.voice_line, "Written in your voice (Formal email)");
    assert_eq!(draft.flags.len(), 1);
    assert!(draft.allow_blocked(false));

    // Settings > Voice: load, turn on, try it.
    client.run(Command::LoadVoice).await.unwrap();
    client
        .run(Command::SetVoice(VoiceChanges {
            enabled: Some(true),
            sources: Some(Vec::new()),
            ..VoiceChanges::default()
        }))
        .await
        .unwrap();
    client
        .run(Command::TryVoice {
            request: "Reply to Dana, say yes".into(),
        })
        .await
        .unwrap();
    let state = wait_for(&client, "the sample", |s| {
        s.voice
            .sample
            .as_ref()
            .is_some_and(|x| x.state == VoiceSampleState::Ready)
    })
    .await;
    assert!(state.voice.enabled);
    assert_eq!(
        state.voice.sample.as_ref().unwrap().voice_line,
        "Written in your voice (Formal email)"
    );

    // Edit, then allow with the tick.
    client
        .run(Command::EditApproval {
            approval_id: approval_id.clone(),
            fields: vec![("body".into(), "I will check my calendar.".into())],
        })
        .await
        .unwrap();
    client
        .run(Command::AnswerApproval {
            approval_id: approval_id.clone(),
            choice: ApprovalChoice::AllowOnce,
            checked: Some("hash-the-user-saw".into()),
        })
        .await
        .unwrap();
    client
        .run(Command::VoiceFeedback {
            approval_id: approval_id.clone(),
        })
        .await
        .unwrap();
    let edit = recorded_of(&hub, "approval.edit");
    assert_eq!(edit[0]["fields"]["body"], "I will check my calendar.");
    let answer = recorded_of(&hub, "approval.answer");
    assert_eq!(answer[0]["checked"], true);
    assert_eq!(answer[0]["checked_hash"], "hash-the-user-saw");
    let set = recorded_of(&hub, "voice.set");
    assert_eq!(set[0]["enabled"], true);
    assert_eq!(recorded_of(&hub, "voice.get").len(), 1);
    assert_eq!(
        recorded_of(&hub, "voice.try")[0]["request"],
        "Reply to Dana, say yes"
    );
    assert_eq!(recorded_of(&hub, "voice.feedback").len(), 1);
}

/// Try again on an error card: a task retries, and a channel sends the last
/// message of the user again as a new message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn try_again_retries_a_task_and_resends_a_channel_message() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let task_id = client.state().tasks[0].id.clone();
    client
        .try_again(consort_client::TryAgainTarget::Task {
            task_id: task_id.clone(),
        })
        .await
        .unwrap();
    let retried = recorded_of(&hub, "task.retry");
    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0]["task_id"], task_id.as_str());

    let tutti = tutti_id(&client);
    client
        .try_again(consort_client::TryAgainTarget::ResendMessage {
            channel_id: tutti,
            body: "Summarize my week".into(),
        })
        .await
        .unwrap();
    assert_eq!(hub.lock().unwrap().sent_bodies, ["Summarize my week"]);
}

/// A started task takes a new model with `task.update`: the selection with
/// its effort, and nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_task_model_sends_task_update_with_the_selection_and_the_effort() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let task_id = client.state().tasks[0].id.clone();
    client
        .set_task_model(
            task_id.clone(),
            consort_client::SelectionView {
                account_label: "Claude Personal".into(),
                model_id: "opus".into(),
                model_name: "Opus 5.5".into(),
                options: vec![(
                    "effort".into(),
                    consort_client::SelectionOption::Text("high".into()),
                )],
            },
        )
        .await
        .unwrap();
    let sent = recorded_of(&hub, "task.update");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["task_id"], task_id.as_str());
    assert_eq!(
        sent[0]["model_selection"]["account_label"],
        "Claude Personal"
    );
    assert_eq!(sent[0]["model_selection"]["model"], "opus");
    assert_eq!(sent[0]["model_selection"]["options"]["effort"], "high");
    assert!(sent[0].get("title").is_none() && sent[0].get("brief").is_none());
}

/// The user edits a card from `show_draft`: the client sends `draft.edit`
/// and shows the answer of the hub, with the text that the assistant wrote.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edit_draft_sends_draft_edit_and_shows_the_saved_text() {
    let (url, hub) = start_hub().await;
    let dir = tempfile::tempdir().unwrap();
    let client = paired_client(&url, dir.path(), ClientOptions::desktop()).await;
    let message_id = MessageId::new().to_string();
    // The snapshot of the channel replaces the cache, so it must be in
    // before the answer of the hub is merged.
    let tutti = tutti_id(&client);
    wait_for(&client, "the timeline", |s| {
        s.timeline(&tutti).is_some_and(|t| t.loaded)
    })
    .await;
    client
        .edit_draft(message_id.clone(), None, Some("Dear Sir,".into()))
        .await
        .unwrap();
    let sent = recorded_of(&hub, "draft.edit");
    assert_eq!(sent[0]["message_id"], message_id.as_str());
    assert_eq!(sent[0]["body"], "Dear Sir,");
    assert!(sent[0].get("title").is_none());
    let state = wait_for(&client, "the edited draft", |s| {
        s.timelines
            .iter()
            .flat_map(|t| t.items.iter())
            .any(|i| matches!(i, TimelineItem::Notice(n) if n.draft.is_some()))
    })
    .await;
    let draft = state
        .timelines
        .iter()
        .flat_map(|t| t.items.iter())
        .find_map(|i| match i {
            TimelineItem::Notice(n) => n.draft.clone(),
            _ => None,
        })
        .unwrap();
    assert_eq!(draft.body, "Dear Sir,");
    assert!(draft.edited);
    assert_eq!(draft.original_body.as_deref(), Some("Dear Dr. Meyer,"));
    assert_eq!(draft.message_id, message_id);
}
