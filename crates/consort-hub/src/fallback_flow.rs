//! The fallback path in motion: the holds that a failed turn sets, the
//! record on the channel while the assistant runs on a fallback, the notices
//! of a move, and the way back. The rules of the walk are in `fallback`.

use consort_proto::client_api::{ChannelGoBack, ChannelResult};
use consort_proto::ids::{ChannelId, MessageId, TaskId};
use consort_proto::model::{
    Author, CoordinatorFallback, FallbackCause, FallbackNotice, FallbackScope, Message,
    MessageKind, ModelSelection, Worker,
};

use crate::coordinator::{COORDINATOR_APPROVAL_MODE, retire_session_tx};
use crate::error::{HubError, Result};
use crate::fallback::{
    Holds, Moved, OUTAGE_HOLD_MS, Walk, holds_by_label, moved_notice, returned_notice,
    signed_out_notices,
};
use crate::hub::{Caller, Hub};
use crate::limits::{NOTICE_TEXT_MAX_BYTES, cut_text};
use crate::link::events::Effects;
use crate::store::conversation::NewMessage;
use crate::store::coordinator as coord_store;
use crate::store::{Tx, conversation, machine, preferences};
use consort_proto::time::Timestamp;

/// Posts a notice of the fallback path. The body has `kind`, `text` and the
/// fields of [`FallbackNotice`].
pub(crate) fn post_fallback_notice(
    tx: &mut Tx<'_>,
    channel_id: ChannelId,
    thread_root: Option<MessageId>,
    task_id: Option<TaskId>,
    kind: &str,
    text: &str,
    notice: &FallbackNotice,
) -> Result<Message> {
    let mut body = serde_json::to_value(notice)?;
    if let Some(object) = body.as_object_mut() {
        object.insert("kind".to_owned(), kind.into());
        object.insert(
            "text".to_owned(),
            cut_text(text, NOTICE_TEXT_MAX_BYTES).into(),
        );
    }
    let mut new = NewMessage::new(
        channel_id,
        Author::System,
        MessageKind::Notice,
        body.to_string(),
    );
    new.thread_root_id = thread_root;
    new.author_task_id = task_id;
    tx.insert_message(new)
}

/// Holds every account of a harness, because its provider was down or
/// overloaded for three turns. Another account of the same provider would
/// fail the same way.
pub(crate) fn hold_outage(tx: &mut Tx<'_>, workers: &[Worker], harness: &str) -> Result<()> {
    let until = tx.now().plus_millis(OUTAGE_HOLD_MS);
    for worker in workers.iter().filter(|w| w.revoked_at.is_none()) {
        let Some(inventory) = worker.inventory.as_ref() else {
            continue;
        };
        for account in inventory.accounts.iter().filter(|a| a.harness == harness) {
            tx.record_account_limit(account.account_id, until, FallbackCause::Outage, false)?;
        }
    }
    Ok(())
}

/// The record on a channel while its assistant runs on a fallback.
fn record_of(moved: &Moved, now: Timestamp) -> CoordinatorFallback {
    CoordinatorFallback {
        from: moved.first.clone(),
        to: moved.chosen.selection.clone(),
        cause: moved.cause,
        returns_at: crate::fallback::returns_at(&moved.lines),
        moved_at: now,
    }
}

/// Saves the record of a move on the channel and posts the notices.
pub(crate) fn assistant_moved(
    tx: &mut Tx<'_>,
    workers: &[Worker],
    channel_id: ChannelId,
    moved: &Moved,
) -> Result<()> {
    let mut channel = conversation::channel_or_404(tx.db(), channel_id)?;
    channel.coordinator_fallback = Some(Box::new(record_of(moved, tx.now())));
    tx.save_channel(&channel)?;
    let (text, notice) = moved_notice(FallbackScope::Assistant, workers, moved);
    post_fallback_notice(
        tx,
        channel_id,
        None,
        None,
        "account_fallback",
        &text,
        &notice,
    )?;
    for (text, notice) in signed_out_notices(&moved.lines) {
        post_fallback_notice(
            tx,
            channel_id,
            None,
            None,
            "fallback_signed_out",
            &text,
            &notice,
        )?;
    }
    Ok(())
}

