//! Draft cards that `show_draft` posts, and `draft.edit`.
//!
//! The card is a notice with `kind: "draft"`. It does not wait and does not
//! stop the turn: the user edits it and sends the text themself. The edit
//! stays on the hub, so every client shows it. Copy needs no request.

use consort_proto::client_api::{DraftEdit, MessageResult};
use consort_proto::model::{Author, Message, MessageKind};
use consort_proto::tools::{
    SHOW_DRAFT_BODY_MAX_BYTES, SHOW_DRAFT_TITLE_MAX_CHARS, SHOW_DRAFT_TO_MAX_BYTES, ShowDraftArgs,
    ShowDraftResult,
};
use serde_json::{Map, Value, json};

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub, ToolCtx};
use crate::limits::check_text;
use crate::store::conversation::{self, NewMessage};

/// The `kind` of the notice body of a draft card.
pub(crate) const DRAFT_NOTICE_KIND: &str = "draft";

/// What the assistant reads after it posted a card.
const SHOWN: &str = "The card is in the conversation. The user edits it and sends the text themself. Nothing was sent, and you do not wait for the user.";

fn check_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > SHOW_DRAFT_TITLE_MAX_CHARS {
        return Err(HubError::invalid(format!(
            "The title must have 1 to {SHOW_DRAFT_TITLE_MAX_CHARS} characters."
        )));
    }
    Ok(title.to_owned())
}

fn check_body(body: &str) -> Result<()> {
    if body.trim().is_empty() {
        return Err(HubError::invalid("The draft is empty. Write the text."));
    }
    check_text("body", body, SHOW_DRAFT_BODY_MAX_BYTES)
}

/// The notice body of a draft card. `text` is the line that a client
/// without draft cards shows.
fn draft_body(title: &str, to: Option<&str>, body: &str, original: Option<&str>) -> String {
    let mut object = Map::new();
    object.insert("kind".to_owned(), json!(DRAFT_NOTICE_KIND));
    object.insert("text".to_owned(), json!(format!("{title}\n\n{body}")));
    object.insert("title".to_owned(), json!(title));
    if let Some(to) = to {
        object.insert("to".to_owned(), json!(to));
    }
    object.insert("body".to_owned(), json!(body));
    if let Some(original) = original {
        object.insert("original_body".to_owned(), json!(original));
    }
    Value::Object(object).to_string()
}

impl Hub {
    /// `show_draft`: posts a card for a text that the user sends themself.
    /// A task session posts it in the thread of its task, a coordinator in
    /// its channel.
    pub(crate) async fn tool_show_draft(
        &self,
        ctx: &ToolCtx,
        args: ShowDraftArgs,
    ) -> Result<Value> {
        let title = check_title(&args.title)?;
        check_body(&args.body)?;
        let to = args
            .to
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned);
        if let Some(to) = &to {
            check_text("to", to, SHOW_DRAFT_TO_MAX_BYTES)?;
        }
        let body = draft_body(&title, to.as_deref(), &args.body, None);
        let (channel_id, session_id) = (ctx.channel.id, ctx.session.id);
        let task = ctx.task.clone();
        let message = self
            .store
            .write(move |tx| {
                // A coordinator posts under the rules of its turn, as with
                // `post_message`: the research pass may not post, and a
                // proactive turn marks the card. A task session has no turn
                // of this kind.
                let posting = match &task {
                    None => Some(crate::proactivity::tools::posting(
                        tx,
                        session_id,
                        channel_id,
                        false,
                        Vec::new(),
                    )?),
                    Some(_) => None,
                };
                let mut new =
                    NewMessage::new(channel_id, Author::Assistant, MessageKind::Notice, body);
                new.author_session_id = Some(session_id);
                if let Some(task) = &task {
                    new.thread_root_id = Some(task.card_message_id);
                    new.author_task_id = Some(task.id);
                }
                let message = tx.insert_message(new)?;
                match posting {
                    Some(posting) => crate::tools::chat::mark_message(tx, message, posting),
                    None => Ok(message),
                }
            })
            .await?;
        Ok(serde_json::to_value(ShowDraftResult {
            message_id: message.id,
            message: SHOWN.to_owned(),
        })?)
    }

    /// `draft.edit`: saves the user's change to a draft card. The card keeps
    /// the text that the assistant wrote, in `original_body`.
    pub(crate) async fn draft_edit(
        &self,
        _caller: &Caller,
        request: DraftEdit,
    ) -> Result<MessageResult> {
        let message = self
            .store
            .write(move |tx| {
                let mut message = conversation::message_or_404(tx.db(), request.message_id)?;
                let object = draft_object(&message)?;
                let current_title = object_text(&object, "title");
                let current_body = object_text(&object, "body");
                let title = match request.title.as_deref() {
                    Some(title) => check_title(title)?,
                    None => current_title,
                };
                let body = match request.body {
                    Some(body) => {
                        check_body(&body)?;
                        body
                    }
                    None => current_body.clone(),
                };
                let original = object
                    .get("original_body")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| (body != current_body).then(|| current_body.clone()));
                let to = object.get("to").and_then(Value::as_str).map(str::to_owned);
                message.body = draft_body(&title, to.as_deref(), &body, original.as_deref());
                message.edited_at = Some(tx.now());
                tx.save_message(&message)?;
                Ok(message)
            })
            .await?;
        Ok(MessageResult { message })
    }
}

