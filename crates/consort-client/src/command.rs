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
    /// The name of a group to make for this channel. When it is set, the
    /// client makes the group first and puts the channel in it, and
    /// `group_id` is not used.
    pub new_group: Option<String>,
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
    /// Brings an archived channel back.
    UnarchiveChannel {
        channel_id: String,
    },
    /// Puts a channel in a group, or takes it out of its group with `None`.
    MoveChannel {
        channel_id: String,
        group_id: Option<String>,
    },
    /// Makes a group. With a channel, puts that channel in the new group.
    CreateGroup {
        name: String,
        channel_id: Option<String>,
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
    /// Hides a task in any status. It leaves the lists and the Tasks panel.
    ArchiveTask {
        task_id: String,
    },
    /// Connects to the hub now, without waiting for the retry timer. It
    /// does nothing while the link is up.
    Reconnect,
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

/// Refuses a channel ID that cannot name a channel. The ID is empty until the
/// first snapshot arrives, and a message for "no channel" would wait in the
/// outbox and fail later, so the composer must not take it.
pub fn check_channel_id(channel_id: &str) -> Result<(), crate::ClientError> {
    if channel_id.trim().is_empty() {
        return Err(crate::ClientError::NoChannel);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_channel_id_is_refused() {
        assert_eq!(check_channel_id(""), Err(crate::ClientError::NoChannel));
        assert_eq!(check_channel_id("  "), Err(crate::ClientError::NoChannel));
        assert_eq!(check_channel_id("chn_1"), Ok(()));
    }
}
