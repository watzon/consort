//! Client logic shared by the desktop app and, later, the mobile apps: the
//! connection to the hub, pairing, the local cache, the reducers that make
//! view state, and the commands that the UI sends.
//!
//! The client runs its own Tokio runtime on a background thread. The API is
//! async functions and a [`Subscription`] to view-state changes. Both work on
//! any executor, so GPUI can await them on its foreground executor. Public
//! types are plain records and enums, so UniFFI can export them.

pub mod access;
pub mod activity;
pub mod admin;
pub mod approval_text;
pub mod assistant_status;
pub mod blob;
pub mod cache;
pub mod chat;
pub mod command;
pub mod command_check;
pub mod connectors;
pub mod convo;
pub mod derive;
pub mod document;
pub mod drafts;
pub mod error;
pub mod extensions_view;
pub mod fallback;
pub mod files;
pub mod format;
pub mod hub;
pub mod limits;
pub mod local_hub;
pub mod memory;
pub mod mock;
pub mod mock_admin;
mod mock_backend;
mod mock_backend_ext;
pub mod mock_convo;
pub mod mock_extensions;
pub mod mock_files;
pub mod mock_first;
pub mod mock_local;
pub mod mock_monid;
pub mod mock_offline;
pub mod mock_proactivity;
pub mod mock_reach;
pub mod mock_routines;
pub mod mock_versions;
pub mod mock_visual;
pub mod mock_voice;
pub mod monid;
pub mod notify;
pub mod offline;
pub mod persist;
pub mod picker;
pub mod plan;
pub mod proactivity;
pub mod reach;
pub mod reduce;
pub mod reduce_convo;
pub mod routines;
pub mod setup;
pub mod skills;
pub mod store;
pub mod task_model;
#[cfg(test)]
mod test_entities;
pub mod turn_errors;
pub mod update;
pub mod uploads;
pub mod versions;
pub mod view;
pub mod visual;
pub mod voice;

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};

pub use assistant_status::AssistantStatusView;
pub use command::{
    AcpSpec, AttachInput, ChannelChanges, Command, CommandOutput, ConnectionChanges, NewAccount,
    NewChannel, NewConnection, PastedBytes, SecretEnv, SecretText, SkillChanges,
};
pub use command_check::{
    BoundaryView, CONFIDENCE_LIMITS, CheckDetailView, CheckEndpointChoice, CheckSourceKind,
    CommandCheckChoice, CommandCheckView, EndpointView, TestResultView,
};
pub use connectors::*;
pub use consort_proto::model::{DEFAULT_CONFIDENCE_LIMIT, GuideState};
pub use consort_proto::session_host_link::MONID_PROMPT_BLOCK;
pub use drafts::{DraftOutcomeView, ShownDraftView, draft_title};
pub use error::ClientError;
pub use fallback::{
    ChannelFallbackView, FALLBACK_STEPS_MAX, FallbackCauseView, FallbackNoticeKind,
    FallbackNoticeView, FallbackPathView, FallbackScopeView, FallbackStepView, PathLineView,
    PathOutcomeView, StepSkip,
};
pub use files::{AttachmentView, ChannelFileView, FilesListing};
pub use monid::{
    MonidAccessView, MonidCallDetailView, MonidCallRowView, MonidCallStateView, MonidCardView,
    MonidLimitsView, MonidSettingsView, MonidToolView, MonidUnattendedView, MonidView,
    find_call_row, money_text,
};
pub use skills::*;
pub use store::Drafts;
pub use turn_errors::{ErrorAction, ErrorCardView, TryAgainTarget, TurnErrorKindView, TurnFailed};
pub use uploads::{UploadStatus, UploadTarget, UploadView};
pub use view::*;
pub use visual::{VisualImageFile, VisualView};

pub(crate) type Reply = oneshot::Sender<Result<CommandOutput, ClientError>>;

struct Request {
    command: Command,
    reply: Reply,
}

