//! What a client asks for. Each public method of [`crate::Client`] sends one
//! of these to the backend on the client's own runtime.

use std::fmt;

pub use consort_proto::model::SecretEnv;

use crate::connectors::{AuthKind, ConnectionCallsView, ToolApprovalsView, UseInView};
use crate::skills::{SkillFileContentView, SkillFileView, SkillInstallInput, SkillPreviewView};
use crate::view::{ApprovalChoice, Isolation, SelectionView, SignInChoice};

/// A secret text, such as a sign-in code. `Debug` never shows it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretText(pub String);

impl fmt::Debug for SecretText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// The bytes of a pasted file. `Debug` shows only the size.
#[derive(Clone, PartialEq, Eq)]
pub struct PastedBytes(pub std::sync::Arc<Vec<u8>>);

impl fmt::Debug for PastedBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} bytes>", self.0.len())
    }
}

/// What the user attaches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachInput {
    /// A file on this computer, from the file dialog or a drop.
    Path(String),
    /// An image from the clipboard. The client keeps a copy in its cache
    /// folder, then uploads it like a file.
    Pasted {
        name: String,
        mime: String,
        bytes: PastedBytes,
    },
    /// A file that is on the hub already, such as a channel file.
    Blob(consort_proto::model::BlobRef),
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
    /// `Command::Attach` added uploads with these IDs, one for each file.
    Attached(Vec<String>),
    /// A blob is in the local cache at this path.
    LocalFile(String),
    /// `connection.connect` or `connection.reconnect` started a connect
    /// attempt. The connection's status follows it in the view state.
    ConnectStarted {
        connection_id: String,
        /// The sign-in page to open in the system browser. `None` for a
        /// scheme with no browser step.
        redirect_url: Option<String>,
    },
    /// What a skill install would bring.
    SkillPreview(SkillPreviewView),
    /// `skill.install` installed a skill with this ID.
    SkillInstalled(String),
    /// The files of a skill.
    SkillFiles(Vec<SkillFileView>),
    /// The text of one file of a skill.
    SkillFile(SkillFileContentView),
    /// A page of the audit log of a connection.
    ConnectionCalls(ConnectionCallsView),
    /// `routine.create` or `routine.update` saved the routine with this ID.
    RoutineSaved(String),
    /// One memory note, with its text.
    MemoryNote(crate::memory::MemoryNoteView),
    /// The versions of a memory note, newest first, with their text.
    MemoryHistory(Vec<crate::memory::MemoryNoteView>),
}

/// The fields of a new connection, from the Connect dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewConnection {
    /// The Composio toolkit slug.
    pub service: String,
    pub name: String,
    pub use_in: UseInView,
    pub approvals: ToolApprovalsView,
    /// The scheme to sign in with. `None` lets the hub choose.
    pub auth_scheme: Option<AuthKind>,
    /// The values of the credential form, for a scheme with no browser
    /// step. They go to the hub and on to Composio. `Debug` never shows them.
    pub credentials: SecretEnv,
    /// The connect card that this connect answers.
    pub approval_id: Option<String>,
}

/// Changes to a connection. A `None` field stays as it is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConnectionChanges {
    pub name: Option<String>,
    pub use_in: Option<UseInView>,
    pub approvals: Option<ToolApprovalsView>,
}

/// Changes to a skill. A `None` field stays as it is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkillChanges {
    pub enabled: Option<bool>,
    pub use_in: Option<UseInView>,
    /// Move the pin to the new version that waits for review.
    pub accept_new_version: bool,
}

