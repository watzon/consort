//! Client logic shared by the desktop app and, later, the mobile apps: the
//! connection to the hub, pairing, the local cache, the reducers that make
//! view state, and the commands that the UI sends.
//!
//! The client runs its own Tokio runtime on a background thread. The API is
//! async functions and a [`Subscription`] to view-state changes. Both work on
//! any executor, so GPUI can await them on its foreground executor. Public
//! types are plain records and enums, so UniFFI can export them.

pub mod cache;
pub mod command;
pub mod derive;
pub mod error;
pub mod format;
pub mod hub;
pub mod limits;
pub mod mock;
mod mock_backend;
pub mod reduce;
pub mod store;
pub mod view;

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};

pub use command::{
    AcpSpec, ChannelChanges, Command, CommandOutput, NewAccount, NewChannel, SecretEnv, SecretText,
};
pub use error::ClientError;
pub use store::Drafts;
pub use view::*;

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
        command::check_channel_id(&channel_id)?;
        self.run(Command::SendMessage {
            channel_id,
            body,
            client_message_id: new_client_message_id(),
        })
        .await
    }

    /// Pairs with a hub: `POST /v1/pair/client` with the setup or pairing
    /// code, then connects.
    pub async fn pair(&self, hub_url: String, code: String) -> Result<(), ClientError> {
        self.run(Command::Pair { hub_url, code }).await
    }

    /// Changes the text of one of the user's own messages.
    pub async fn edit_message(&self, message_id: String, body: String) -> Result<(), ClientError> {
        self.run(Command::EditMessage { message_id, body }).await
    }

    pub async fn reply_to_task(&self, task_id: String, body: String) -> Result<(), ClientError> {
        self.run(Command::ReplyToTask {
            task_id,
            body,
            client_message_id: new_client_message_id(),
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

    /// Makes an account on a worker and returns its ID.
    pub async fn create_account(&self, account: NewAccount) -> Result<String, ClientError> {
        match self.run_for(Command::CreateAccount(account)).await? {
            CommandOutput::AccountCreated(id) => Ok(id),
            _ => Err(ClientError::Other(
                "The hub did not say which account it made. Open Settings > Workers to find it"
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