/// A handle to the client. Cheap to clone. The background runtime stops when
/// the last handle drops. Dropping it never waits for the runtime, so it is
/// safe on a UI thread.
#[derive(Clone)]
pub struct Client {
    requests: mpsc::UnboundedSender<Request>,
    state: watch::Receiver<Arc<ViewState>>,
    /// Where the drafts live. `None` for the mock and for tests.
    drafts: Option<store::Store>,
    _runtime: Arc<RuntimeThread>,
}

/// The background thread that runs the client's Tokio runtime.
struct RuntimeThread {
    stop: Option<oneshot::Sender<()>>,
}

impl Drop for RuntimeThread {
    /// Tells the runtime to stop and returns at once. The thread ends on its
    /// own. Nothing waits for a call that is stuck on the hub, and the
    /// outbox is on disk already, so nothing is lost.
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

/// Changes to the view state.
pub struct Subscription {
    rx: watch::Receiver<Arc<ViewState>>,
}

impl Subscription {
    /// Waits for the next change and returns the new state. Returns `None`
    /// when the client stopped.
    pub async fn next(&mut self) -> Option<Arc<ViewState>> {
        self.rx.changed().await.ok()?;
        Some(self.rx.borrow_and_update().clone())
    }

    /// The state now.
    pub fn current(&self) -> Arc<ViewState> {
        self.rx.borrow().clone()
    }
}

/// What a backend does with commands. The mock and the hub connection are
/// the two backends. `handle` must not wait: it starts the work and sends the
/// result on `reply` when it is done, so one slow command never holds up the
/// next one.
trait Backend: Send + 'static {
    fn handle(&mut self, command: Command, reply: Reply, publish: &watch::Sender<Arc<ViewState>>);
}

fn spawn_runtime<F, Fut>(name: &str, run: F) -> Result<RuntimeThread, ClientError>
where
    F: FnOnce(oneshot::Receiver<()>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name(format!("{name}-worker"))
        .enable_all()
        .build()
        .map_err(|err| ClientError::Other(format!("Cannot start the client runtime: {err}")))?;
    let (stop_tx, stop_rx) = oneshot::channel();
    // The thread is not joined: see `RuntimeThread::drop`.
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            runtime.block_on(run(stop_rx));
            runtime.shutdown_background();
        })
        .map_err(|err| ClientError::Other(format!("Cannot start the client thread: {err}")))?;
    Ok(RuntimeThread {
        stop: Some(stop_tx),
    })
}

async fn serve<B: Backend>(
    mut backend: B,
    mut requests: mpsc::UnboundedReceiver<Request>,
    publish: watch::Sender<Arc<ViewState>>,
    mut stop: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = &mut stop => break,
            request = requests.recv() => {
                let Some(request) = request else { break };
                backend.handle(request.command, request.reply, &publish);
            }
        }
    }
}

impl Client {
    /// A client on mock data from the designs, with no hub.
    pub fn mock() -> Result<Client, ClientError> {
        Client::mock_with(mock::view_state())
    }

    /// A client on the given mock state, with no hub.
    pub fn mock_with(state: ViewState) -> Result<Client, ClientError> {
        let initial = Arc::new(state);
        let (publish, state) = watch::channel(initial.clone());
        let (requests, rx) = mpsc::unbounded_channel();
        let runtime = spawn_runtime("consort-client", move |stop| async move {
            let backend = mock_backend::MockBackend::new((*initial).clone());
            serve(backend, rx, publish, stop).await;
        })?;
        Ok(Client {
            requests,
            state,
            drafts: None,
            _runtime: Arc::new(runtime),
        })
    }

    /// A desktop client for the paired hub, with its identity in the OS
    /// config folder. With no hub paired, the state says `NotPaired` and the
    /// app shows onboarding.
    pub fn start() -> Result<Client, ClientError> {
        Client::start_with(
            store::Store::default_location(),
            hub::ClientOptions::desktop(),
        )
    }

