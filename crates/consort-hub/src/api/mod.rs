//! Routes a decoded client request to its handler.
//!
//! `hello`, `sync.snapshot`, `sync.subscribe` and `auth.http_token` belong to
//! the connection (see `net::client_ws`). Every other request is a method on
//! [`Hub`] with the snake_case name of the request type, for example
//! `task.create` is `Hub::task_create`. The method takes the proto request
//! body and returns the proto response type.

use consort_proto::client_api::ClientRequest;
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
        R::MemoryRestore(r) => to_value(&hub.memory_restore(caller, r).await?),
        R::MemoryMove(r) => to_value(&hub.memory_move(caller, r).await?),
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
        R::AccountCreate(r) => to_value(&hub.account_create(caller, r).await?),
        R::AccountUpdate(r) => to_value(&hub.account_update(caller, r).await?),
        R::AccountRemove(r) => to_value(&hub.account_remove(caller, r).await?),
        R::AccountSignOut(r) => to_value(&hub.account_sign_out(caller, r).await?),
        R::SignInStart(r) => to_value(&hub.sign_in_start(caller, r).await?),
        R::SignInSubmitCode(r) => to_value(&hub.sign_in_submit_code(caller, r).await?),
        R::SignInCancel(r) => to_value(&hub.sign_in_cancel(caller, r).await?),
        R::WorkerToolsCheck(r) => to_value(&hub.worker_tools_check(caller, r).await?),
        R::WorkerToolInstall(r) => to_value(&hub.worker_tool_install(caller, r).await?),
        R::AcpRegistryList(r) => to_value(&hub.acp_registry_list(caller, r).await?),
        R::PreferencesSetFavoriteModel(r) => {
            to_value(&hub.preferences_set_favorite_model(caller, r).await?)
        }
        R::ComposioSetKey(r) => to_value(&hub.composio_set_key(caller, r).await?),
        R::ComposioRemoveKey(r) => to_value(&hub.composio_remove_key(caller, r).await?),
        R::ComposioGetStatus(r) => to_value(&hub.composio_get_status(caller, r).await?),
        R::ConnectorCatalogSearch(r) => to_value(&hub.connector_catalog_search(caller, r).await?),
        R::ConnectorCatalogCategories(r) => {
            to_value(&hub.connector_catalog_categories(caller, r).await?)
        }
        R::ConnectorCatalogService(r) => to_value(&hub.connector_catalog_service(caller, r).await?),
        R::ConnectionConnect(r) => to_value(&hub.connection_connect(caller, r).await?),
        R::ConnectionUpdate(r) => to_value(&hub.connection_update(caller, r).await?),
        R::ConnectionDisconnect(r) => to_value(&hub.connection_disconnect(caller, r).await?),
        R::ConnectionReconnect(r) => to_value(&hub.connection_reconnect(caller, r).await?),
        R::ConnectionCalls(r) => to_value(&hub.connection_calls(caller, r).await?),
        R::ChannelSetConnectionEnabled(r) => {
            to_value(&hub.channel_set_connection_enabled(caller, r).await?)
        }
        R::ChannelSetSkillEnabled(r) => to_value(&hub.channel_set_skill_enabled(caller, r).await?),
        R::SkillUpdate(r) => to_value(&hub.skill_update(caller, r).await?),
        R::SkillRemove(r) => to_value(&hub.skill_remove(caller, r).await?),
        R::SkillFiles(r) => to_value(&hub.skill_files(caller, r).await?),
        R::SkillReadFile(r) => to_value(&hub.skill_read_file(caller, r).await?),
        R::SkillDirectorySearch(r) => to_value(&hub.skill_directory_search(caller, r).await?),
        R::SkillDirectorySources(r) => to_value(&hub.skill_directory_sources(caller, r).await?),
        R::SkillDirectoryAddSource(r) => {
            to_value(&hub.skill_directory_add_source(caller, r).await?)
        }
        R::SkillDirectoryRemoveSource(r) => {
            to_value(&hub.skill_directory_remove_source(caller, r).await?)
        }
        R::SkillPreview(r) => to_value(&hub.skill_preview(caller, r).await?),
        R::SkillInstall(r) => to_value(&hub.skill_install(caller, r).await?),
        R::Hello(_) | R::SyncSnapshot(_) | R::SyncSubscribe(_) | R::AuthHttpToken(_) => Err(
            HubError::invalid("This request belongs to the connection and cannot be routed."),
        ),
    }
}