/// Clears the record of a channel and posts the notice that the assistant
/// went back. `from` is what the assistant ran on.
pub(crate) fn assistant_returned(
    tx: &mut Tx<'_>,
    workers: &[Worker],
    channel_id: ChannelId,
    record: &CoordinatorFallback,
) -> Result<()> {
    let mut channel = conversation::channel_or_404(tx.db(), channel_id)?;
    channel.coordinator_fallback = None;
    tx.save_channel(&channel)?;
    let (text, notice) = returned_notice(workers, &record.from, &record.to, record.cause);
    post_fallback_notice(
        tx,
        channel_id,
        None,
        None,
        "fallback_returned",
        &text,
        &notice,
    )?;
    Ok(())
}

/// Keeps the record of a channel in step with a new coordinator session. A
/// session that starts on a fallback sets the record and says so. A session
/// that starts on the first choice, while the record is there, ends the
/// record: with a notice when the assistant goes back, and silently when the
/// user chose another model meanwhile.
pub(crate) fn note_start(
    tx: &mut Tx<'_>,
    channel_id: ChannelId,
    workers: &[Worker],
    first: Option<&ModelSelection>,
    moved: Option<&Moved>,
) -> Result<()> {
    let mut channel = conversation::channel_or_404(tx.db(), channel_id)?;
    match (moved, channel.coordinator_fallback.take()) {
        (Some(moved), existing) => {
            let same = existing.as_ref().is_some_and(|e| {
                e.from == moved.first
                    && e.to.account_label == moved.chosen.selection.account_label
                    && e.to.model == moved.chosen.selection.model
            });
            if !same {
                assistant_moved(tx, workers, channel_id, moved)?;
            }
        }
        (None, Some(existing)) => {
            if first.is_some_and(|f| *f == existing.from) {
                assistant_returned(tx, workers, channel_id, &existing)?;
            } else {
                tx.save_channel(&channel)?;
            }
        }
        (None, None) => {}
    }
    Ok(())
}

/// The result of an attempt to send the assistant back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Return {
    /// The channel does not run on a fallback.
    NotOnFallback,
    /// The first choice cannot answer yet.
    NotYet,
    /// A turn runs. The assistant goes back when the hub tries again.
    Busy,
    /// The assistant went back.
    Returned,
}

/// Sends the assistant of a channel back to its first choice when that can
/// answer and no turn runs. The next turn starts a new session there.
pub(crate) fn return_in_tx(
    hub: &Hub,
    tx: &mut Tx<'_>,
    channel_id: ChannelId,
    fx: &mut Effects,
) -> Result<Return> {
    let channel = conversation::channel_or_404(tx.db(), channel_id)?;
    let Some(record) = channel.coordinator_fallback else {
        return Ok(Return::NotOnFallback);
    };
    let workers = machine::workers(tx.db())?;
    let rows = machine::account_holds(tx.db(), tx.now())?;
    let holds: Holds = holds_by_label(&workers, &rows);
    let path = preferences::preferences(tx.db())?.fallback_path;
    let walk = Walk {
        path: &path,
        workers: &workers,
        holds: &holds,
        mode: COORDINATOR_APPROVAL_MODE,
    };
    if !walk.first_is_usable(&record.from) {
        return Ok(Return::NotYet);
    }
    if coord_store::coordinator(tx.db(), channel_id)?.turn_open {
        return Ok(Return::Busy);
    }
    retire_session_tx(
        hub,
        tx,
        channel_id,
        None,
        "The assistant went back to its first choice.",
        fx,
    )?;
    assistant_returned(tx, &workers, channel_id, &record)?;
    Ok(Return::Returned)
}