    /// A client with its identity in `store` (or nowhere, for tests).
    pub fn start_with(
        store: Option<store::Store>,
        options: hub::ClientOptions,
    ) -> Result<Client, ClientError> {
        let (publish, state) = watch::channel(Arc::new(ViewState::default()));
        let drafts = store.clone();
        let shared = hub::Shared::new(store, options, publish.clone())?;
        let (requests, rx) = mpsc::unbounded_channel();
        let runtime = spawn_runtime("consort-client", move |stop| async move {
            tokio::spawn(hub::run_link(shared.clone()));
            tokio::spawn(hub::run_publisher(shared.clone()));
            tokio::spawn(hub::run_outbox(shared.clone()));
            tokio::spawn(hub::run_loader(shared.clone()));
            tokio::spawn(hub::run_logos(shared.clone()));
            tokio::spawn(hub::run_save(shared.clone()));
            let backend = hub::HubBackend { shared };
            serve(backend, rx, publish, stop).await;
        })?;
        Ok(Client {
            requests,
            state,
            drafts,
            _runtime: Arc::new(runtime),
        })
    }

    /// The view state now.
    pub fn state(&self) -> Arc<ViewState> {
        self.state.borrow().clone()
    }

    /// A subscription to view-state changes.
    pub fn subscribe(&self) -> Subscription {
        Subscription {
            rx: self.state.clone(),
        }
    }

    /// Sends one command and waits for the backend to finish it.
    pub async fn run(&self, command: Command) -> Result<(), ClientError> {
        self.run_for(command).await.map(|_| ())
    }

    /// Sends one command, waits for the backend to finish it, and gives
    /// back what it made.
    pub async fn run_for(&self, command: Command) -> Result<CommandOutput, ClientError> {
        let (reply, rx) = oneshot::channel();
        self.requests
            .send(Request { command, reply })
            .map_err(|_| ClientError::Stopped)?;
        rx.await.map_err(|_| ClientError::Stopped)?
    }

    /// Sends one command and blocks the calling thread until it is done. For
    /// start-up work before a UI loop runs; never call it on a UI thread that
    /// is drawing.
    pub fn run_blocking(&self, command: Command) -> Result<CommandOutput, ClientError> {
        let (reply, rx) = oneshot::channel();
        self.requests
            .send(Request { command, reply })
            .map_err(|_| ClientError::Stopped)?;
        rx.blocking_recv().map_err(|_| ClientError::Stopped)?
    }

    /// Sends a message. The client makes the `client_message_id`, so a retry
    /// never makes a second message.
    pub async fn send_message(&self, channel_id: String, body: String) -> Result<(), ClientError> {
        self.send_message_with(channel_id, body, Vec::new()).await
    }

    /// Sends a message with the uploads `attachments` (upload IDs). It
    /// waits until they are on the hub.
    pub async fn send_message_with(
        &self,
        channel_id: String,
        body: String,
        attachments: Vec<String>,
    ) -> Result<(), ClientError> {
        command::check_channel_id(&channel_id)?;
        self.run(Command::SendMessage {
            channel_id,
            body,
            client_message_id: new_client_message_id(),
            attachments,
        })
        .await
    }

    /// Attaches files to a target and returns the upload IDs.
    pub async fn attach(
        &self,
        target: UploadTarget,
        inputs: Vec<AttachInput>,
    ) -> Result<Vec<String>, ClientError> {
        match self.run_for(Command::Attach { target, inputs }).await? {
            CommandOutput::Attached(ids) => Ok(ids),
            _ => Ok(Vec::new()),
        }
    }

    /// Downloads a blob into the local cache and returns its path. With a
    /// name, the path is a copy with that file name.
    pub async fn fetch_blob(
        &self,
        sha256: String,
        size: u64,
        name: Option<String>,
    ) -> Result<String, ClientError> {
        match self
            .run_for(Command::FetchBlob { sha256, size, name })
            .await?
        {
            CommandOutput::LocalFile(path) => Ok(path),
            _ => Err(ClientError::Other(
                "Consort could not find the downloaded file. Try again".into(),
            )),
        }
    }

