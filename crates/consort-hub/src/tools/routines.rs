//! Hub tools for routines. Owned by agent B.
//!
//! A session sees and changes only the routines of its own channel. The
//! rules for a change live in `ops/routines.rs` and are the same as for a
//! client.

use consort_proto::client_api::RoutineUpdate;
use consort_proto::model::RoutineAction;
use consort_proto::tools::{
    NoArgs, OkResult, RoutineCreateArgs, RoutineDeleteArgs, RoutineListResult, RoutineResult,
    RoutineUpdateArgs,
};
use serde_json::Value;

use super::tasks::limit_to_channel;
use crate::error::Result;
use crate::hub::{Hub, ToolCtx};
use crate::store::{conversation, routines};

impl Hub {
    /// Lowers the placement of a `start_task` action to what the channel
    /// allows for the assistant, as `start_task` does. The task of the
    /// routine takes the approval mode of the channel when it runs.
    async fn limit_routine_action(
        &self,
        ctx: &ToolCtx,
        action: &mut RoutineAction,
    ) -> Result<Vec<String>> {
        let RoutineAction::StartTask { placement, .. } = action else {
            return Ok(Vec::new());
        };
        let channel_id = ctx.channel.id;
        let channel = self
            .store
            .read(move |conn| conversation::channel_or_404(conn, channel_id))
            .await?;
        let mut no_mode = None;
        Ok(limit_to_channel(&channel, placement, &mut no_mode).for_assistant)
    }
}

impl Hub {
    pub(crate) async fn tool_routine_list(&self, ctx: &ToolCtx, _args: NoArgs) -> Result<Value> {
        let channel_id = ctx.channel.id;
        let routines = self
            .store
            .read(move |conn| routines::routines_in_channel(conn, channel_id))
            .await?;
        Ok(serde_json::to_value(RoutineListResult { routines })?)
    }

    pub(crate) async fn tool_routine_create(
        &self,
        ctx: &ToolCtx,
        args: RoutineCreateArgs,
    ) -> Result<Value> {
        let mut action = args.action;
        let notes = self.limit_routine_action(ctx, &mut action).await?;
        let routine = self
            .create_routine_in(
                ctx.channel.id,
                args.name,
                args.schedule,
                args.prompt,
                action,
                args.enabled.unwrap_or(true),
            )
            .await?;
        Ok(serde_json::to_value(RoutineResult { routine, notes })?)
    }

    pub(crate) async fn tool_routine_update(
        &self,
        ctx: &ToolCtx,
        args: RoutineUpdateArgs,
    ) -> Result<Value> {
        let mut action = args.action;
        let notes = match action.as_mut() {
            Some(action) => self.limit_routine_action(ctx, action).await?,
            None => Vec::new(),
        };
        let update = RoutineUpdate {
            channel_id: None,
            routine_id: args.routine_id,
            name: args.name,
            schedule: args.schedule,
            prompt: args.prompt,
            action,
            enabled: args.enabled,
            monid: None,
        };
        let routine = self.update_routine_in(Some(ctx.channel.id), update).await?;
        Ok(serde_json::to_value(RoutineResult { routine, notes })?)
    }

    pub(crate) async fn tool_routine_delete(
        &self,
        ctx: &ToolCtx,
        args: RoutineDeleteArgs,
    ) -> Result<Value> {
        self.delete_routine_in(Some(ctx.channel.id), args.routine_id)
            .await?;
        Ok(serde_json::to_value(OkResult::OK)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::test_hub;
    use crate::scheduler::testing::{channel, load, routine, save};
    use consort_proto::error::ErrorCode;
    use consort_proto::ids::{AccountId, RoutineId, SessionId, WorkerId};
    use consort_proto::model::{
        Channel, RoutineAction, RoutineSchedule, Session, SessionRole, SessionState, Usage,
    };
    use consort_proto::time::Timestamp;
    use consort_proto::tools::ToolAudience;
    use serde_json::json;

    fn ctx_for(channel: &Channel) -> ToolCtx {
        ToolCtx {
            session: Session {
                execution_worker_id: None,
                id: SessionId::new(),
                role: SessionRole::Coordinator,
                channel_id: channel.id,
                task_id: None,
                worker_id: WorkerId::new(),
                account_id: AccountId::new(),
                harness: "claude".to_owned(),
                model: "opus".to_owned(),
                options: Default::default(),
                workspace_path: "/tmp".to_owned(),
                native_ref: None,
                state: SessionState::Active,
                last_seq: 0,
                usage: Usage::default(),
                started_at: Timestamp::EPOCH,
                ended_at: None,
                end_reason: None,
            },
            channel: channel.clone(),
            task: None,
            audience: ToolAudience::ChannelCoordinator,
        }
    }

    fn create_args() -> RoutineCreateArgs {
        RoutineCreateArgs {
            name: "Morning brief".to_owned(),
            schedule: RoutineSchedule::Interval { seconds: 3_600 },
            prompt: "Summarize my day.".to_owned(),
            action: RoutineAction::CoordinatorTurn,
            enabled: None,
        }
    }

    fn routine_of(value: &Value) -> consort_proto::model::Routine {
        serde_json::from_value(value["routine"].clone()).unwrap()
    }

    #[tokio::test]
    async fn create_makes_an_enabled_routine_in_the_channel_of_the_session() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let ctx = ctx_for(&channel);

        let value = hub.tool_routine_create(&ctx, create_args()).await.unwrap();

        let created = routine_of(&value);
        assert_eq!(created.channel_id, channel.id);
        assert!(created.enabled);
        assert!(created.next_run_at.is_some());
        assert_eq!(load(&hub, created.id).await, created);
    }

    #[tokio::test]
    async fn create_with_enabled_false_makes_a_routine_that_is_off() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let ctx = ctx_for(&channel);
        let mut args = create_args();
        args.enabled = Some(false);

        let created = routine_of(&hub.tool_routine_create(&ctx, args).await.unwrap());

        assert!(!created.enabled);
        assert_eq!(created.next_run_at, None);
    }

