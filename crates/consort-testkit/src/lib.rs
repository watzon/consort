//! Test helpers for Consort.
//!
//! [`FakeHarness`] is a harness that plays a script of events. Each turn
//! plays the next entry of [`FakeScript::turns`] (the last entry repeats).
//! A script can open an approval and wait for the answer, so tests can
//! round-trip `session.respond` without a real harness.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use consort_harness::{
    AccountConfig, AccountStatus, Answer, Capabilities, EventReceiver, Harness, HarnessError,
    HarnessKind, HarnessSession, SessionSpec,
};
use consort_proto::ids::{ItemId, NativeRef, RequestId, TurnId};
use consort_proto::model::{
    ApprovalDetails, ApprovalMode, AssistantMessagePayload, AuthState, CommandPayload, ItemPayload,
    ItemStatus, ModelInfo, RequestOption, RequestOptionKind, SessionItem, SessionState,
};
use consort_proto::time::Timestamp;
use consort_proto::worker_link::{
    RequestBody, RequestOpened, RequestOutcome, SessionEvent, TurnEndReason, TurnInput,
};
use tokio::sync::{mpsc, oneshot, watch};

/// One step of a fake turn.
#[derive(Debug, Clone)]
pub enum FakeStep {
    /// Send this event as it is.
    Event(SessionEvent),
    /// A completed assistant message.
    Text(String),
    /// A completed command with its output.
    Command {
        /// The command line.
        command: String,
        /// The output.
        output: String,
    },
    /// Open a command approval and wait for the answer. The turn goes on
    /// after any answer; a deny adds a "Denied." message.
    Approval {
        /// The request ID.
        request_id: String,
        /// The command.
        command: String,
    },
    /// Wait.
    Sleep(Duration),
    /// End the turn as failed.
    Fail(String),
}

/// The script of a fake session.
#[derive(Debug, Clone, Default)]
pub struct FakeScript {
    /// One list of steps per turn. The last list repeats.
    pub turns: Vec<Vec<FakeStep>>,
}

impl FakeScript {
    /// A script whose every turn replies with one message.
    pub fn reply(text: &str) -> Self {
        Self {
            turns: vec![vec![FakeStep::Text(text.to_owned())]],
        }
    }
}

/// What a fake harness saw, for assertions.
#[derive(Debug, Default)]
pub struct FakeRecord {
    /// The spec of every `open`.
    pub opened: Vec<SessionSpec>,
    /// Every turn input.
    pub turns: Vec<TurnInput>,
    /// Every steer input.
    pub steers: Vec<TurnInput>,
    /// Every answer.
    pub answers: Vec<(RequestId, Answer)>,
    /// How many interrupts arrived.
    pub interrupts: usize,
    /// How many sessions closed.
    pub closed: usize,
}

/// A harness that plays a script.
#[derive(Debug, Clone)]
pub struct FakeHarness {
    kind: HarnessKind,
    script: FakeScript,
    record: Arc<Mutex<FakeRecord>>,
    capabilities: Capabilities,
    /// When set, `open` with `resume` fails.
    pub fail_resume: bool,
}

impl FakeHarness {
    /// A fake harness that reports itself as `kind`.
    pub fn new(kind: HarnessKind, script: FakeScript) -> Self {
        Self {
            kind,
            script,
            record: Arc::new(Mutex::new(FakeRecord::default())),
            capabilities: Capabilities {
                steer: true,
                resume: true,
                usage: false,
                mcp_http: true,
                approval_modes: ApprovalMode::ALL.to_vec(),
                ..Capabilities::default()
            },
            fail_resume: false,
        }
    }

    /// What the harness saw so far.
    pub fn record(&self) -> Arc<Mutex<FakeRecord>> {
        self.record.clone()
    }

    /// The model that the fake account offers.
    pub fn model() -> ModelInfo {
        ModelInfo {
            id: "fake-model".to_owned(),
            name: "Fake model".to_owned(),
            options: Vec::new(),
        }
    }
}

fn now_item(spec: &SessionSpec, turn: &TurnId, payload: ItemPayload) -> SessionEvent {
    let now = Timestamp::now();
    SessionEvent::Item(SessionItem {
        id: ItemId::new(),
        session_id: spec.session_id,
        turn_id: turn.clone(),
        payload,
        status: ItemStatus::Completed,
        created_at: now,
        updated_at: now,
    })
}

#[async_trait]
impl Harness for FakeHarness {
    fn kind(&self) -> HarnessKind {
        self.kind
    }

    fn capabilities(&self, _account: &AccountConfig) -> Capabilities {
        self.capabilities.clone()
    }

    async fn probe(&self, _account: &AccountConfig) -> AccountStatus {
        AccountStatus {
            harness_version: Some("0.0.0-fake".to_owned()),
            auth_state: AuthState::SignedIn,
            auth_detail: Some("Fake account".to_owned()),
            models: vec![Self::model()],
            capabilities: self.capabilities.clone(),
        }
    }