    /// Pairs with a hub: `POST /v1/pair/client` with the setup or pairing
    /// code, then connects.
    pub async fn pair(&self, hub_url: String, code: String) -> Result<(), ClientError> {
        self.run(Command::Pair { hub_url, code }).await
    }

    /// Forgets the hub. This removes the identity, the outbox and the saved
    /// data from this computer. The app then asks for a pairing again.
    pub async fn unpair(&self) -> Result<(), ClientError> {
        self.run(Command::Unpair).await
    }

    /// Changes the text of one of the user's own messages.
    pub async fn edit_message(&self, message_id: String, body: String) -> Result<(), ClientError> {
        self.run(Command::EditMessage { message_id, body }).await
    }

    pub async fn reply_to_task(&self, task_id: String, body: String) -> Result<(), ClientError> {
        self.reply_to_task_with(task_id, body, Vec::new()).await
    }

    /// Replies in a task thread with the uploads `attachments`.
    pub async fn reply_to_task_with(
        &self,
        task_id: String,
        body: String,
        attachments: Vec<String>,
    ) -> Result<(), ClientError> {
        self.run(Command::ReplyToTask {
            task_id,
            body,
            client_message_id: new_client_message_id(),
            attachments,
        })
        .await
    }

    pub async fn answer_approval(
        &self,
        approval_id: String,
        choice: ApprovalChoice,
    ) -> Result<(), ClientError> {
        self.run(Command::AnswerApproval {
            approval_id,
            choice,
            checked: None,
        })
        .await
    }

    pub async fn answer_decision(
        &self,
        decision_id: String,
        option: Option<u32>,
        text: Option<String>,
    ) -> Result<(), ClientError> {
        self.run(Command::AnswerDecision {
            decision_id,
            option,
            text,
        })
        .await
    }

    pub async fn create_channel(&self, channel: NewChannel) -> Result<(), ClientError> {
        self.run(Command::CreateChannel(channel)).await
    }

    pub async fn update_channel(
        &self,
        channel_id: String,
        changes: ChannelChanges,
    ) -> Result<(), ClientError> {
        self.run(Command::UpdateChannel {
            channel_id,
            changes,
        })
        .await
    }

    pub async fn archive_channel(&self, channel_id: String) -> Result<(), ClientError> {
        self.run(Command::ArchiveChannel { channel_id }).await
    }

    pub async fn delete_channel(&self, channel_id: String) -> Result<(), ClientError> {
        self.run(Command::DeleteChannel { channel_id }).await
    }

    /// Brings an archived channel back.
    pub async fn unarchive_channel(&self, channel_id: String) -> Result<(), ClientError> {
        self.run(Command::UnarchiveChannel { channel_id }).await
    }

    /// Puts a channel in a group, or takes it out of its group with `None`.
    pub async fn move_channel(
        &self,
        channel_id: String,
        group_id: Option<String>,
    ) -> Result<(), ClientError> {
        self.run(Command::MoveChannel {
            channel_id,
            group_id,
        })
        .await
    }

    /// Makes a group. With a channel, the channel moves into the new group.
    pub async fn create_group(
        &self,
        name: String,
        channel_id: Option<String>,
    ) -> Result<(), ClientError> {
        self.run(Command::CreateGroup { name, channel_id }).await
    }

    /// Hides a task in any status.
    pub async fn archive_task(&self, task_id: String) -> Result<(), ClientError> {
        self.run(Command::ArchiveTask { task_id }).await
    }

    /// Connects to the hub now, without waiting for the retry timer.
    pub async fn reconnect(&self) -> Result<(), ClientError> {
        self.run(Command::Reconnect).await
    }

    /// The drafts that were saved when the app last ran. A missing or damaged
    /// file reads as no drafts.
    pub fn load_drafts(&self) -> Drafts {
        self.drafts
            .as_ref()
            .map(|store| store.load_drafts())
            .unwrap_or_default()
    }