    #[tokio::test]
    async fn create_follows_the_same_rules_as_the_client_api() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let ctx = ctx_for(&channel);
        let mut args = create_args();
        args.schedule = RoutineSchedule::Cron {
            expression: "0 8 * * *".to_owned(),
            time_zone: "Mars/Olympus".to_owned(),
        };

        let err = hub.tool_routine_create(&ctx, args).await.unwrap_err();

        assert_eq!(err.code(), ErrorCode::Invalid);
        assert!(err.to_string().contains("Mars/Olympus"), "{err}");
    }

    #[tokio::test]
    async fn list_returns_only_the_routines_of_the_channel() {
        let (hub, _dir) = test_hub().await;
        let mine = channel(&hub, false).await;
        let other = channel(&hub, false).await;
        let own = routine(
            mine.id,
            RoutineSchedule::Interval { seconds: 60 },
            RoutineAction::CoordinatorTurn,
        );
        let foreign = routine(
            other.id,
            RoutineSchedule::Interval { seconds: 60 },
            RoutineAction::CoordinatorTurn,
        );
        save(&hub, &own).await;
        save(&hub, &foreign).await;

        let value = hub
            .tool_routine_list(&ctx_for(&mine), NoArgs {})
            .await
            .unwrap();

        let listed: Vec<consort_proto::model::Routine> =
            serde_json::from_value(value["routines"].clone()).unwrap();
        assert_eq!(listed, vec![own]);
    }

    #[tokio::test]
    async fn update_changes_a_routine_of_the_channel() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let ctx = ctx_for(&channel);
        let created = routine_of(&hub.tool_routine_create(&ctx, create_args()).await.unwrap());

        let value = hub
            .tool_routine_update(
                &ctx,
                RoutineUpdateArgs {
                    routine_id: created.id,
                    name: Some("Evening brief".to_owned()),
                    schedule: None,
                    prompt: None,
                    action: None,
                    enabled: Some(false),
                },
            )
            .await
            .unwrap();

        let updated = routine_of(&value);
        assert_eq!(updated.name, "Evening brief");
        assert!(!updated.enabled);
        assert_eq!(updated.next_run_at, None);
        assert_eq!(updated.prompt, created.prompt);
        assert_eq!(load(&hub, created.id).await, updated);
    }

    #[tokio::test]
    async fn delete_removes_a_routine_of_the_channel() {
        let (hub, _dir) = test_hub().await;
        let channel = channel(&hub, false).await;
        let ctx = ctx_for(&channel);
        let created = routine_of(&hub.tool_routine_create(&ctx, create_args()).await.unwrap());

        let value = hub
            .tool_routine_delete(
                &ctx,
                RoutineDeleteArgs {
                    routine_id: created.id,
                },
            )
            .await
            .unwrap();

        assert_eq!(value, json!({"ok": true}));
        let listed = hub.tool_routine_list(&ctx, NoArgs {}).await.unwrap();
        assert_eq!(listed, json!({"routines": []}));
    }

    #[tokio::test]
    async fn a_session_cannot_reach_the_routines_of_another_channel() {
        let (hub, _dir) = test_hub().await;
        let mine = channel(&hub, false).await;
        let other = channel(&hub, false).await;
        let foreign = routine(
            other.id,
            RoutineSchedule::Interval { seconds: 60 },
            RoutineAction::CoordinatorTurn,
        );
        save(&hub, &foreign).await;
        let ctx = ctx_for(&mine);

        let err = hub
            .tool_routine_update(
                &ctx,
                RoutineUpdateArgs {
                    routine_id: foreign.id,
                    name: Some("Hijacked".to_owned()),
                    schedule: None,
                    prompt: None,
                    action: None,
                    enabled: None,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::NotFound);

        let err = hub
            .tool_routine_delete(
                &ctx,
                RoutineDeleteArgs {
                    routine_id: foreign.id,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::NotFound);

        // A routine that does not exist gives the same answer.
        let err = hub
            .tool_routine_delete(
                &ctx,
                RoutineDeleteArgs {
                    routine_id: RoutineId::new(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::NotFound);

        assert_eq!(load(&hub, foreign.id).await, foreign);
    }
}
