//! A user message that arrives while an outgoing draft card waits.
//!
//! The text of the user may change the draft ("make it shorter"), so the
//! hub does not leave the card open. It closes every waiting outgoing card
//! of the session as `replaced`, and it gives the text to the assistant as
//! the result of the tool call that waited. The new call asks again. The hub
//! never sends anything on its own.
//!
//! Only outgoing cards behave this way (`ConnectorCard::outgoing`), and
//! only while a tool call still waits for the card. A card without a
//! waiting call stays open: a later answer runs it as before. A call is not
//! waiting when its session ended or its turn closed (an interrupt), even
//! when the waiter is still there: the message then takes the normal path
//! and reaches the session.

use consort_proto::ids::{ApprovalId, ChannelId, TaskId};
use consort_proto::model::{Approval, ApprovalDetails, ApprovalStatus, BlobRef, ConnectorCard};
use rusqlite::Connection;
use serde_json::Value;

use crate::error::Result;
use crate::hub::Hub;
use crate::store::coordinator as coord_store;
use crate::store::{Tx, work};

/// Whose cards a message replaces.
#[derive(Debug, Clone, Copy)]
pub(crate) enum DraftScope {
    /// The cards of a task session.
    Task(TaskId),
    /// The cards of the coordinator of a channel (cards without a task).
    Coordinator(ChannelId),
}

/// Closes the waiting outgoing cards of `scope` as replaced and keeps the
/// text on them. Returns their IDs. The caller must call [`signal_replaced`]
/// after the commit, so the waiting calls see the saved cards.
///
/// Only the text reaches the card. A message with attachments replaces
/// nothing: the files would be lost, so the message takes the normal path.
pub(crate) fn replace_waiting_drafts(
    hub: &Hub,
    tx: &mut Tx<'_>,
    scope: DraftScope,
    text: &str,
    attachments: &[BlobRef],
) -> Result<Vec<ApprovalId>> {
    if !attachments.is_empty() {
        return Ok(Vec::new());
    }
    let pending = match scope {
        DraftScope::Task(task) => work::pending_approvals_of_task(tx.db(), task)?,
        DraftScope::Coordinator(channel) => work::pending_approvals(tx.db(), Some(channel))?
            .into_iter()
            .filter(|a| a.task_id.is_none())
            .collect(),
    };
    let mut replaced = Vec::new();
    for mut approval in pending {
        if !is_waiting_draft(hub, tx.db(), scope, &approval)? {
            continue;
        }
        let ApprovalDetails::Connector(card) = &mut approval.details else {
            continue;
        };
        card.replaced_by = Some(text.to_owned());
        approval.status = ApprovalStatus::Replaced;
        approval.answered_at = Some(tx.now());
        tx.save_approval(&approval)?;
        replaced.push(approval.id);
    }
    if let (DraftScope::Task(task), false) = (scope, replaced.is_empty()) {
        crate::ops::waiting::resume_if_nothing_waits(tx, Some(task))?;
    }
    Ok(replaced)
}

/// Whether a live tool call waits for this outgoing card. A waiter alone is
/// not enough: it outlives a harness call that an interrupt or a lost
/// session killed, for up to the approval wait. Replacing such a card would
/// hand the text of the user to a dead call and lose it. So the session of
/// the card must be live, with an open turn, and for a coordinator it must
/// be the session that the channel uses now.
fn is_waiting_draft(
    hub: &Hub,
    conn: &Connection,
    scope: DraftScope,
    approval: &Approval,
) -> Result<bool> {
    if !(approval.is_hub_card()
        && matches!(&approval.details, ApprovalDetails::Connector(card) if card.outgoing.is_some())
        && hub.connectors.waiter_exists(approval.id))
    {
        return Ok(false);
    }
    let Some(session) = work::session(conn, approval.session_id)? else {
        return Ok(false);
    };
    if session.state.is_final() {
        return Ok(false);
    }
    Ok(match scope {
        DraftScope::Task(_) => work::session_runtime(conn, session.id)?.turn_open,
        DraftScope::Coordinator(channel) => {
            let row = coord_store::coordinator(conn, channel)?;
            row.turn_open && row.session_id == Some(approval.session_id)
        }
    })
}

impl Hub {
    /// Wakes the tool calls that wait for cards that a message replaced.
    pub(crate) fn signal_replaced(&self, ids: &[ApprovalId]) {
        for id in ids {
            if let Some(waiter) = self.connectors.take_waiter(*id) {
                let _ = waiter.send(ApprovalStatus::Replaced);
            }
        }
    }
}

/// The result text for the tool call that waited. It holds the text of the
/// user, and the text that the user had saved on the card, if any.
pub(crate) fn replaced_message(card: &ConnectorCard) -> String {
    let sent = card.replaced_by.as_deref().unwrap_or_default();
    let mut text = format!(
        "The user sent this while your draft waited: {sent}. If it asks for a change, write the draft again and call the tool. If not, answer it and call the tool again with the same text."
    );
    if card.edited() {
        let saved = card
            .editable
            .iter()
            .filter_map(|key| {
                let value = card.arguments.get(key)?;
                let shown = match value {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                Some(format!("{key}: {shown}"))
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !saved.is_empty() {
            text.push_str(
                "\n\nThe user had edited the draft and saved this text. Keep these edits unless the message says otherwise:\n",
            );
            text.push_str(&saved);
        }
    }
    text
}