    /// Saves the drafts in the client's folder, through a temporary file
    /// and a rename. Does nothing when the client has no folder.
    pub fn save_drafts(&self, drafts: &Drafts) -> Result<(), ClientError> {
        match &self.drafts {
            Some(store) => store.save_drafts(drafts),
            None => Ok(()),
        }
    }

    /// The notification settings of this app. They live on this computer,
    /// not on the hub. The mock and tests get the defaults.
    pub fn load_notification_settings(&self) -> notify::NotificationSettings {
        self.drafts
            .as_ref()
            .map(|store| store.load_notification_settings())
            .unwrap_or_default()
    }

    /// Saves the notification settings in the client's folder. Does nothing
    /// when the client has no folder.
    pub fn save_notification_settings(
        &self,
        settings: &notify::NotificationSettings,
    ) -> Result<(), ClientError> {
        match &self.drafts {
            Some(store) => store.save_notification_settings(settings),
            None => Ok(()),
        }
    }

    /// The window layout of this app. The mock and tests get the defaults.
    pub fn load_layout(&self) -> store::LayoutSettings {
        self.drafts
            .as_ref()
            .map(|store| store.load_layout())
            .unwrap_or_default()
    }

    /// Saves the window layout in the client's folder. Does nothing when the
    /// client has no folder.
    pub fn save_layout(&self, layout: &store::LayoutSettings) -> Result<(), ClientError> {
        match &self.drafts {
            Some(store) => store.save_layout(layout),
            None => Ok(()),
        }
    }

    /// Loads the replies of a task's thread when they are not loaded yet.
    pub async fn load_thread(&self, task_id: String) -> Result<(), ClientError> {
        self.run(Command::LoadThread { task_id }).await
    }

    /// Loads the next page of older messages in a channel.
    pub async fn load_older(&self, channel_id: String) -> Result<(), ClientError> {
        self.run(Command::LoadOlder { channel_id }).await
    }

    pub async fn resolve_task(&self, task_id: String) -> Result<(), ClientError> {
        self.run(Command::ResolveTask { task_id }).await
    }

    pub async fn reopen_task(&self, task_id: String) -> Result<(), ClientError> {
        self.run(Command::ReopenTask { task_id }).await
    }

    pub async fn retry_task(&self, task_id: String) -> Result<(), ClientError> {
        self.run(Command::RetryTask { task_id }).await
    }

    /// Sets the model and the options of a task that has started. The hub
    /// refuses a Resolved or archived task, and a worker that does not have
    /// the account. Use [`TaskThreadView::model_options`] to know which
    /// picks start a new session.
    pub async fn set_task_model(
        &self,
        task_id: String,
        selection: SelectionView,
    ) -> Result<(), ClientError> {
        self.run(Command::SetTaskModel { task_id, selection }).await
    }

    /// Saves the user's change to the title or text of a draft card from
    /// `show_draft`. Copy needs no call: the app copies the text itself.
    pub async fn edit_draft(
        &self,
        message_id: String,
        title: Option<String>,
        body: Option<String>,
    ) -> Result<(), ClientError> {
        self.run(Command::EditDraft {
            message_id,
            title,
            body,
        })
        .await
    }

    /// Asks for an image of a visual at `width` CSS pixels, at `scale` device
    /// pixels per CSS pixel, in the theme of `style`. The file is in the
    /// local blob cache. The client keeps the images it got in this run: the
    /// same ask is answered at once, and two asks at once share one request.
    ///
    /// The hub may need up to 60 seconds to draw a visual that it has not
    /// drawn before.
    ///
    /// - [`ClientError::Unavailable`]: no session host is online, or its
    ///   browser is still installing. Try again later.
    /// - `ClientError::Hub` with the code `not_found`: the message has no
    ///   visual, or the app cannot read it.
    pub async fn render_visual(
        &self,
        message_id: String,
        width: u32,
        scale: u8,
        style: consort_proto::model::VisualStyle,
    ) -> Result<VisualImageFile, ClientError> {
        visual::check_render(width, scale, &style)?;
        match self
            .run_for(Command::RenderVisual {
                message_id,
                width,
                scale,
                style,
            })
            .await?
        {
            CommandOutput::VisualImage(file) => Ok(file),
            _ => Err(ClientError::Other(
                "Consort could not find the image of the visual. Try again".into(),
            )),
        }
    }

