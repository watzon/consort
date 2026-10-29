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
pub mod mock;
mod mock_backend;
pub mod reduce;
pub mod store;
pub mod view;

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};

pub use command::{ChannelChanges, Command, NewChannel};
pub use error::ClientError;
pub use view::*;

struct Request {
    command: Command,
    reply: oneshot::Sender<Result<(), ClientError>>,
}

/// A handle to the client. Cheap to clone. The background runtime stops when
/// the last handle drops.
#[derive(Clone)]
pub struct Client {
    requests: mpsc::UnboundedSender<Request>,
    state: watch::Receiver<Arc<ViewState>>,
    _runtime: Arc<RuntimeThread>,
}

/// The background thread that runs the client's Tokio runtime.
struct RuntimeThread {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for RuntimeThread {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
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
/// the two backends.
trait Backend: Send + 'static {
    fn handle(
        &mut self,
        command: Command,
        publish: &watch::Sender<Arc<ViewState>>,
    ) -> impl std::future::Future<Output = Result<(), ClientError>> + Send;
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
    let thread = std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            runtime.block_on(run(stop_rx));
            runtime.shutdown_background();
        })
        .map_err(|err| ClientError::Other(format!("Cannot start the client thread: {err}")))?;
    Ok(RuntimeThread {
        stop: Some(stop_tx),
        thread: Some(thread),
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
                let result = backend.handle(request.command, &publish).await;
                let _ = request.reply.send(result);
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
        let shared = hub::Shared::new(store, options, publish.clone())?;
        let (requests, rx) = mpsc::unbounded_channel();
        let runtime = spawn_runtime("consort-client", move |stop| async move {
            tokio::spawn(hub::run_link(shared.clone()));
            let backend = hub::HubBackend { shared };
            serve(backend, rx, publish, stop).await;
        })?;
        Ok(Client {
            requests,
            state,
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
        let (reply, rx) = oneshot::channel();
        self.requests
            .send(Request { command, reply })
            .map_err(|_| ClientError::Stopped)?;
        rx.await.map_err(|_| ClientError::Stopped)?
    }

    /// Sends one command and blocks the calling thread until it is done. For
    /// start-up work before a UI loop runs; never call it on a UI thread that
    /// is drawing.
    pub fn run_blocking(&self, command: Command) -> Result<(), ClientError> {
        let (reply, rx) = oneshot::channel();
        self.requests
            .send(Request { command, reply })
            .map_err(|_| ClientError::Stopped)?;
        rx.blocking_recv().map_err(|_| ClientError::Stopped)?
    }

    /// Sends a message. The client makes the `client_message_id`, so a retry
    /// never makes a second message.
    pub async fn send_message(&self, channel_id: String, body: String) -> Result<(), ClientError> {
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

    pub async fn set_worker_labels(
        &self,
        worker_id: String,
        labels: Vec<String>,
    ) -> Result<(), ClientError> {
        self.run(Command::SetWorkerLabels { worker_id, labels })
            .await
    }
}

/// A new idempotency key for a message.
pub fn new_client_message_id() -> String {
    uuid::Uuid::now_v7().to_string()
}