impl Hub {
    /// Sends every assistant that runs on a fallback back to its first
    /// choice when the limit or the outage ended and the assistant is idle.
    /// The maintenance loop calls it.
    pub(crate) async fn return_fallbacks(&self) -> Result<()> {
        let ids = self
            .store
            .read(|conn| {
                Ok(conversation::channels(conn)?
                    .into_iter()
                    .filter(|c| c.coordinator_fallback.is_some())
                    .map(|c| c.id)
                    .collect::<Vec<_>>())
            })
            .await?;
        for channel_id in ids {
            let hub = self.clone();
            let fx = self
                .store
                .write(move |tx| {
                    let mut fx = Effects::default();
                    return_in_tx(&hub, tx, channel_id, &mut fx)?;
                    Ok(fx)
                })
                .await?;
            self.run_effects(fx);
        }
        Ok(())
    }

    /// `channel.go_back`: "Go back to <model>" in the model chip.
    pub(crate) async fn channel_go_back(
        &self,
        _caller: &Caller,
        request: ChannelGoBack,
    ) -> Result<ChannelResult> {
        let hub = self.clone();
        let channel_id = request.channel_id;
        let (result, fx, channel) = self
            .store
            .write(move |tx| {
                let mut fx = Effects::default();
                let result = return_in_tx(&hub, tx, channel_id, &mut fx)?;
                let channel = conversation::channel_or_404(tx.db(), channel_id)?;
                Ok((result, fx, channel))
            })
            .await?;
        self.run_effects(fx);
        match result {
            Return::Returned => Ok(ChannelResult { channel }),
            Return::NotOnFallback => Err(HubError::conflict(
                "The assistant of this channel is not on a fallback. Nothing to go back to.",
            )),
            Return::NotYet => Err(HubError::conflict(
                "The first choice cannot answer yet. Try again after its limit resets.",
            )),
            Return::Busy => Err(HubError::conflict(
                "The assistant is answering now. It goes back by itself when it has finished.",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use consort_proto::ids::{AccountId, WorkerId};
    use consort_proto::model::{
        AuthState, Capabilities, HarnessAccount, Inventory, ModelInfo, Os, Worker, WorkerLoad,
        WorkerPolicy, WorkerState,
    };

    use super::*;
    use crate::hub::test_hub;

    fn account(label: &str) -> HarnessAccount {
        HarnessAccount {
            blocked_reason: None,
            account_id: AccountId::new(),
            harness: "claude".to_owned(),
            label: label.to_owned(),
            harness_version: None,
            auth_state: AuthState::SignedIn,
            auth_detail: None,
            models: vec![ModelInfo {
                id: "opus".to_owned(),
                name: "Opus 5.5".to_owned(),
                options: Vec::new(),
            }],
            capabilities: Capabilities::default(),
            enabled: true,
            profile: true,
            email: None,
            plan: None,
            secret_names: Vec::new(),
            sign_in_methods: Vec::new(),
        }
    }

    fn worker(accounts: Vec<HarnessAccount>) -> Worker {
        Worker {
            kind: Default::default(),
            id: WorkerId::new(),
            name: "hub".to_owned(),
            hostname: "hub.local".to_owned(),
            os: Os::Linux,
            arch: "x86_64".to_owned(),
            public_key: consort_proto::auth::PublicKey::from_bytes([1; 32]),
            version: "0.1.0".to_owned(),
            protocol_version: 1,
            tags: Vec::new(),
            paused: false,
            state: WorkerState::Online,
            last_seen_at: None,
            inventory: Some(Inventory {
                boundary: None,
                hash: String::new(),
                hostname: "hub.local".to_owned(),
                os: Os::Linux,
                arch: "x86_64".to_owned(),
                tags: Vec::new(),
                accounts,
                local_mcp_servers: Vec::new(),
                checkouts: Vec::new(),
                policy: WorkerPolicy::default(),
                load: WorkerLoad::default(),
                tools: Vec::new(),
                update: None,
            }),
            paired_at: Timestamp::from_millis(0),
            revoked_at: None,
        }
    }

    fn sel(label: &str) -> ModelSelection {
        ModelSelection {
            account_label: label.to_owned(),
            model: "opus".to_owned(),
            options: Default::default(),
        }
    }

    /// A hub whose Tutti assistant runs on a fallback: Personal is the first
    /// choice, Work is where the assistant is. `hold_ms` is the hold on
    /// Personal from now. Returns the ID of Personal.
    async fn on_a_fallback(hub: &Hub, hold_ms: i64) -> (ChannelId, AccountId) {
        let personal = account("Personal");
        let personal_id = personal.account_id;
        let work = worker(vec![personal, account("Work")]);
        let tutti = hub.store.read(conversation::tutti_channel).await.unwrap();
        let channel_id = tutti.id;
        hub.store
            .write(move |tx| {
                tx.save_worker(&work)?;
                tx.record_account_limit(
                    personal_id,
                    tx.now().plus_millis(hold_ms),
                    FallbackCause::UsageLimit,
                    true,
                )?;
                let mut channel = conversation::channel_or_404(tx.db(), channel_id)?;
                channel.coordinator_selection = Some(sel("Personal"));
                channel.coordinator_fallback = Some(Box::new(CoordinatorFallback {
                    from: sel("Personal"),
                    to: sel("Work"),
                    cause: FallbackCause::UsageLimit,
                    returns_at: Some(tx.now().plus_millis(hold_ms)),
                    moved_at: tx.now(),
                }));
                tx.save_channel(&channel)
            })
            .await
            .unwrap();
        (channel_id, personal_id)
    }

    async fn attempt(hub: &Hub, channel_id: ChannelId) -> Return {
        let hub2 = hub.clone();
        hub.store
            .write(move |tx| {
                let mut fx = Effects::default();
                return_in_tx(&hub2, tx, channel_id, &mut fx)
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_assistant_stays_while_the_first_choice_is_held() {
        let (hub, _dir) = test_hub().await;
        let (channel_id, _) = on_a_fallback(&hub, 60_000).await;
        assert_eq!(attempt(&hub, channel_id).await, Return::NotYet);
        let channel = hub
            .store
            .read(move |c| conversation::channel_or_404(c, channel_id))
            .await
            .unwrap();
        assert!(channel.coordinator_fallback.is_some());
    }

    #[tokio::test]
    async fn the_assistant_goes_back_when_the_hold_ends_and_a_turn_is_not_open() {
        let (hub, _dir) = test_hub().await;
        let (channel_id, personal_id) = on_a_fallback(&hub, 60_000).await;
        // The hold ends: the limit reset, or the user cleared it.
        hub.store
            .write(move |tx| tx.clear_account_limit(personal_id))
            .await
            .unwrap();
        // A turn runs: the assistant is not cut.
        hub.store
            .write(move |tx| {
                let mut row = coord_store::coordinator(tx.db(), channel_id)?;
                row.turn_open = true;
                tx.save_coordinator(&row)
            })
            .await
            .unwrap();
        assert_eq!(attempt(&hub, channel_id).await, Return::Busy);
        hub.store
            .write(move |tx| {
                let mut row = coord_store::coordinator(tx.db(), channel_id)?;
                row.turn_open = false;
                tx.save_coordinator(&row)
            })
            .await
            .unwrap();
        assert_eq!(attempt(&hub, channel_id).await, Return::Returned);
        assert_eq!(attempt(&hub, channel_id).await, Return::NotOnFallback);
        let (channel, notices) = hub
            .store
            .read(move |c| {
                Ok((
                    conversation::channel_or_404(c, channel_id)?,
                    conversation::messages_page(c, channel_id, None, None, 100, usize::MAX)?
                        .0
                        .into_iter()
                        .map(|m| m.body)
                        .collect::<Vec<_>>(),
                ))
            })
            .await
            .unwrap();
        assert!(channel.coordinator_fallback.is_none());
        let returned = notices
            .iter()
            .find(|n| n.contains("fallback_returned"))
            .expect("the notice that the assistant went back");
        assert!(
            returned.contains("The limit of Personal has reset"),
            "{returned}"
        );
        assert!(
            returned.contains("Opus 5.5"),
            "the name of the model: {returned}"
        );
    }
}
