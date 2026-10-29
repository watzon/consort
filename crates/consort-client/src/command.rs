//! What a client asks for. Each public method of [`crate::Client`] sends one
//! of these to the backend on the client's own runtime.

use crate::view::{ApprovalChoice, Isolation};

/// The fields of a new channel, from the New channel dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewChannel {
    pub name: String,
    pub purpose: String,
    pub group_id: Option<String>,
    pub repos: Vec<String>,
    pub preferred_workers: Vec<String>,
    pub task_selection: Option<String>,
    pub isolation: Isolation,
}

/// Changes to a channel. A `None` field stays as it is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelChanges {
    pub name: Option<String>,
    pub purpose: Option<String>,
    pub instructions: Option<String>,
    pub icon: Option<String>,
    pub group_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Pairs this client with a hub, with the setup code or a pairing code.
    Pair {
        hub_url: String,
        code: String,
    },
    SendMessage {
        channel_id: String,
        body: String,
        client_message_id: String,
    },
    DeleteMessage {
        message_id: String,
    },
    EditMessage {
        message_id: String,
        body: String,
    },
    ReplyToTask {
        task_id: String,
        body: String,
        client_message_id: String,
    },
    CreateChannel(NewChannel),
    UpdateChannel {
        channel_id: String,
        changes: ChannelChanges,
    },
    ArchiveChannel {
        channel_id: String,
    },
    DeleteChannel {
        channel_id: String,
    },
    SetGroupCollapsed {
        group_id: String,
        collapsed: bool,
    },
    MarkRead {
        channel_id: String,
    },
    /// Loads the replies of a task's thread, once.
    LoadThread {
        task_id: String,
    },
    /// Loads the next page of older messages in a channel's timeline.
    LoadOlder {
        channel_id: String,
    },
    ResolveTask {
        task_id: String,
    },
    ReopenTask {
        task_id: String,
    },
    RetryTask {
        task_id: String,
    },
    StopTask {
        task_id: String,
    },
    AnswerApproval {
        approval_id: String,
        choice: ApprovalChoice,
    },
    AnswerDecision {
        decision_id: String,
        option: Option<u32>,
        text: Option<String>,
    },
    /// Removes a message that the hub refused from the list of failed sends.
    DismissSendFailure {
        id: String,
    },
    WorkerPairingCode,
    CancelWorkerPairing,
    RenameWorker {
        worker_id: String,
        name: String,
    },
    SetWorkerLabels {
        worker_id: String,
        labels: Vec<String>,
    },
    SetWorkerPaused {
        worker_id: String,
        paused: bool,
    },
    RemoveWorker {
        worker_id: String,
    },
}