    async fn open(
        &self,
        _account: &AccountConfig,
        spec: SessionSpec,
    ) -> Result<(Box<dyn HarnessSession>, EventReceiver), HarnessError> {
        if spec.resume.is_some() && self.fail_resume {
            return Err(HarnessError::ResumeFailed(
                "the fake harness forgot".to_owned(),
            ));
        }
        if let Ok(mut record) = self.record.lock() {
            record.opened.push(spec.clone());
        }
        let (events, rx) = mpsc::channel(256);
        let native = spec
            .resume
            .clone()
            .unwrap_or_else(|| NativeRef::new(format!("fake-{}", spec.session_id)));
        let _ = events
            .send(SessionEvent::NativeRef {
                native_ref: native.clone(),
            })
            .await;
        let _ = events
            .send(SessionEvent::State {
                state: SessionState::Idle,
            })
            .await;
        let (cancel, _) = watch::channel(0_u64);
        Ok((
            Box::new(FakeSession {
                spec,
                script: self.script.clone(),
                capabilities: self.capabilities.clone(),
                native,
                events,
                record: self.record.clone(),
                waiters: Arc::new(Mutex::new(HashMap::new())),
                turn_index: 0,
                cancel,
            }),
            rx,
        ))
    }
}

type Waiters = Arc<Mutex<HashMap<RequestId, oneshot::Sender<Answer>>>>;

struct FakeSession {
    spec: SessionSpec,
    script: FakeScript,
    capabilities: Capabilities,
    native: NativeRef,
    events: mpsc::Sender<SessionEvent>,
    record: Arc<Mutex<FakeRecord>>,
    waiters: Waiters,
    turn_index: usize,
    cancel: watch::Sender<u64>,
}

