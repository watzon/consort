//! The first conversation: the wake when the first account signs in, and the
//! state of the guide. The words of the guide are in `prompts/guide.md`. The
//! prompt of the Tutti coordinator holds them only while the guide runs.

use consort_proto::client_api::{GuideTryAgain, PreferencesResult, PreferencesSetUserName};
use consort_proto::frame::Empty;
use consort_proto::ids::{ApprovalId, RoutineId, WorkerId};
use consort_proto::model::{
    AddWorkerCard, Approval, ApprovalDetails, ApprovalStatus, AuthState, Channel, GuideState,
    HUB_CARD_ALLOW, MachineKind, Preferences, ProactiveKind, Routine, RoutineAction, RoutineKind,
    RoutineOrigin, RoutineSchedule, USER_NAME_MAX_CHARS, Worker, WorkerState,
};
use consort_proto::tools::{AddWorkerArgs, FinishGuideArgs, GuideOutcome, NoArgs, SetUserNameArgs};
use consort_proto::turn_input::WakeEvent;
use serde_json::{Value, json};

use super::{gate, routines};
use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub, ToolCtx};
use crate::ops::waiting::resume_if_nothing_waits;
use crate::prompts;
use crate::store::{
    Tx, conversation, coordinator as coord_store, machine, meta_get, preferences, proactivity,
    routines as routine_store, work,
};

/// The `meta` key that says the first greeting is over. See `schema_v19.sql`.
pub(crate) const FIRST_GREETING_KEY: &str = "first_greeting";
const FIRED: &str = "fired";
const SKIPPED: &str = "skipped";

/// The key of the follow-up with the one more offer of the guide.
const OFFER_KEY: &str = "guide-offer";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// What the assistant does in the follow-up turn. The prompt holds no guide
/// then.
const OFFER_REASON: &str = "The user said Not now to the guided setup yesterday. Offer it once more in one short message, with the suggestions Yes, Not now and No, I look around myself. If the user says yes, call start_guide. If not, stop. This is the last offer.";

/// The notice above the greeting (Paper T1).
fn signed_in_text(account: &str) -> String {
    format!("You signed in to {account}. The assistant can answer now.")
}

/// The name of the first account that can run a turn: the same rule as the
/// setup gate of a client.
fn signed_in_account(workers: &[Worker]) -> Option<String> {
    workers
        .iter()
        .filter(|w| w.revoked_at.is_none())
        .filter_map(|w| w.inventory.as_ref())
        .flat_map(|i| i.accounts.iter())
        .find(|a| a.enabled && a.auth_state == AuthState::SignedIn)
        .map(|a| a.label.clone())
}

/// The hub was used before: a message in Tutti, another channel, or a task.
fn has_history(tx: &Tx<'_>, tutti: &Channel) -> Result<bool> {
    let any = |sql: &str, id: &str| -> Result<bool> {
        Ok(tx.db().query_row(sql, [id], |row| row.get::<_, bool>(0))?)
    };
    let id = tutti.id.to_string();
    Ok(any(
        "SELECT EXISTS (SELECT 1 FROM messages WHERE channel_id = ?1)",
        &id,
    )? || any("SELECT EXISTS (SELECT 1 FROM channels WHERE id <> ?1)", &id)?
        || tx
            .db()
            .query_row("SELECT EXISTS (SELECT 1 FROM tasks)", [], |row| {
                row.get::<_, bool>(0)
            })?)
}

/// Queues the `first_sign_in` event of a hub, one time. Returns the channel
/// to kick. It runs inside the write of each inventory, so the flag and the
/// event commit together, and a restart cannot send a second event.
///
/// A hub with history sets the flag and sends nothing. When the level of Tutti
/// or Pause all holds the wake, the flag stays open, so the next inventory
/// tries again.
pub(crate) fn first_sign_in(tx: &mut Tx<'_>) -> Result<Option<consort_proto::ids::ChannelId>> {
    if meta_get(tx.db(), FIRST_GREETING_KEY)?.is_some() {
        return Ok(None);
    }
    let workers = machine::workers(tx.db())?;
    let Some(account) = signed_in_account(&workers) else {
        return Ok(None);
    };
    let tutti = conversation::tutti_channel(tx.db())?;
    if has_history(tx, &tutti)? {
        tx.meta_set(FIRST_GREETING_KEY, SKIPPED)?;
        return Ok(None);
    }
    let settings = proactivity::settings(tx.db())?;
    let now = tx.now();
    let decision = gate::decide(&gate::GateInput {
        kind: ProactiveKind::ChannelCreated,
        level: tutti.proactivity_level(),
        channel_name: &tutti.name,
        channel_kind: tutti.kind,
        settings: &settings,
        now,
        turns_today: 0,
    });
    if decision != gate::Gate::Run {
        return Ok(None);
    }
    tx.meta_set(FIRST_GREETING_KEY, FIRED)?;
    set_guide(tx, GuideState::Running)?;
    tx.post_notice(tutti.id, None, None, "signed_in", &signed_in_text(&account))?;
    queue_greeting(tx, &tutti, account)?;
    Ok(Some(tutti.id))
}

