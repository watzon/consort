//! What a client asks for. Each public method of [`crate::Client`] sends one
//! of these to the backend on the client's own runtime.

use std::fmt;

pub use consort_proto::model::SecretEnv;

use crate::view::{ApprovalChoice, Isolation, SelectionView, SignInChoice};

/// A secret text, such as a sign-in code. `Debug` never shows it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretText(pub String);

impl fmt::Debug for SecretText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// The ACP agent of a new account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcpSpec {
    /// An agent from the ACP registry, by its ID.
    Registry { id: String },
    /// A program that the worker runs as it is.
    Command { program: String, args: Vec<String> },
}

/// The fields of a new account, from the Add account dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewAccount {
    pub worker_id: String,
    /// The harness: `claude`, `codex`, `opencode`, `gemini` or `acp`.
    pub harness: String,
    pub label: String,
    pub acp: Option<AcpSpec>,
    /// Secret variables, such as an API key.
    pub env: SecretEnv,
}

/// What a command gives back, for the few commands that make something.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CommandOutput {
    #[default]
    Done,
    /// `account.create` made an account with this ID.
    AccountCreated(String),
    /// `sign_in.start` started a sign-in with this ID.
    SignInStarted(String),
}

/// The fields of a new channel, from the New channel dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewChannel {
    pub name: String,
    pub purpose: String,
    pub group_id: Option<String>,
    pub repos: Vec<String>,
    pub preferred_workers: Vec<String>,
    /// The model of the channel's assistant. `None` uses Everything's.
    pub coordinator_selection: Option<SelectionView>,
    /// The model that tasks start with. `None` uses Everything's.
    pub task_selection: Option<SelectionView>,
    pub isolation: Isolation,
    /// The worker tags that the channel requires.
    pub tags: Vec<String>,
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
    /// The worker tags that the channel requires. `Some(vec![])` removes
    /// them all.
    pub tags: Option<Vec<String>>,
    /// The model of the channel's assistant. `Some(None)` takes the
    /// channel's own model away, so it uses Everything's.
    pub coordinator_selection: Option<Option<SelectionView>>,
    /// The model that tasks start with. `Some(None)` takes it away.
    pub task_selection: Option<Option<SelectionView>>,
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
    SetWorkerTags {
        worker_id: String,
        tags: Vec<String>,
    },
    SetWorkerPaused {
        worker_id: String,
        paused: bool,
    },
    RemoveWorker {
        worker_id: String,
    },
    /// Makes an account on a worker. The output is
    /// [`CommandOutput::AccountCreated`].
    CreateAccount(NewAccount),
    /// Renames an account, or sets or removes its secret variables.
    UpdateAccount {
        worker_id: String,
        account_id: String,
        label: Option<String>,
        env_set: SecretEnv,
        env_remove: Vec<String>,
    },
    /// Removes an account and its profile folder.
    RemoveAccount {
        worker_id: String,
        account_id: String,
    },
    SignOutAccount {
        worker_id: String,
        account_id: String,
    },
    /// Starts a sign-in. The output is [`CommandOutput::SignInStarted`].
    StartSignIn {
        worker_id: String,
        account_id: String,
        method: SignInChoice,
    },
    /// Sends the code that the sign-in page showed.
    SubmitSignInCode {
        sign_in_id: String,
        code: SecretText,
    },
    CancelSignIn {
        sign_in_id: String,
    },
    /// Forgets a sign-in that has ended, so it leaves the view state.
    DismissSignIn {
        sign_in_id: String,
    },
    /// Asks a worker to check for tool updates now.
    CheckTools {
        worker_id: String,
    },
    /// Asks a worker to install or update one tool.
    InstallTool {
        worker_id: String,
        tool: String,
    },
    /// Loads the ACP registry, and the icons of its agents. It does nothing
    /// while a load runs or after a load that worked.
    LoadAcpRegistry,
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