async fn play(
    spec: SessionSpec,
    steps: Vec<FakeStep>,
    turn: TurnId,
    events: mpsc::Sender<SessionEvent>,
    waiters: Waiters,
    mut cancel: watch::Receiver<u64>,
) {
    let send = |e: SessionEvent| {
        let events = events.clone();
        async move {
            let _ = events.send(e).await;
        }
    };
    send(SessionEvent::TurnStarted {
        turn_id: turn.clone(),
    })
    .await;
    send(SessionEvent::State {
        state: SessionState::Active,
    })
    .await;
    let mut reason = TurnEndReason::Completed;
    let mut error = None;
    for step in steps {
        if cancel.has_changed().unwrap_or(false) {
            reason = TurnEndReason::Interrupted;
            break;
        }
        match step {
            FakeStep::Event(event) => send(event).await,
            FakeStep::Text(text) => {
                send(now_item(
                    &spec,
                    &turn,
                    ItemPayload::AssistantMessage(AssistantMessagePayload { text }),
                ))
                .await;
            }
            FakeStep::Command { command, output } => {
                let payload = CommandPayload {
                    command,
                    output,
                    exit_code: Some(0),
                    ..CommandPayload::default()
                };
                send(now_item(&spec, &turn, ItemPayload::Command(payload))).await;
            }
            FakeStep::Approval {
                request_id,
                command,
            } => {
                let request_id = RequestId::new(request_id);
                let (tx, rx) = oneshot::channel();
                if let Ok(mut w) = waiters.lock() {
                    w.insert(request_id.clone(), tx);
                }
                send(SessionEvent::RequestOpened(RequestOpened {
                    request_id: request_id.clone(),
                    summary: format!("Run `{command}`"),
                    body: RequestBody::Approval(ApprovalDetails::Command { command, cwd: None }),
                    options: vec![
                        RequestOption {
                            id: "allow".into(),
                            label: "Allow".into(),
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
                }))
                .await;
                tokio::select! {
                    answer = rx => {
                        let allowed = answer.ok().and_then(|a| a.option_id).as_deref() == Some("allow");
                        let outcome = if allowed { RequestOutcome::Allowed } else { RequestOutcome::Denied };
                        send(SessionEvent::RequestClosed { request_id, outcome }).await;
                        if !allowed {
                            send(now_item(&spec, &turn, ItemPayload::AssistantMessage(AssistantMessagePayload { text: "Denied.".into() }))).await;
                        }
                    }
                    _ = cancel.changed() => {
                        send(SessionEvent::RequestClosed { request_id, outcome: RequestOutcome::Canceled }).await;
                        reason = TurnEndReason::Interrupted;
                        break;
                    }
                }
            }
            FakeStep::Sleep(duration) => {
                tokio::select! {
                    () = tokio::time::sleep(duration) => {}
                    _ = cancel.changed() => { reason = TurnEndReason::Interrupted; break; }
                }
            }
            FakeStep::Fail(message) => {
                reason = TurnEndReason::Failed;
                error = Some(message);
                break;
            }
        }
    }
    send(SessionEvent::TurnEnded {
        turn_id: turn,
        reason,
        error,
    })
    .await;
    send(SessionEvent::State {
        state: SessionState::Idle,
    })
    .await;
}

#[async_trait]
impl HarnessSession for FakeSession {
    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn native_ref(&self) -> Option<NativeRef> {
        Some(self.native.clone())
    }

    async fn turn(&mut self, input: TurnInput) -> Result<TurnId, HarnessError> {
        if let Ok(mut record) = self.record.lock() {
            record.turns.push(input);
        }
        let steps = self
            .script
            .turns
            .get(self.turn_index)
            .or_else(|| self.script.turns.last())
            .cloned()
            .unwrap_or_default();
        self.turn_index += 1;
        let turn = TurnId::new(format!("fake-turn-{}", self.turn_index));
        tokio::spawn(play(
            self.spec.clone(),
            steps,
            turn.clone(),
            self.events.clone(),
            self.waiters.clone(),
            self.cancel.subscribe(),
        ));
        Ok(turn)
    }

    async fn steer(&mut self, input: TurnInput) -> Result<(), HarnessError> {
        if let Ok(mut record) = self.record.lock() {
            record.steers.push(input);
        }
        Ok(())
    }

    async fn interrupt(&mut self) -> Result<(), HarnessError> {
        if let Ok(mut record) = self.record.lock() {
            record.interrupts += 1;
        }
        self.cancel.send_modify(|n| *n += 1);
        Ok(())
    }

    async fn respond(&mut self, request: RequestId, answer: Answer) -> Result<(), HarnessError> {
        let waiter = self
            .waiters
            .lock()
            .ok()
            .and_then(|mut w| w.remove(&request));
        let Some(waiter) = waiter else {
            return Err(HarnessError::UnknownRequest(request.to_string()));
        };
        if let Ok(mut record) = self.record.lock() {
            record.answers.push((request, answer.clone()));
        }
        let _ = waiter.send(answer);
        Ok(())
    }

    async fn close(self: Box<Self>) -> Result<(), HarnessError> {
        if let Ok(mut record) = self.record.lock() {
            record.closed += 1;
        }
        let _ = self
            .events
            .send(SessionEvent::State {
                state: SessionState::Ended,
            })
            .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::ids::SessionId;
    use std::collections::BTreeMap;

    fn spec() -> SessionSpec {
        SessionSpec {
            session_id: SessionId::new(),
            workspace: std::env::temp_dir(),
            runtime_dir: std::env::temp_dir(),
            model: "fake-model".into(),
            options: Default::default(),
            approval_mode: ApprovalMode::Ask,
            system_prompt_append: String::new(),
            mcp_servers: vec![],
            env: BTreeMap::new(),
            resume: None,
        }
    }

    async fn until_end(rx: &mut EventReceiver) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        while let Some(e) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
        {
            let end = matches!(e, SessionEvent::TurnEnded { .. });
            out.push(e);
            if end {
                break;
            }
        }
        out
    }

    #[tokio::test]
    async fn plays_a_turn_with_an_approval() {
        let script = FakeScript {
            turns: vec![vec![
                FakeStep::Approval {
                    request_id: "r1".into(),
                    command: "ls".into(),
                },
                FakeStep::Text("Done.".into()),
            ]],
        };
        let harness = FakeHarness::new(HarnessKind::ClaudeCode, script);
        let account = AccountConfig::default_for(HarnessKind::ClaudeCode);
        let (mut session, mut rx) = harness.open(&account, spec()).await.unwrap();
        session
            .turn(TurnInput {
                text: "go".into(),
                attachments: vec![],
            })
            .await
            .unwrap();
        loop {
            if let Some(SessionEvent::RequestOpened(_)) = rx.recv().await {
                break;
            }
        }
        session
            .respond(
                RequestId::new("r1"),
                Answer {
                    option_id: Some("allow".into()),
                    text: None,
                },
            )
            .await
            .unwrap();
        let rest = until_end(&mut rx).await;
        assert!(rest.iter().any(|e| matches!(
            e,
            SessionEvent::RequestClosed {
                outcome: RequestOutcome::Allowed,
                ..
            }
        )));
        assert!(matches!(
            rest.last(),
            Some(SessionEvent::TurnEnded {
                reason: TurnEndReason::Completed,
                ..
            })
        ));
        assert_eq!(harness.record().lock().unwrap().answers.len(), 1);
    }

    #[tokio::test]
    async fn interrupt_ends_the_turn() {
        let script = FakeScript {
            turns: vec![vec![FakeStep::Sleep(Duration::from_secs(30))]],
        };
        let harness = FakeHarness::new(HarnessKind::Codex, script);
        let account = AccountConfig::default_for(HarnessKind::Codex);
        let (mut session, mut rx) = harness.open(&account, spec()).await.unwrap();
        session.turn(TurnInput::default()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        session.interrupt().await.unwrap();
        let events = until_end(&mut rx).await;
        assert!(matches!(
            events.last(),
            Some(SessionEvent::TurnEnded {
                reason: TurnEndReason::Interrupted,
                ..
            })
        ));
    }
}