fn queue_greeting(tx: &mut Tx<'_>, tutti: &Channel, account: String) -> Result<()> {
    let mut wake = routines::pending_wake(tutti.id, ProactiveKind::ChannelCreated, tx.now(), None);
    tx.insert_wake(&mut wake)?;
    tx.queue_wake(
        tutti.id,
        &WakeEvent::FirstSignIn {
            account,
            log: wake.id,
        },
    )
}

fn set_guide(tx: &mut Tx<'_>, state: GuideState) -> Result<Preferences> {
    let mut prefs = preferences::preferences(tx.db())?;
    if prefs.guide != state {
        prefs.guide = state;
        tx.save_preferences(&prefs)?;
    }
    Ok(prefs)
}

/// Deletes the follow-up with the one more offer, when it waits.
fn cancel_offer(tx: &mut Tx<'_>, channel_id: consort_proto::ids::ChannelId) -> Result<()> {
    if let Some(row) = proactivity::follow_up_by_key(tx.db(), channel_id, OFFER_KEY)? {
        if let Some(routine) = routine_store::routine(tx.db(), row.routine_id)? {
            tx.delete_routine(&routine)?;
        }
        tx.delete_follow_up_row(row.routine_id)?;
    }
    Ok(())
}

/// The follow-up with the one more offer, a day from now.
fn schedule_offer(tx: &mut Tx<'_>, channel_id: consort_proto::ids::ChannelId) -> Result<()> {
    let now = tx.now();
    let at = now.plus_millis(DAY_MS);
    let routine = Routine {
        id: RoutineId::new(),
        channel_id,
        name: "Follow-up".to_owned(),
        schedule: RoutineSchedule::Once { at },
        prompt: OFFER_REASON.to_owned(),
        action: RoutineAction::CoordinatorTurn,
        enabled: true,
        next_run_at: Some(at),
        last_run_at: None,
        last_run_status: None,
        monid: None,
        kind: RoutineKind::FollowUp,
        origin: RoutineOrigin::Assistant,
        created_at: Some(now),
    };
    tx.save_routine(&routine)?;
    tx.save_follow_up(&proactivity::FollowUpRow {
        routine_id: routine.id,
        channel_id,
        key: Some(OFFER_KEY.to_owned()),
        set_at: now,
        set_in_proactive_turn: false,
    })
}

/// The name of the user: trimmed and not too long. Empty clears it.
fn clean_user_name(name: &str) -> Result<Option<String>> {
    let name = name.trim();
    if name.chars().any(char::is_control) || name.chars().count() > USER_NAME_MAX_CHARS {
        return Err(HubError::invalid(format!(
            "The name must fit on one line, with at most {USER_NAME_MAX_CHARS} characters."
        )));
    }
    Ok((!name.is_empty()).then(|| name.to_owned()))
}

impl Hub {
    /// Keeps the name of the user. The tool and the client API both end here.
    async fn store_user_name(&self, name: Option<String>) -> Result<Preferences> {
        self.store
            .write(move |tx| {
                let mut prefs = preferences::preferences(tx.db())?;
                if prefs.user_name != name {
                    prefs.user_name = name;
                    tx.save_preferences(&prefs)?;
                }
                Ok(prefs)
            })
            .await
    }

    pub(crate) async fn preferences_set_user_name(
        &self,
        _caller: &Caller,
        request: PreferencesSetUserName,
    ) -> Result<PreferencesResult> {
        let preferences = self
            .store_user_name(clean_user_name(&request.name)?)
            .await?;
        Ok(PreferencesResult { preferences })
    }

    pub(crate) async fn tool_set_user_name(
        &self,
        _ctx: &ToolCtx,
        args: SetUserNameArgs,
    ) -> Result<Value> {
        let Some(name) = clean_user_name(&args.name)? else {
            return Err(HubError::invalid("The name is empty."));
        };
        self.store_user_name(Some(name.clone())).await?;
        Ok(json!({ "name": name }))
    }

    pub(crate) async fn tool_start_guide(&self, ctx: &ToolCtx, _args: NoArgs) -> Result<Value> {
        let channel_id = ctx.channel.id;
        self.store
            .write(move |tx| {
                cancel_offer(tx, channel_id)?;
                set_guide(tx, GuideState::Running)?;
                Ok(())
            })
            .await?;
        Ok(json!({
            "status": "running",
            "guide": prompts::GUIDE.trim(),
            "note": "Skip the greeting. Go on with the first question that the user has not answered.",
        }))
    }