    /// The Try again button of an error card.
    ///
    /// - A task retries: the hub starts a new session from the brief.
    /// - A channel sends the last message of the user again, as a new
    ///   message. The first one stays in the timeline, so it shows twice.
    ///   A failed routine run or check-in has no message to send, and its
    ///   card has no Try again.
    pub async fn try_again(&self, target: TryAgainTarget) -> Result<(), ClientError> {
        match target {
            TryAgainTarget::Task { task_id } => self.retry_task(task_id).await,
            TryAgainTarget::ResendMessage { channel_id, body } => {
                self.send_message(channel_id, body).await
            }
            TryAgainTarget::Greeting => self.run(Command::GuideTryAgain).await,
        }
    }

    pub async fn stop_task(&self, task_id: String) -> Result<(), ClientError> {
        self.run(Command::StopTask { task_id }).await
    }

    pub async fn worker_pairing_code(&self) -> Result<(), ClientError> {
        self.run(Command::WorkerPairingCode).await
    }

    pub async fn rename_worker(&self, worker_id: String, name: String) -> Result<(), ClientError> {
        self.run(Command::RenameWorker { worker_id, name }).await
    }

    pub async fn set_worker_tags(
        &self,
        worker_id: String,
        tags: Vec<String>,
    ) -> Result<(), ClientError> {
        self.run(Command::SetWorkerTags { worker_id, tags }).await
    }

    /// Changes the limits of a worker. A `None` keeps its value. It fails when
    /// the worker is offline.
    pub async fn set_worker_policy(
        &self,
        worker_id: String,
        max_approval_mode: Option<ApprovalMode>,
        allowed_roots: Option<Vec<String>>,
        allow_shell: Option<bool>,
    ) -> Result<(), ClientError> {
        self.run(Command::SetWorkerPolicy {
            worker_id,
            max_approval_mode,
            allowed_roots,
            allow_shell,
        })
        .await
    }

    /// Stars a model in the model picker, or takes the star away. The hub
    /// keeps the favorites, so every client shows them.
    pub async fn set_favorite_model(
        &self,
        account_label: String,
        model_id: String,
        favorite: bool,
    ) -> Result<(), ClientError> {
        self.run(Command::SetFavoriteModel {
            account_label,
            model_id,
            favorite,
        })
        .await
    }

    /// Replaces the source of the command check.
    pub async fn set_command_check(&self, choice: CommandCheckChoice) -> Result<(), ClientError> {
        self.run(Command::SetCommandCheck { choice }).await
    }

    /// Adds an API endpoint, or changes one.
    pub async fn save_endpoint(
        &self,
        id: Option<String>,
        name: String,
        base_url: String,
        api_key: Option<String>,
    ) -> Result<(), ClientError> {
        self.run(Command::SaveEndpoint {
            id,
            name,
            base_url,
            api_key,
        })
        .await
    }

    /// Removes an API endpoint.
    pub async fn remove_endpoint(&self, id: String) -> Result<(), ClientError> {
        self.run(Command::RemoveEndpoint { id }).await
    }

    /// Keeps the choice Do this later of the setup on the hub, for every
    /// client of this user.
    pub async fn set_setup_later(&self, later: bool) -> Result<(), ClientError> {
        self.run(Command::SetSetupLater { later }).await
    }

    /// Keeps the name of the user on the hub, for every client. Empty clears
    /// it.
    pub async fn set_user_name(&self, name: String) -> Result<(), ClientError> {
        self.run(Command::SetUserName { name }).await
    }