/// The fields of a new channel, from the New channel dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewChannel {
    pub name: String,
    pub purpose: String,
    pub group_id: Option<String>,
    pub repos: Vec<String>,
    pub preferred_workers: Vec<String>,
    /// The model of the channel's assistant. `None` uses Tutti's.
    pub coordinator_selection: Option<SelectionView>,
    /// The model that tasks start with. `None` uses Tutti's.
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
    /// channel's own model away, so it uses Tutti's.
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
    /// Sends a message. It waits until the uploads in `attachments` are on
    /// the hub, then sends their blobs with the text and takes the uploads
    /// away. A failed upload stops the send with an error.
    SendMessage {
        channel_id: String,
        body: String,
        client_message_id: String,
        /// Upload IDs, from [`CommandOutput::Attached`].
        attachments: Vec<String>,
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
        /// Upload IDs, as for `SendMessage`.
        attachments: Vec<String>,
    },
    // Files.
    /// Attaches files to a composer, a reply box or a folder of channel
    /// files. Each file becomes an upload that starts at once. The output
    /// is [`CommandOutput::Attached`]. A file that is too large or cannot be
    /// read still gets an upload, in the failed state, so the chip says
    /// what happened.
    Attach {
        target: crate::uploads::UploadTarget,
        inputs: Vec<AttachInput>,
    },
    /// Starts a failed upload again.
    RetryUpload {
        id: String,
    },
    /// Takes an upload away.
    RemoveUpload {
        id: String,
    },
    /// Loads the files of a channel (`files.list`).
    LoadChannelFiles {
        channel_id: String,
    },
    /// Renames or moves a channel file (`files.move`).
    MoveChannelFile {
        channel_id: String,
        from_path: String,
        to_path: String,
    },
    DeleteChannelFile {
        channel_id: String,
        path: String,
    },
    /// Puts a blob that is on the hub in the channel files (`files.put`),
    /// such as an attachment of a message.
    SaveToChannelFiles {
        channel_id: String,
        path: String,
        blob: consort_proto::model::BlobRef,
    },
    /// Downloads a blob into the local cache. With a name, the output is a
    /// copy with that file name, so another app can open it. The output is
    /// [`CommandOutput::LocalFile`].
    FetchBlob {
        sha256: String,
        size: u64,
        name: Option<String>,
    },
    /// Downloads a blob and writes a copy to `to`.
    SaveBlob {
        sha256: String,
        size: u64,
        to: String,
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
    /// Stars a model in the model picker, or takes the star away. The hub
    /// keeps the favorites for every client.
    SetFavoriteModel {
        account_label: String,
        model_id: String,
        favorite: bool,
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
    /// Turns the provider extras of an account on or off.
    SetAccountExtras {
        worker_id: String,
        account_id: String,
        enabled: bool,
    },
    // Connectors.
    /// Asks the hub for the state of the Composio key and the plan.
    LoadComposioStatus,
    /// Saves the Composio project key on the hub. The key never comes back.
    SetComposioKey {
        api_key: SecretText,
    },
    RemoveComposioKey,
    /// Loads the categories of the connector catalog.
    LoadConnectorCategories,
    /// Searches the catalog. The first page replaces the old results.
    SearchConnectorCatalog {
        search: String,
        category: Option<String>,
    },
    /// Loads the next page of the last catalog search.
    LoadMoreConnectorCatalog,
    /// Loads the tools of a service, once.
    LoadServiceTools {
        service: String,
    },
    /// Starts a connection. The output is [`CommandOutput::ConnectStarted`].
    Connect(NewConnection),
    /// Starts a new sign-in for a connection. The output is
    /// [`CommandOutput::ConnectStarted`].
    ReconnectConnection {
        connection_id: String,
        credentials: SecretEnv,
    },
    UpdateConnection {
        connection_id: String,
        changes: ConnectionChanges,
    },
    DisconnectConnection {
        connection_id: String,
    },
    /// Loads a page of the audit log of a connection. The output is
    /// [`CommandOutput::ConnectionCalls`].
    LoadConnectionCalls {
        connection_id: String,
        before: Option<String>,
    },
    /// Turns a connection off or on in one channel.
    SetChannelConnectionEnabled {
        channel_id: String,
        connection_id: String,
        enabled: bool,
    },
    // Skills.
    /// Turns a skill off or on in one channel.
    SetChannelSkillEnabled {
        channel_id: String,
        skill_id: String,
        enabled: bool,
    },
    /// Searches the skill directories. The first page replaces the old
    /// results.
    SearchSkillDirectory {
        search: String,
        source: Option<String>,
    },
    LoadMoreSkillDirectory,
    LoadSkillDirectorySources,
    AddSkillDirectorySource {
        url: String,
    },
    RemoveSkillDirectorySource {
        id: String,
    },
    /// The output is [`CommandOutput::SkillPreview`].
    PreviewSkill {
        source: SkillInstallInput,
    },
    /// The output is [`CommandOutput::SkillInstalled`].
    InstallSkill {
        source: SkillInstallInput,
        use_in: UseInView,
        enabled: Option<bool>,
        /// The install card that this install answers.
        approval_id: Option<String>,
    },
    UpdateSkill {
        skill_id: String,
        changes: SkillChanges,
    },
    RemoveSkill {
        skill_id: String,
    },
    // Routines.
    /// Creates a routine (no `routine_id`) or saves the changes of one. The
    /// output is [`CommandOutput::RoutineSaved`].
    SaveRoutine {
        routine_id: Option<String>,
        form: crate::routines::RoutineForm,
    },
    /// Turns a routine on or off.
    SetRoutineEnabled {
        routine_id: String,
        enabled: bool,
    },
    DeleteRoutine {
        routine_id: String,
    },
    /// Runs a routine now. The hub refuses it while the last run still runs.
    RunRoutineNow {
        routine_id: String,
    },
    /// Loads the newest runs of a routine (`routine.runs`).
    LoadRoutineRuns {
        routine_id: String,
    },
    // Memory. A scope is `global` or a channel ID.
    /// Loads the notes of a scope (`memory.list`), or with `deleted` the
    /// ones in Recently deleted.
    LoadMemory {
        scope: String,
        deleted: bool,
    },
    /// Reads one note with its text. The output is
    /// [`CommandOutput::MemoryNote`].
    ReadMemoryNote {
        scope: String,
        path: String,
    },
    /// Writes a note. `expected_sha256` is the hash that the user started
    /// from, or `None` for a new note. A note that changed since then fails
    /// with the hub code `stale_write`, and the text stays with the caller.
    /// The output is [`CommandOutput::MemoryNote`].
    WriteMemoryNote {
        scope: String,
        path: String,
        content: String,
        expected_sha256: Option<String>,
    },
    /// Moves a note to Recently deleted.
    DeleteMemoryNote {
        scope: String,
        path: String,
        sha256: String,
    },
    /// Brings a note back from Recently deleted.
    RestoreMemoryNote {
        scope: String,
        path: String,
    },
    /// Renames a note. The output is [`CommandOutput::MemoryNote`].
    MoveMemoryNote {
        scope: String,
        from_path: String,
        to_path: String,
        sha256: String,
    },
    /// The output is [`CommandOutput::MemoryHistory`].
    LoadMemoryHistory {
        scope: String,
        path: String,
    },
    /// The output is [`CommandOutput::SkillFiles`].
    LoadSkillFiles {
        skill_id: String,
        /// The version that waits for review, not the installed one.
        pending: bool,
    },
    /// The output is [`CommandOutput::SkillFile`].
    ReadSkillFile {
        skill_id: String,
        path: String,
        pending: bool,
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