    pub(crate) async fn tool_finish_guide(
        &self,
        ctx: &ToolCtx,
        args: FinishGuideArgs,
    ) -> Result<Value> {
        let channel_id = ctx.channel.id;
        let summary = self
            .store
            .write(move |tx| {
                let prefs = preferences::preferences(tx.db())?;
                cancel_offer(tx, channel_id)?;
                match args.outcome {
                    GuideOutcome::Closed => {
                        set_guide(tx, GuideState::Closed)?;
                        Ok("The guide is closed.")
                    }
                    GuideOutcome::Declined => {
                        set_guide(tx, GuideState::Stopped)?;
                        Ok("The guide has ended. No offer comes.")
                    }
                    GuideOutcome::Later if prefs.guide_offered_again => {
                        set_guide(tx, GuideState::Stopped)?;
                        Ok("The guide has ended. You made the one more offer already, so no offer comes.")
                    }
                    GuideOutcome::Later => {
                        schedule_offer(tx, channel_id)?;
                        let mut prefs = set_guide(tx, GuideState::Stopped)?;
                        prefs.guide_offered_again = true;
                        tx.save_preferences(&prefs)?;
                        Ok("The guide stops now. The hub wakes you tomorrow for one more offer.")
                    }
                }
            })
            .await?;
        Ok(json!({ "status": "ok", "summary": summary }))
    }

    /// `guide.try_again`: the first turn failed before the assistant spoke.
    /// The hub queues the event again only when no turn runs and no event
    /// waits, so a second click starts no second greeting.
    pub(crate) async fn guide_try_again(
        &self,
        _caller: &Caller,
        _request: GuideTryAgain,
    ) -> Result<Empty> {
        let kick = self
            .store
            .write(|tx| {
                let prefs = preferences::preferences(tx.db())?;
                if prefs.guide != GuideState::Running {
                    return Ok(None);
                }
                let tutti = conversation::tutti_channel(tx.db())?;
                let spoke: bool = tx.db().query_row(
                    "SELECT EXISTS (SELECT 1 FROM messages WHERE channel_id = ?1
                     AND json_extract(data, '$.author') <> 'system')",
                    [tutti.id.to_string()],
                    |row| row.get(0),
                )?;
                let row = coord_store::coordinator(tx.db(), tutti.id)?;
                let waiting = !coord_store::pending_wakes(tx.db(), tutti.id)?.is_empty();
                if spoke || row.turn_open || waiting {
                    return Ok(None);
                }
                let workers = machine::workers(tx.db())?;
                let Some(account) = signed_in_account(&workers) else {
                    return Ok(None);
                };
                queue_greeting(tx, &tutti, account)?;
                Ok(Some(tutti.id))
            })
            .await?;
        if let Some(channel) = kick {
            self.kick_coordinator(channel);
        }
        Ok(Empty {})
    }

    pub(crate) async fn tool_add_worker(
        &self,
        ctx: &ToolCtx,
        args: AddWorkerArgs,
    ) -> Result<Value> {
        let id = ApprovalId::new();
        let summary = if args.this_computer {
            "Add this computer as a worker"
        } else {
            "Add a worker"
        };
        self.post_hub_card(
            ctx,
            id,
            summary.to_owned(),
            ApprovalDetails::AddWorker(Box::new(AddWorkerCard {
                this_computer: args.this_computer,
                worker_id: None,
                worker_name: None,
            })),
        )
        .await?;
        Ok(json!({
            "status": "waiting_for_user",
            "card": id,
            "summary": "The app shows the command, and you never see the code. End your turn. A card_answered event says when a worker is online.",
        }))
    }

    /// Closes the waiting worker cards when an execution worker comes online,
    /// and tells the session that posted each card.
    pub(crate) async fn close_worker_cards(&self, worker_id: WorkerId) -> Result<()> {
        let closed = self
            .store
            .write(move |tx| {
                let worker = machine::worker_or_404(tx.db(), worker_id)?;
                if worker.kind != MachineKind::Execution || worker.state != WorkerState::Online {
                    return Ok((String::new(), Vec::new()));
                }
                let mut closed: Vec<Approval> = Vec::new();
                for mut approval in work::pending_approvals(tx.db(), None)? {
                    let ApprovalDetails::AddWorker(card) = &mut approval.details else {
                        continue;
                    };
                    card.worker_id = Some(worker.id);
                    card.worker_name = Some(worker.name.clone());
                    approval.status = ApprovalStatus::Allowed;
                    approval.answer_option_id = Some(HUB_CARD_ALLOW.to_owned());
                    approval.answered_at = Some(tx.now());
                    tx.save_approval(&approval)?;
                    resume_if_nothing_waits(tx, approval.task_id)?;
                    closed.push(approval);
                }
                Ok((worker.name, closed))
            })
            .await?;
        let (name, cards) = closed;
        for approval in cards {
            self.send_card_answered(&approval, format!("The worker {name} is online."))
                .await?;
        }
        Ok(())
    }
}