fn object_text(object: &Map<String, Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The JSON object of a draft card, or a refusal for any other message.
fn draft_object(message: &Message) -> Result<Map<String, Value>> {
    let not_a_draft = || HubError::invalid("This message is not a draft card.");
    if message.kind != MessageKind::Notice || message.deleted_at.is_some() {
        return Err(not_a_draft());
    }
    match serde_json::from_str::<Value>(&message.body) {
        Ok(Value::Object(object))
            if object.get("kind").and_then(Value::as_str) == Some(DRAFT_NOTICE_KIND) =>
        {
            Ok(object)
        }
        _ => Err(not_a_draft()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::hub::{test_hub, test_task_session};
    use crate::store::work;
    use consort_proto::model::SessionRole;
    use consort_proto::tools::ToolAudience;

    async fn task_ctx(hub: &Hub) -> ToolCtx {
        let (task, session) = test_task_session(hub).await;
        let channel = hub.store.read(conversation::tutti_channel).await.unwrap();
        ToolCtx {
            session,
            channel,
            task: Some(task),
            audience: ToolAudience::TaskSession,
        }
    }

    async fn message(hub: &Hub, id: consort_proto::ids::MessageId) -> Message {
        hub.store
            .read(move |conn| conversation::message_or_404(conn, id))
            .await
            .unwrap()
    }

    fn args(title: &str, to: Option<&str>, body: &str) -> ShowDraftArgs {
        ShowDraftArgs {
            title: title.to_owned(),
            to: to.map(str::to_owned),
            body: body.to_owned(),
        }
    }

    #[tokio::test]
    async fn show_draft_posts_a_card_in_the_task_thread_and_waits_for_nobody() {
        let (hub, _dir) = test_hub().await;
        let ctx = task_ctx(&hub).await;
        let value = hub
            .tool_show_draft(
                &ctx,
                args("Claim form", Some("Dr. Meyer's office"), "Dear Dr. Meyer,"),
            )
            .await
            .unwrap();
        let result: ShowDraftResult = serde_json::from_value(value).unwrap();
        let card = message(&hub, result.message_id).await;
        assert_eq!(card.kind, MessageKind::Notice);
        assert_eq!(card.author, Author::Assistant);
        assert_eq!(
            card.thread_root_id,
            ctx.task.as_ref().map(|t| t.card_message_id)
        );
        let body: Value = serde_json::from_str(&card.body).unwrap();
        assert_eq!(body["kind"], "draft");
        assert_eq!(body["title"], "Claim form");
        assert_eq!(body["to"], "Dr. Meyer's office");
        assert_eq!(body["body"], "Dear Dr. Meyer,");
        assert!(result.message.contains("Nothing was sent"));
        // The turn goes on: no approval waits.
        let pending = hub
            .store
            .read(|conn| work::pending_approvals(conn, None))
            .await
            .unwrap();
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn show_draft_in_a_coordinator_session_posts_in_the_channel() {
        let (hub, _dir) = test_hub().await;
        let mut ctx = task_ctx(&hub).await;
        ctx.task = None;
        ctx.session.role = SessionRole::Coordinator;
        ctx.audience = ToolAudience::TuttiCoordinator;
        let value = hub
            .tool_show_draft(&ctx, args("Letter", None, "Hello"))
            .await
            .unwrap();
        let result: ShowDraftResult = serde_json::from_value(value).unwrap();
        let card = message(&hub, result.message_id).await;
        assert_eq!(card.thread_root_id, None);
        let body: Value = serde_json::from_str(&card.body).unwrap();
        assert!(body.get("to").is_none());
    }

    /// Opens a turn of the coordinator in the session of `ctx`, with these
    /// events.
    async fn open_turn(
        hub: &Hub,
        ctx: &ToolCtx,
        events: Vec<consort_proto::turn_input::WakeEvent>,
    ) {
        let (channel_id, session_id) = (ctx.channel.id, ctx.session.id);
        hub.store
            .write(move |tx| {
                tx.save_coordinator(&crate::store::coordinator::CoordinatorRow {
                    channel_id,
                    session_id: Some(session_id),
                    turn_open: true,
                    turn_started_at: Some(tx.now()),
                    turn_events: events,
                })
            })
            .await
            .unwrap();
    }

    fn coordinator_ctx(mut ctx: ToolCtx) -> ToolCtx {
        ctx.task = None;
        ctx.session.role = SessionRole::Coordinator;
        ctx.audience = ToolAudience::TuttiCoordinator;
        ctx
    }

    #[tokio::test]
    async fn show_draft_in_a_proactive_turn_marks_the_card() {
        use consort_proto::model::ProactiveKind;
        let (hub, _dir) = test_hub().await;
        let ctx = coordinator_ctx(task_ctx(&hub).await);
        open_turn(
            &hub,
            &ctx,
            vec![consort_proto::turn_input::WakeEvent::FollowUp {
                routine: consort_proto::ids::RoutineId::new(),
                reason: "Check the quote.".to_owned(),
                set_at: consort_proto::time::Timestamp::from_millis(1),
                log: 1,
            }],
        )
        .await;
        let value = hub
            .tool_show_draft(&ctx, args("Letter", None, "Hello"))
            .await
            .unwrap();
        let id = serde_json::from_value::<ShowDraftResult>(value)
            .unwrap()
            .message_id;
        let card = message(&hub, id).await;
        let mark = card.proactive.expect("a proactive mark");
        assert_eq!(mark.kind, ProactiveKind::FollowUp);
    }

    #[tokio::test]
    async fn show_draft_is_refused_in_the_research_pass_and_posts_nothing() {
        let (hub, _dir) = test_hub().await;
        let ctx = coordinator_ctx(task_ctx(&hub).await);
        open_turn(
            &hub,
            &ctx,
            vec![consort_proto::turn_input::WakeEvent::Research {
                routine: consort_proto::ids::RoutineId::new(),
                log: 1,
            }],
        )
        .await;
        let error = hub
            .tool_show_draft(&ctx, args("Letter", None, "Hello"))
            .await
            .unwrap_err();
        assert!(error.to_error_body().message.contains("read-only"));
        let channel_id = ctx.channel.id;
        let (posted, _) = hub
            .store
            .read(move |conn| {
                conversation::messages_page(conn, channel_id, None, None, 50, 1 << 20)
            })
            .await
            .unwrap();
        assert!(
            posted.iter().all(|m| m.kind != MessageKind::Notice),
            "no card was posted"
        );
    }

    #[tokio::test]
    async fn show_draft_refuses_an_empty_or_too_long_text() {
        let (hub, _dir) = test_hub().await;
        let ctx = task_ctx(&hub).await;
        assert!(
            hub.tool_show_draft(&ctx, args("  ", None, "Body"))
                .await
                .is_err()
        );
        assert!(
            hub.tool_show_draft(&ctx, args("Title", None, " "))
                .await
                .is_err()
        );
        let long_title = "t".repeat(SHOW_DRAFT_TITLE_MAX_CHARS + 1);
        assert!(
            hub.tool_show_draft(&ctx, args(&long_title, None, "Body"))
                .await
                .is_err()
        );
        let long_body = "b".repeat(SHOW_DRAFT_BODY_MAX_BYTES + 1);
        assert!(
            hub.tool_show_draft(&ctx, args("Title", None, &long_body))
                .await
                .is_err()
        );
        let long_to = "n".repeat(SHOW_DRAFT_TO_MAX_BYTES + 1);
        assert!(
            hub.tool_show_draft(&ctx, args("Title", Some(&long_to), "Body"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn an_edit_is_saved_on_the_hub_and_keeps_the_text_the_assistant_wrote() {
        let (hub, _dir) = test_hub().await;
        let ctx = task_ctx(&hub).await;
        let value = hub
            .tool_show_draft(&ctx, args("Letter", Some("Anna"), "Hello Anna"))
            .await
            .unwrap();
        let id = serde_json::from_value::<ShowDraftResult>(value)
            .unwrap()
            .message_id;
        let edit = |body: &str| DraftEdit {
            message_id: id,
            title: None,
            body: Some(body.to_owned()),
        };
        hub.draft_edit(&Caller::Hub, edit("Hi Anna")).await.unwrap();
        hub.draft_edit(&Caller::Hub, edit("Hi Anna, thanks"))
            .await
            .unwrap();
        let card = message(&hub, id).await;
        assert!(card.edited_at.is_some());
        let body: Value = serde_json::from_str(&card.body).unwrap();
        assert_eq!(body["body"], "Hi Anna, thanks");
        assert_eq!(body["original_body"], "Hello Anna", "the first text stays");
        assert_eq!(body["to"], "Anna");
        assert_eq!(body["title"], "Letter");
    }

    #[tokio::test]
    async fn draft_edit_refuses_a_message_that_is_not_a_draft_card() {
        let (hub, _dir) = test_hub().await;
        let (task, _) = test_task_session(&hub).await;
        let err = hub
            .draft_edit(
                &Caller::Hub,
                DraftEdit {
                    message_id: task.card_message_id,
                    title: None,
                    body: Some("x".to_owned()),
                },
            )
            .await
            .unwrap_err();
        assert!(err.to_error_body().message.contains("not a draft card"));
    }
}