    /// Replaces the fallback path: the built-in row and the steps, in
    /// order.
    pub async fn set_fallback_path(
        &self,
        other_accounts: bool,
        steps: Vec<SelectionView>,
    ) -> Result<(), ClientError> {
        self.run(Command::SetFallbackPath {
            other_accounts,
            steps,
        })
        .await
    }

    /// Takes the assistant of a channel back to its first choice now.
    pub async fn go_back(&self, channel_id: String) -> Result<(), ClientError> {
        self.run(Command::GoBack { channel_id }).await
    }

    /// Makes an account on a worker and returns its ID.
    pub async fn create_account(&self, account: NewAccount) -> Result<String, ClientError> {
        match self.run_for(Command::CreateAccount(account)).await? {
            CommandOutput::AccountCreated(id) => Ok(id),
            _ => Err(ClientError::Other(
                "The hub did not say which account it made. Open Settings > Providers to find it"
                    .into(),
            )),
        }
    }

    /// Renames an account. The new name must not be empty.
    pub async fn rename_account(
        &self,
        worker_id: String,
        account_id: String,
        label: String,
    ) -> Result<(), ClientError> {
        self.run(Command::UpdateAccount {
            worker_id,
            account_id,
            label: Some(label),
            env_set: SecretEnv::new(),
            env_remove: Vec::new(),
        })
        .await
    }

    /// Saves secret variables, such as an API key, for an account.
    pub async fn set_account_secrets(
        &self,
        worker_id: String,
        account_id: String,
        env_set: SecretEnv,
    ) -> Result<(), ClientError> {
        self.run(Command::UpdateAccount {
            worker_id,
            account_id,
            label: None,
            env_set,
            env_remove: Vec::new(),
        })
        .await
    }

    pub async fn remove_account(
        &self,
        worker_id: String,
        account_id: String,
    ) -> Result<(), ClientError> {
        self.run(Command::RemoveAccount {
            worker_id,
            account_id,
        })
        .await
    }

    pub async fn sign_out_account(
        &self,
        worker_id: String,
        account_id: String,
    ) -> Result<(), ClientError> {
        self.run(Command::SignOutAccount {
            worker_id,
            account_id,
        })
        .await
    }

    /// Starts a sign-in and returns its ID. Its steps show in
    /// [`ViewState::sign_ins`].
    pub async fn start_sign_in(
        &self,
        worker_id: String,
        account_id: String,
        method: SignInChoice,
    ) -> Result<String, ClientError> {
        match self
            .run_for(Command::StartSignIn {
                worker_id,
                account_id,
                method,
            })
            .await?
        {
            CommandOutput::SignInStarted(id) => Ok(id),
            _ => Err(ClientError::Other(
                "The hub did not start the sign-in. Try again".into(),
            )),
        }
    }

    pub async fn submit_sign_in_code(
        &self,
        sign_in_id: String,
        code: String,
    ) -> Result<(), ClientError> {
        self.run(Command::SubmitSignInCode {
            sign_in_id,
            code: SecretText(code),
        })
        .await
    }

    pub async fn cancel_sign_in(&self, sign_in_id: String) -> Result<(), ClientError> {
        self.run(Command::CancelSignIn { sign_in_id }).await
    }

    pub async fn check_tools(&self, worker_id: String) -> Result<(), ClientError> {
        self.run(Command::CheckTools { worker_id }).await
    }

    pub async fn install_tool(&self, worker_id: String, tool: String) -> Result<(), ClientError> {
        self.run(Command::InstallTool { worker_id, tool }).await
    }

    pub async fn load_acp_registry(&self) -> Result<(), ClientError> {
        self.run(Command::LoadAcpRegistry).await
    }
}

/// A new idempotency key for a message.
pub fn new_client_message_id() -> String {
    uuid::Uuid::now_v7().to_string()
}
