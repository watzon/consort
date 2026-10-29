//! Routes a decoded client request to its handler.
//!
//! `hello`, `sync.snapshot`, `sync.subscribe` and `auth.http_token` belong to
//! the connection (see `net::client_ws`). Every other request is a method on
//! [`Hub`] with the snake_case name of the request type, for example
//! `task.create` is `Hub::task_create`. The method takes the proto request
//! body and returns the proto response type.

use consort_proto::client_api::ClientRequest;
use consort_proto::error::ErrorBody;
use consort_proto::frame::MessageFamily;
use serde::Serialize;
use serde_json::Value;

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub};

pub(crate) fn to_value<T: Serialize>(value: &T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

pub(crate) async fn dispatch(hub: &Hub, caller: Caller, request: ClientRequest) -> Result<Value> {
    use ClientRequest as R;
    let caller = &caller;
    let request_type = request.type_name();
    match request {
        R::ChannelCreate(r) => to_value(&hub.channel_create(caller, r).await?),
        R::ChannelUpdate(r) => to_value(&hub.channel_update(caller, r).await?),
        R::ChannelArchive(r) => to_value(&hub.channel_archive(caller, r).await?),
        R::ChannelDelete(r) => to_value(&hub.channel_delete(caller, r).await?),
        R::ChannelMove(r) => to_value(&hub.channel_move(caller, r).await?),
        R::GroupCreate(r) => to_value(&hub.group_create(caller, r).await?),
        R::GroupUpdate(r) => to_value(&hub.group_update(caller, r).await?),
        R::GroupDelete(r) => to_value(&hub.group_delete(caller, r).await?),
        R::GroupReorder(r) => to_value(&hub.group_reorder(caller, r).await?),
        R::RepoCreate(r) => to_value(&hub.repo_create(caller, r).await?),
        R::MessageSend(r) => to_value(&hub.message_send(caller, r).await?),
        R::MessageList(r) => to_value(&hub.message_list(caller, r).await?),
        R::MessageEdit(r) => to_value(&hub.message_edit(caller, r).await?),
        R::MessageDelete(r) => to_value(&hub.message_delete(caller, r).await?),
        R::MessageReact(r) => to_value(&hub.message_react(caller, r).await?),
        R::MessageSearch(r) => to_value(&hub.message_search(caller, r).await?),
        R::ReadMark(r) => to_value(&hub.read_mark(caller, r).await?),
        R::TaskCreate(r) => to_value(&hub.task_create(caller, r).await?),
        R::TaskUpdate(r) => to_value(&hub.task_update(caller, r).await?),
        R::TaskResolve(r) => to_value(&hub.task_resolve(caller, r).await?),
        R::TaskReopen(r) => to_value(&hub.task_reopen(caller, r).await?),
        R::TaskRetry(r) => to_value(&hub.task_retry(caller, r).await?),
        R::TaskInterrupt(r) => to_value(&hub.task_interrupt(caller, r).await?),
        R::TaskArchive(r) => to_value(&hub.task_archive(caller, r).await?),
        R::TaskDelete(r) => to_value(&hub.task_delete(caller, r).await?),
        R::TaskGet(r) => to_value(&hub.task_get(caller, r).await?),
        R::TaskItems(r) => to_value(&hub.task_items(caller, r).await?),
        R::TaskItemDetail(r) => to_value(&hub.task_item_detail(caller, r).await?),
        R::ApprovalAnswer(r) => to_value(&hub.approval_answer(caller, r).await?),
        R::DecisionAnswer(r) => to_value(&hub.decision_answer(caller, r).await?),
        R::MemoryList(r) => to_value(&hub.memory_list(caller, r).await?),
        R::MemoryRead(r) => to_value(&hub.memory_read(caller, r).await?),
        R::MemoryWrite(r) => to_value(&hub.memory_write(caller, r).await?),
        R::MemoryDelete(r) => to_value(&hub.memory_delete(caller, r).await?),
        R::MemoryHistory(r) => to_value(&hub.memory_history(caller, r).await?),
        R::FilesList(r) => to_value(&hub.files_list(caller, r).await?),
        R::FilesPut(r) => to_value(&hub.files_put(caller, r).await?),
        R::FilesMove(r) => to_value(&hub.files_move(caller, r).await?),
        R::FilesDelete(r) => to_value(&hub.files_delete(caller, r).await?),
        R::RoutineCreate(r) => to_value(&hub.routine_create(caller, r).await?),
        R::RoutineUpdate(r) => to_value(&hub.routine_update(caller, r).await?),
        R::RoutineDelete(r) => to_value(&hub.routine_delete(caller, r).await?),
        R::RoutineRunNow(r) => to_value(&hub.routine_run_now(caller, r).await?),
        R::RoutineRuns(r) => to_value(&hub.routine_runs(caller, r).await?),
        R::WorkerList(r) => to_value(&hub.worker_list(caller, r).await?),
        R::WorkerPairingCode(r) => to_value(&hub.worker_pairing_code(caller, r).await?),
        R::WorkerRename(r) => to_value(&hub.worker_rename(caller, r).await?),
        R::WorkerSetTags(r) => to_value(&hub.worker_set_tags(caller, r).await?),
        R::WorkerSetPaused(r) => to_value(&hub.worker_set_paused(caller, r).await?),
        R::WorkerRevoke(r) => to_value(&hub.worker_revoke(caller, r).await?),
        R::WorkerRefreshInventory(r) => to_value(&hub.worker_refresh_inventory(caller, r).await?),
        R::ClientPairingCode(r) => to_value(&hub.client_pairing_code(caller, r).await?),
        R::ClientList(r) => to_value(&hub.client_list(caller, r).await?),
        R::ClientRevoke(r) => to_value(&hub.client_revoke(caller, r).await?),
        R::ClientPushRegister(r) => to_value(&hub.client_push_register(caller, r).await?),
        // Accounts, sign-in, managed tools and the ACP registry: the
        // contract is in place, the handlers come next (docs/ACCOUNTS-PLAN.md).
        R::AccountCreate(_)
        | R::AccountUpdate(_)
        | R::AccountRemove(_)
        | R::AccountSignOut(_)
        | R::SignInStart(_)
        | R::SignInSubmitCode(_)
        | R::SignInCancel(_)
        | R::WorkerToolsCheck(_)
        | R::WorkerToolInstall(_)
        | R::AcpRegistryList(_) => Err(HubError::Api(ErrorBody::not_built(request_type))),
        R::Hello(_) | R::SyncSnapshot(_) | R::SyncSubscribe(_) | R::AuthHttpToken(_) => Err(
            HubError::invalid("This request belongs to the connection and cannot be routed."),
        ),
    }
}
