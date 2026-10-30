//! The assistant's management tools (docs/CONNECTORS-SKILLS-PLAN.md,
//! "Assistant management tools").
//!
//! The hub enforces the scope and the risk rule, not the prompt. A change
//! that adds something new and narrow, or makes access smaller or approvals
//! stricter, runs at once and posts a short activity line. Any other change
//! posts a management approval card, and runs when the user allows it: the
//! hub parses the card's tool call again and runs it with `approved` set.
//! The assistant never handles a secret: a sign-in or a connection goes
//! through a card that the user finishes in the app or the browser.

use consort_proto::client_api::{
    AccountCreate, AccountUpdate, ChannelArchive, ChannelCreate, ChannelMove,
    ChannelSetConnectionEnabled, ChannelSetSkillEnabled, ChannelUpdate, GroupCreate, RepoCreate,
    WorkerSetTags, WorkerToolInstall,
};
use consort_proto::ids::{ApprovalId, ChannelId, GroupId, RepoId};
use consort_proto::model::{
    Approval, ApprovalDetails, Channel, ConnectServiceCard, Connection, ManagementCard,
    ModelSelection, SignInCard, SignInMethod, Skill, ToolId, UseIn, normalize_tags,
    validate_connection_name,
};
use consort_proto::tools::{
    AddAccountArgs, ArchiveChannelArgs, ConnectServiceArgs, CreateChannelArgs, DisconnectArgs,
    HubTool, HubToolCall, InstallToolArgs, ListConnectionsResult, ListInstalledSkillsResult,
    ManagementResult, RemoveSkillArgs, RequestAccessArgs, SearchConnectorCatalogArgs,
    SearchConnectorCatalogResult, SetAccountExtrasArgs, SetChannelConnectorArgs,
    SetChannelSkillArgs, SetChannelTagsArgs, SetDefaultModelArgs, SetWorkerTagsArgs,
    StartSignInArgs, ToolAudience, UpdateChannelArgs, UpdateConnectionArgs, UpdateSkillArgs,
    UpdateThisChannelArgs,
};
use serde::Serialize;
use serde_json::Value;

use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub, ToolCtx};
use crate::store::{connectors as con_store, conversation, machine, skills as skill_store};

/// The most services that `search_connector_catalog` returns.
const CATALOG_SEARCH_MAX: usize = 25;
/// The notice kind of an activity line.
const ACTIVITY_NOTICE: &str = "assistant_change";

/// "1 more channel" or "4 more channels".
fn more_channels(n: usize) -> String {
    if n == 1 {
        "1 more channel".to_owned()
    } else {
        format!("{n} more channels")
    }
}

/// How a "Use in" scope reads in a card.
fn describe_use_in(use_in: &UseIn, channels: &[Channel]) -> String {
    match use_in {
        UseIn::EveryChannel => "every channel".to_owned(),
        UseIn::Tags { tags } => format!("channels with the tags {}", tags.join(", ")),
        UseIn::Channels { channel_ids } => {
            let names: Vec<String> = channel_ids
                .iter()
                .map(|id| {
                    channels
                        .iter()
                        .find(|c| c.id == *id)
                        .map_or_else(|| id.to_string(), |c| c.name.clone())
                })
                .collect();
            format!("the channels {}", names.join(", "))
        }
    }
}

/// The wider scope that gives an item to one more channel. A tag scope
/// becomes the list of the channels that it gives the item to now, plus
/// this channel.
fn widen_to(use_in: &UseIn, channel: &Channel, channels: &[Channel]) -> UseIn {
    match use_in {
        UseIn::EveryChannel => UseIn::EveryChannel,
        UseIn::Channels { channel_ids } => {
            let mut ids = channel_ids.clone();
            if !ids.contains(&channel.id) {
                ids.push(channel.id);
            }
            UseIn::Channels { channel_ids: ids }
        }
        UseIn::Tags { .. } => {
            let mut ids: Vec<ChannelId> = channels
                .iter()
                .filter(|c| c.archived_at.is_none() && use_in.matches_channel(c))
                .map(|c| c.id)
                .collect();
            if !ids.contains(&channel.id) {
                ids.push(channel.id);
            }
            UseIn::Channels { channel_ids: ids }
        }
    }
}

impl Hub {
    /// Posts a management approval card and answers `waiting_for_user`.
    async fn ask_first<A: Serialize>(
        &self,
        ctx: &ToolCtx,
        tool: HubTool,
        args: &A,
        change: String,
        effect: String,
    ) -> Result<ManagementResult> {
        let id = ApprovalId::new();
        let card = ManagementCard {
            tool: tool.name().to_owned(),
            change: change.clone(),
            effect,
            arguments: serde_json::to_value(args)?,
        };
        self.post_hub_card(
            ctx,
            id,
            change.clone(),
            ApprovalDetails::Management(Box::new(card)),
        )
        .await?;
        Ok(ManagementResult::waiting_for_user(
            id,
            format!(
                "The user must allow this first: {change} The answer comes as a card_answered event."
            ),
        ))
    }

    /// Posts the short activity line of a change that ran at once.
    async fn activity(&self, ctx: &ToolCtx, text: String) -> Result<()> {
        let channel_id = ctx.channel.id;
        self.store
            .write(move |tx| {
                tx.post_notice(channel_id, None, None, ACTIVITY_NOTICE, &text)?;
                Ok(())
            })
            .await
    }

    /// Reports a change. One that ran at once gets an activity line.
    async fn applied(
        &self,
        ctx: &ToolCtx,
        approved: bool,
        summary: String,
    ) -> Result<ManagementResult> {
        if !approved {
            self.activity(ctx, summary.clone()).await?;
        }
        Ok(ManagementResult::applied(summary))
    }

    async fn all_channels(&self) -> Result<Vec<Channel>> {
        self.store.read(conversation::channels).await
    }

    async fn find_connection(&self, key: &str) -> Result<Connection> {
        let wanted = key.to_owned();
        self.store
            .read(move |conn| con_store::find_connection(conn, &wanted))
            .await?
            .ok_or_else(|| {
                HubError::not_found(format!(
                    "There is no connection {key}. Call list_connections to see the handles."
                ))
            })
    }

    async fn find_skill(&self, key: &str) -> Result<Skill> {
        let wanted = key.to_owned();
        self.store
            .read(move |conn| skill_store::find_skill(conn, &wanted))
            .await?
            .ok_or_else(|| {
                HubError::not_found(format!(
                    "There is no skill {key}. Call list_installed_skills to see the names."
                ))
            })
    }

    /// The group with this name, made when there is none.
    async fn group_named(&self, name: &str) -> Result<GroupId> {
        let wanted = name.trim().to_lowercase();
        let found = self
            .store
            .read(move |conn| {
                Ok(conversation::groups(conn)?
                    .into_iter()
                    .find(|g| g.name.trim().to_lowercase() == wanted))
            })
            .await?;
        match found {
            Some(group) => Ok(group.id),
            None => Ok(self
                .group_create(
                    &Caller::Hub,
                    GroupCreate {
                        name: name.trim().to_owned(),
                    },
                )
                .await?
                .group
                .id),
        }
    }

    async fn repo_ids(&self, urls: &[String]) -> Result<Vec<RepoId>> {
        let mut ids = Vec::new();
        for url in urls {
            let repo = self
                .repo_create(
                    &Caller::Hub,
                    RepoCreate {
                        url: url.clone(),
                        name: None,
                        default_branch: None,
                    },
                )
                .await?
                .repo;
            if !ids.contains(&repo.id) {
                ids.push(repo.id);
            }
        }
        Ok(ids)
    }

    /// The connectors and skills that a channel gains and loses with new
    /// tags, in words.
    async fn tag_effect(&self, channel: Option<&Channel>, tags: &[String]) -> Result<String> {
        let (connections, skills) = self
            .store
            .read(|conn| Ok((con_store::connections(conn)?, skill_store::skills(conn)?)))
            .await?;
        let id = channel.map_or_else(ChannelId::new, |c| c.id);
        let everything = channel.is_some_and(Channel::is_everything);
        let empty: Vec<String> = Vec::new();
        let old_tags = channel.map_or(&empty, |c| &c.tags);
        let mut gained = Vec::new();
        let mut lost = Vec::new();
        let names = connections
            .iter()
            .map(|c| (&c.use_in, c.name.clone()))
            .chain(
                skills
                    .iter()
                    .map(|s| (&s.use_in, format!("the skill {}", s.name))),
            );
        for (use_in, name) in names {
            let before = channel.is_some() && use_in.matches(id, everything, old_tags);
            let after = use_in.matches(id, everything, tags);
            if after && !before {
                gained.push(name);
            } else if before && !after {
                lost.push(name);
            }
        }
        let mut parts = Vec::new();
        if !gained.is_empty() {
            parts.push(format!("It gets {}.", gained.join(", ")));
        }
        if !lost.is_empty() {
            parts.push(format!("It loses {}.", lost.join(", ")));
        }
        parts.push(
            "Placement uses the tags too, so the work of the channel can move to other workers."
                .to_owned(),
        );
        Ok(parts.join(" "))
    }

    /// Runs one management tool. Read tools return their result; change
    /// tools return a [`ManagementResult`].
    pub(crate) async fn tool_management(&self, ctx: &ToolCtx, call: HubToolCall) -> Result<Value> {
        match call {
            HubToolCall::ListConnections(_) => self.tool_list_connections(ctx).await,
            HubToolCall::ListInstalledSkills(_) => self.tool_list_installed_skills(ctx).await,
            HubToolCall::SearchConnectorCatalog(a) => self.tool_search_connector_catalog(a).await,
            other => Ok(serde_json::to_value(self.manage(ctx, other, false).await?)?),
        }
    }

    /// Carries out a management card that the user allowed. Returns the
    /// text for the `card_answered` event.
    pub(crate) async fn approved_management(
        &self,
        approval: &Approval,
        card: &ManagementCard,
    ) -> Result<String> {
        let ctx = self.ctx_for_approval(approval).await?;
        let call = HubToolCall::parse_for(ctx.audience, &card.tool, card.arguments.clone())
            .map_err(|e| HubError::Api(e.to_error_body()))?;
        let result = self.manage(&ctx, call, true).await?;
        Ok(match result.status {
            consort_proto::tools::ChangeStatus::Refused => {
                format!("The change was not made: {}", result.summary)
            }
            _ => result.summary,
        })
    }

    /// The change tools. With `approved`, the user allowed the change on a
    /// card, so the hub makes it without asking again.
    async fn manage(
        &self,
        ctx: &ToolCtx,
        call: HubToolCall,
        approved: bool,
    ) -> Result<ManagementResult> {
        use HubToolCall as C;
        match call {
            C::CreateChannel(a) => self.m_create_channel(ctx, a, approved).await,
            C::UpdateChannel(a) => self.m_update_channel(ctx, a, approved).await,
            C::ArchiveChannel(a) => self.m_archive_channel(ctx, a, approved).await,
            C::SetDefaultModel(a) => self.m_set_default_model(ctx, a, approved).await,
            C::SetWorkerTags(a) => self.m_set_worker_tags(ctx, a, approved).await,
            C::InstallTool(a) => self.m_install_tool(ctx, a, approved).await,
            C::AddAccount(a) => self.m_add_account(ctx, a).await,
            C::StartSignIn(a) => self.m_start_sign_in(ctx, a).await,
            C::SetAccountExtras(a) => self.m_set_account_extras(ctx, a, approved).await,
            C::ConnectService(a) => self.m_connect_service(ctx, a).await,
            C::UpdateConnection(a) => self.m_update_connection(ctx, a, approved).await,
            C::Disconnect(a) => self.m_disconnect(ctx, a, approved).await,
            C::UpdateSkill(a) => self.m_update_skill(ctx, a, approved).await,
            C::RemoveSkill(a) => self.m_remove_skill(ctx, a, approved).await,
            C::UpdateThisChannel(a) => self.m_update_this_channel(ctx, a, approved).await,
            C::SetChannelTags(a) => self.m_set_channel_tags(ctx, a, approved).await,
            C::SetChannelConnector(a) => self.m_set_channel_connector(ctx, a, approved).await,
            C::SetChannelSkill(a) => self.m_set_channel_skill(ctx, a, approved).await,
            C::RequestAccess(a) => self.m_request_access(ctx, a, approved).await,
            other => Err(HubError::internal(format!(
                "{} is not a management tool",
                other.tool().name()
            ))),
        }
    }

    // Read tools ---------------------------------------------------------------

    async fn tool_list_connections(&self, ctx: &ToolCtx) -> Result<Value> {
        let channel_id = ctx.channel.id;
        let everything = ctx.audience == ToolAudience::EverythingCoordinator;
        let result = self
            .store
            .read(move |conn| {
                let channel = conversation::channel_or_404(conn, channel_id)?;
                let all = con_store::connections(conn)?;
                let connections: Vec<Connection> = if everything {
                    all
                } else {
                    all.into_iter()
                        .filter(|c| c.use_in.matches_channel(&channel))
                        .collect()
                };
                let off = connections
                    .iter()
                    .filter(|c| channel.disabled_connections.contains(&c.id))
                    .map(|c| c.id)
                    .collect();
                Ok(ListConnectionsResult {
                    connections,
                    off_in_this_channel: off,
                })
            })
            .await?;
        Ok(serde_json::to_value(result)?)
    }

    async fn tool_list_installed_skills(&self, ctx: &ToolCtx) -> Result<Value> {
        let channel_id = ctx.channel.id;
        let everything = ctx.audience == ToolAudience::EverythingCoordinator;
        let result = self
            .store
            .read(move |conn| {
                let channel = conversation::channel_or_404(conn, channel_id)?;
                let all = skill_store::skills(conn)?;
                let skills: Vec<Skill> = if everything {
                    all
                } else {
                    all.into_iter()
                        .filter(|s| s.use_in.matches_channel(&channel))
                        .collect()
                };
                let off = skills
                    .iter()
                    .filter(|s| channel.disabled_skills.contains(&s.id))
                    .map(|s| s.id)
                    .collect();
                Ok(ListInstalledSkillsResult {
                    skills,
                    off_in_this_channel: off,
                })
            })
            .await?;
        Ok(serde_json::to_value(result)?)
    }

    async fn tool_search_connector_catalog(
        &self,
        args: SearchConnectorCatalogArgs,
    ) -> Result<Value> {
        let mut services = self
            .search_services(Some(&args.search), args.category.as_deref())
            .await?;
        services.truncate(CATALOG_SEARCH_MAX);
        Ok(serde_json::to_value(SearchConnectorCatalogResult {
            services,
        })?)
    }

    // Channels -------------------------------------------------------------------

    async fn m_create_channel(
        &self,
        ctx: &ToolCtx,
        a: CreateChannelArgs,
        approved: bool,
    ) -> Result<ManagementResult> {
        let tags = normalize_tags(a.tags.clone().unwrap_or_default())?;
        if !tags.is_empty() && !approved {
            let effect = self.tag_effect(None, &tags).await?;
            return self
                .ask_first(
                    ctx,
                    HubTool::CreateChannel,
                    &a,
                    format!(
                        "Create the channel {} with the tags {}.",
                        a.name.trim(),
                        tags.join(", ")
                    ),
                    effect,
                )
                .await;
        }
        let group_id = match a.group.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
            Some(name) => Some(self.group_named(name).await?),
            None => None,
        };
        let channel = self
            .channel_create(
                &Caller::Hub,
                ChannelCreate {
                    name: a.name,
                    purpose: a.purpose,
                    instructions: String::new(),
                    icon: None,
                    color: None,
                    group_id,
                    coordinator_selection: a.model,
                    task_selection: a.task_model,
                    repo_ids: Vec::new(),
                    default_isolation: None,
                    default_approval_mode: None,
                    memory_enabled: None,
                    tags,
                },
            )
            .await?
            .channel;
        let mut result = self
            .applied(
                ctx,
                approved,
                format!("The assistant created the channel {}.", channel.name),
            )
            .await?;
        result.channel_id = Some(channel.id);
        Ok(result)
    }

    /// The channel update for the fields that both channel tools change.
    #[allow(clippy::too_many_arguments)]
    async fn apply_channel_update(
        &self,
        channel: &Channel,
        name: Option<String>,
        purpose: Option<String>,
        instructions: Option<String>,
        model: Option<ModelSelection>,
        task_model: Option<ModelSelection>,
        repos: Option<Vec<String>>,
        tags: Option<Vec<String>>,
    ) -> Result<Channel> {
        let repo_ids = match repos {
            Some(urls) => Some(self.repo_ids(&urls).await?),
            None => None,
        };
        Ok(self
            .channel_update(
                &Caller::Hub,
                ChannelUpdate {
                    channel_id: channel.id,
                    name,
                    purpose,
                    instructions,
                    icon: None,
                    color: None,
                    coordinator_selection: model.map(Some),
                    task_selection: task_model.map(Some),
                    preferred_workers: None,
                    repo_ids,
                    default_isolation: None,
                    default_approval_mode: None,
                    allowed_local_mcp_servers: None,
                    memory_enabled: None,
                    tags,
                },
            )
            .await?
            .channel)
    }

    async fn m_update_channel(
        &self,
        ctx: &ToolCtx,
        a: UpdateChannelArgs,
        approved: bool,
    ) -> Result<ManagementResult> {
        let id = a.channel;
        let channel = self
            .store
            .read(move |conn| conversation::channel_or_404(conn, id))
            .await?;
        let tags = a.tags.clone().map(normalize_tags).transpose()?;
        let tags_change = tags.as_ref().is_some_and(|t| *t != channel.tags);
        if tags_change && !approved {
            let new_tags = tags.clone().unwrap_or_default();
            let effect = self.tag_effect(Some(&channel), &new_tags).await?;
            return self
                .ask_first(
                    ctx,
                    HubTool::UpdateChannel,
                    &a,
                    format!(
                        "Change the tags of {} to {}.",
                        channel.name,
                        if new_tags.is_empty() {
                            "none".to_owned()
                        } else {
                            new_tags.join(", ")
                        }
                    ),
                    effect,
                )
                .await;
        }
        if let Some(group) = a.group.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
            let group_id = self.group_named(group).await?;
            if channel.group_id != Some(group_id) {
                self.channel_move(
                    &Caller::Hub,
                    ChannelMove {
                        channel_id: channel.id,
                        group_id: Some(group_id),
                        position: i64::MAX,
                    },
                )
                .await?;
            }
        }
        let updated = self
            .apply_channel_update(
                &channel,
                a.name,
                a.purpose,
                a.instructions,
                a.model,
                a.task_model,
                a.repos,
                tags.filter(|_| tags_change),
            )
            .await?;
        let mut result = self
            .applied(
                ctx,
                approved,
                format!("The assistant updated the channel {}.", updated.name),
            )
            .await?;
        result.channel_id = Some(updated.id);
        Ok(result)
    }

    async fn m_archive_channel(
        &self,
        ctx: &ToolCtx,
        a: ArchiveChannelArgs,
        approved: bool,
    ) -> Result<ManagementResult> {
        let id = a.channel;
        let channel = self
            .store
            .read(move |conn| conversation::channel_or_404(conn, id))
            .await?;
        if channel.is_everything() {
            return Ok(ManagementResult::refused(
                "The Everything channel cannot be archived.",
            ));
        }
        if !approved {
            return self
                .ask_first(
                    ctx,
                    HubTool::ArchiveChannel,
                    &a,
                    format!("Archive the channel {}.", channel.name),
                    "The channel leaves the sidebar and its assistant stops. Its messages, tasks and memory stay, and the user can bring it back.".to_owned(),
                )
                .await;
        }
        self.channel_archive(
            &Caller::Hub,
            ChannelArchive {
                channel_id: id,
                archived: true,
            },
        )
        .await?;
        let mut result =
            ManagementResult::applied(format!("The channel {} is archived.", channel.name));
        result.channel_id = Some(id);
        Ok(result)
    }

    async fn m_set_default_model(
        &self,
        ctx: &ToolCtx,
        a: SetDefaultModelArgs,
        approved: bool,
    ) -> Result<ManagementResult> {
        let everything = self.store.read(conversation::everything_channel).await?;
        self.apply_channel_update(
            &everything,
            None,
            None,
            None,
            Some(a.model.clone()),
            None,
            None,
            None,
        )
        .await?;
        self.applied(
            ctx,
            approved,
            format!("The assistant set the default model to {}.", a.model.model),
        )
        .await
    }

    // Workers and accounts ---------------------------------------------------------

    async fn worker_name(&self, worker: consort_proto::ids::WorkerId) -> Result<String> {
        let w = self
            .store
            .read(move |conn| machine::worker_or_404(conn, worker))
            .await?;
        Ok(w.name)
    }

    async fn m_set_worker_tags(
        &self,
        ctx: &ToolCtx,
        a: SetWorkerTagsArgs,
        approved: bool,
    ) -> Result<ManagementResult> {
        let tags = normalize_tags(a.tags.clone())?;
        let name = self.worker_name(a.worker).await?;
        if !approved {
            return self
                .ask_first(
                    ctx,
                    HubTool::SetWorkerTags,
                    &a,
                    format!(
                        "Change the tags of the worker {name} to {}.",
                        if tags.is_empty() {
                            "none".to_owned()
                        } else {
                            tags.join(", ")
                        }
                    ),
                    "Placement uses worker tags, so channels and tasks can move to or away from this worker.".to_owned(),
                )
                .await;
        }
        self.worker_set_tags(
            &Caller::Hub,
            WorkerSetTags {
                worker_id: a.worker,
                tags,
            },
        )
        .await?;
        Ok(ManagementResult::applied(format!(
            "The tags of {name} changed."
        )))
    }

    async fn m_install_tool(
        &self,
        ctx: &ToolCtx,
        a: InstallToolArgs,
        approved: bool,
    ) -> Result<ManagementResult> {
        let name = self.worker_name(a.worker).await?;
        self.worker_tool_install(
            &Caller::Hub,
            WorkerToolInstall {
                worker_id: a.worker,
                tool: ToolId::from(a.tool.trim()),
            },
        )
        .await?;
        self.applied(
            ctx,
            approved,
            format!(
                "The assistant started to install {} on {name}.",
                a.tool.trim()
            ),
        )
        .await
    }

    async fn post_sign_in_card(&self, ctx: &ToolCtx, card: SignInCard) -> Result<ApprovalId> {
        let id = ApprovalId::new();
        let summary = format!("Sign in to {} ({})", card.account_label, card.harness);
        self.post_hub_card(ctx, id, summary, ApprovalDetails::SignIn(Box::new(card)))
            .await?;
        Ok(id)
    }

    async fn m_add_account(&self, ctx: &ToolCtx, a: AddAccountArgs) -> Result<ManagementResult> {
        let account_id = self
            .account_create(
                &Caller::Hub,
                AccountCreate {
                    worker_id: a.worker,
                    harness: a.provider.trim().to_owned(),
                    label: a.name.clone(),
                    acp: None,
                    env: Default::default(),
                },
            )
            .await?
            .account_id;
        let approval_id = self
            .post_sign_in_card(
                ctx,
                SignInCard {
                    worker_id: a.worker,
                    account_id,
                    account_label: a.name.trim().to_owned(),
                    harness: a.provider.trim().to_owned(),
                    method: None,
                    sign_in_id: None,
                },
            )
            .await?;
        let mut result = ManagementResult::waiting_for_user(
            approval_id,
            format!(
                "The account {} is added. The user signs in on the card.",
                a.name.trim()
            ),
        );
        result.account_id = Some(account_id);
        Ok(result)
    }

    async fn m_start_sign_in(&self, ctx: &ToolCtx, a: StartSignInArgs) -> Result<ManagementResult> {
        let (worker, account) = (a.worker, a.account);
        let found = self
            .store
            .read(move |conn| {
                let worker = machine::worker_or_404(conn, worker)?;
                Ok(worker
                    .inventory
                    .and_then(|i| i.accounts.into_iter().find(|acc| acc.account_id == account)))
            })
            .await?
            .ok_or_else(|| {
                HubError::not_found(
                    "The worker has no such account. Call list_workers to see the accounts.",
                )
            })?;
        let method = match a.method.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
            None => None,
            Some(kind) => {
                let parsed: Option<SignInMethod> =
                    serde_json::from_value(serde_json::json!({ "kind": kind })).ok();
                // A method with fields, such as an API key, needs the app.
                parsed.filter(|m| found.sign_in_methods.contains(m))
            }
        };
        let approval_id = self
            .post_sign_in_card(
                ctx,
                SignInCard {
                    worker_id: a.worker,
                    account_id: a.account,
                    account_label: found.label.clone(),
                    harness: found.harness.clone(),
                    method,
                    sign_in_id: None,
                },
            )
            .await?;
        let mut result = ManagementResult::waiting_for_user(
            approval_id,
            format!("The user signs in to {} on the card.", found.label),
        );
        result.account_id = Some(a.account);
        Ok(result)
    }

    async fn m_set_account_extras(
        &self,
        ctx: &ToolCtx,
        a: SetAccountExtrasArgs,
        approved: bool,
    ) -> Result<ManagementResult> {
        let name = self.worker_name(a.worker).await?;
        if a.enabled && !approved {
            return self
                .ask_first(
                    ctx,
                    HubTool::SetAccountExtras,
                    &a,
                    format!("Turn on the provider extras of an account on {name}."),
                    "New sessions of this account get the MCP servers, plugins and skills that the provider syncs, next to the ones from Consort.".to_owned(),
                )
                .await;
        }
        self.account_update(
            &Caller::Hub,
            AccountUpdate {
                worker_id: a.worker,
                account_id: a.account,
                label: None,
                env_set: Default::default(),
                env_remove: Vec::new(),
                extras_enabled: Some(a.enabled),
            },
        )
        .await?;
        let mut result = self
            .applied(
                ctx,
                approved,
                format!(
                    "The provider extras of an account on {name} are {}.",
                    if a.enabled { "on" } else { "off" }
                ),
            )
            .await?;
        result.account_id = Some(a.account);
        Ok(result)
    }

    // Connectors -------------------------------------------------------------------

    /// The default scope of a new item from this session: every channel for
    /// Everything, otherwise the channels with this channel's tags, or this
    /// channel alone (DECISIONS row 345).
    pub(crate) fn default_use_in(ctx: &ToolCtx) -> UseIn {
        if ctx.audience == ToolAudience::EverythingCoordinator {
            UseIn::EveryChannel
        } else if ctx.channel.tags.is_empty() {
            UseIn::Channels {
                channel_ids: vec![ctx.channel.id],
            }
        } else {
            UseIn::Tags {
                tags: ctx.channel.tags.clone(),
            }
        }
    }

    async fn m_connect_service(
        &self,
        ctx: &ToolCtx,
        a: ConnectServiceArgs,
    ) -> Result<ManagementResult> {
        self.connectors.client()?;
        if let Err(err) = validate_connection_name(&a.name) {
            return Ok(ManagementResult::refused(format!("The name {err}.")));
        }
        let use_in = self
            .check_use_in(
                a.use_in
                    .clone()
                    .unwrap_or_else(|| Self::default_use_in(ctx)),
            )
            .await?;
        let slug = a.service.trim().to_ascii_lowercase();
        let service = self
            .search_services(Some(&slug), None)
            .await?
            .into_iter()
            .find(|s| s.slug == slug)
            .ok_or_else(|| {
                HubError::not_found(format!(
                    "The catalog has no service {slug}. Call search_connector_catalog to find its slug."
                ))
            })?;
        let card = ConnectServiceCard {
            service: service.slug.clone(),
            service_name: service.name.clone(),
            logo_url: service.logo_url.clone(),
            name: a.name.trim().to_owned(),
            use_in,
            approvals: a.approvals.unwrap_or_default(),
            connection_id: None,
        };
        let id = ApprovalId::new();
        let summary = format!("Connect {} as {}", service.name, card.name);
        self.post_hub_card(
            ctx,
            id,
            summary,
            ApprovalDetails::ConnectService(Box::new(card)),
        )
        .await?;
        Ok(ManagementResult::waiting_for_user(
            id,
            format!(
                "The user continues on the connect card and signs in to {} in the browser. The answer comes as a card_answered event.",
                service.name
            ),
        ))
    }

    async fn m_update_connection(
        &self,
        ctx: &ToolCtx,
        a: UpdateConnectionArgs,
        approved: bool,
    ) -> Result<ManagementResult> {
        let connection = self.find_connection(&a.connection).await?;
        let use_in = match a.use_in.clone() {
            Some(u) => Some(self.check_use_in(u).await?),
            None => None,
        };
        let channels = self.all_channels().await?;
        let added = use_in
            .as_ref()
            .map(|u| u.added_channels(&connection.use_in, &channels).len())
            .unwrap_or(0);
        let looser = a
            .approvals
            .as_ref()
            .is_some_and(|new| new.is_looser_than(&connection.approvals));
        if (added > 0 || looser) && !approved {
            let mut effects = Vec::new();
            if added > 0 {
                effects.push(format!(
                    "{} becomes available in {}.",
                    connection.name,
                    more_channels(added)
                ));
            }
            if looser {
                effects.push(format!(
                    "{} asks the user less often before it runs a tool.",
                    connection.name
                ));
            }
            return self
                .ask_first(
                    ctx,
                    HubTool::UpdateConnection,
                    &a,
                    format!("Change the settings of {}.", connection.name),
                    effects.join(" "),
                )
                .await;
        }
        let updated = self
            .update_connection_fields(connection.id, a.name, use_in, a.approvals)
            .await?;
        let mut result = self
            .applied(
                ctx,
                approved,
                format!(
                    "The settings of {} ({}) changed.",
                    updated.name, updated.handle
                ),
            )
            .await?;
        result.connection_id = Some(updated.id);
        Ok(result)
    }

    async fn m_disconnect(
        &self,
        ctx: &ToolCtx,
        a: DisconnectArgs,
        approved: bool,
    ) -> Result<ManagementResult> {
        let connection = self.find_connection(&a.connection).await?;
        if !approved {
            return self
                .ask_first(
                    ctx,
                    HubTool::Disconnect,
                    &a,
                    format!(
                        "Disconnect {} ({}).",
                        connection.name, connection.service_name
                    ),
                    "No channel can use it anymore, and Composio deletes the connected account."
                        .to_owned(),
                )
                .await;
        }
        self.disconnect(connection.id).await?;
        let mut result = ManagementResult::applied(format!("{} is disconnected.", connection.name));
        result.connection_id = Some(connection.id);
        Ok(result)
    }

    // Skills -------------------------------------------------------------------------

    async fn m_update_skill(
        &self,
        ctx: &ToolCtx,
        a: UpdateSkillArgs,
        approved: bool,
    ) -> Result<ManagementResult> {
        let skill = self.find_skill(&a.skill).await?;
        let use_in = match a.use_in.clone() {
            Some(u) => Some(self.check_use_in(u).await?),
            None => None,
        };
        let channels = self.all_channels().await?;
        let added = use_in
            .as_ref()
            .map(|u| u.added_channels(&skill.use_in, &channels).len())
            .unwrap_or(0);
        let turns_on_scripts = a.enabled == Some(true) && !skill.enabled && skill.has_scripts();
        let accept = a.accept_new_version == Some(true);
        if (added > 0 || turns_on_scripts || accept) && !approved {
            let mut effects = Vec::new();
            if turns_on_scripts {
                effects.push(format!(
                    "The skill {} has scripts that sessions can run: {}.",
                    skill.name,
                    skill.scripts.join(", ")
                ));
            }
            if added > 0 {
                effects.push(format!("It becomes available in {}.", more_channels(added)));
            }
            if accept {
                effects.push("New sessions get the new version of its files.".to_owned());
            }
            return self
                .ask_first(
                    ctx,
                    HubTool::UpdateSkill,
                    &a,
                    format!("Change the skill {}.", skill.name),
                    effects.join(" "),
                )
                .await;
        }
        let updated = self
            .update_skill_fields(skill.id, a.enabled, use_in, accept)
            .await?;
        let mut result = self
            .applied(
                ctx,
                approved,
                format!("The skill {} changed.", updated.name),
            )
            .await?;
        result.skill_id = Some(updated.id);
        Ok(result)
    }

    async fn m_remove_skill(
        &self,
        ctx: &ToolCtx,
        a: RemoveSkillArgs,
        approved: bool,
    ) -> Result<ManagementResult> {
        let skill = self.find_skill(&a.skill).await?;
        if !approved {
            return self
                .ask_first(
                    ctx,
                    HubTool::RemoveSkill,
                    &a,
                    format!("Remove the skill {}.", skill.name),
                    "New sessions do not get it anymore.".to_owned(),
                )
                .await;
        }
        let id = skill.id;
        self.store
            .write(move |tx| {
                skill_store::skill_or_404(tx.db(), id)?;
                tx.delete_skill(id)
            })
            .await?;
        let mut result = ManagementResult::applied(format!("The skill {} is removed.", skill.name));
        result.skill_id = Some(id);
        Ok(result)
    }

    // A channel's own tools ------------------------------------------------------------

    async fn m_update_this_channel(
        &self,
        ctx: &ToolCtx,
        a: UpdateThisChannelArgs,
        approved: bool,
    ) -> Result<ManagementResult> {
        let updated = self
            .apply_channel_update(
                &ctx.channel,
                None,
                a.purpose,
                a.instructions,
                a.model,
                a.task_model,
                a.repos,
                None,
            )
            .await?;
        let mut result = self
            .applied(
                ctx,
                approved,
                format!("The assistant updated the channel {}.", updated.name),
            )
            .await?;
        result.channel_id = Some(updated.id);
        Ok(result)
    }

    async fn m_set_channel_tags(
        &self,
        ctx: &ToolCtx,
        a: SetChannelTagsArgs,
        approved: bool,
    ) -> Result<ManagementResult> {
        let tags = normalize_tags(a.tags.clone())?;
        if tags == ctx.channel.tags {
            return Ok(ManagementResult::applied(
                "The channel has these tags already.",
            ));
        }
        if !approved {
            let effect = self.tag_effect(Some(&ctx.channel), &tags).await?;
            return self
                .ask_first(
                    ctx,
                    HubTool::SetChannelTags,
                    &a,
                    format!(
                        "Change the tags of {} to {}.",
                        ctx.channel.name,
                        if tags.is_empty() {
                            "none".to_owned()
                        } else {
                            tags.join(", ")
                        }
                    ),
                    effect,
                )
                .await;
        }
        let updated = self
            .apply_channel_update(&ctx.channel, None, None, None, None, None, None, Some(tags))
            .await?;
        let mut result =
            ManagementResult::applied(format!("The tags of {} changed.", updated.name));
        result.channel_id = Some(updated.id);
        Ok(result)
    }

    async fn m_set_channel_connector(
        &self,
        ctx: &ToolCtx,
        a: SetChannelConnectorArgs,
        approved: bool,
    ) -> Result<ManagementResult> {
        let connection = self.find_connection(&a.connection).await?;
        if !connection.use_in.matches_channel(&ctx.channel) {
            return Ok(ManagementResult::refused(format!(
                "{} is not available in this channel. Call request_access to ask the user for it.",
                connection.name
            )));
        }
        self.channel_set_connection_enabled(
            &Caller::Hub,
            ChannelSetConnectionEnabled {
                channel_id: ctx.channel.id,
                connection_id: connection.id,
                enabled: a.enabled,
            },
        )
        .await?;
        let mut result = self
            .applied(
                ctx,
                approved,
                format!(
                    "{} is {} in this channel.",
                    connection.name,
                    if a.enabled { "on" } else { "off" }
                ),
            )
            .await?;
        result.connection_id = Some(connection.id);
        result.channel_id = Some(ctx.channel.id);
        Ok(result)
    }

    async fn m_set_channel_skill(
        &self,
        ctx: &ToolCtx,
        a: SetChannelSkillArgs,
        approved: bool,
    ) -> Result<ManagementResult> {
        let skill = self.find_skill(&a.skill).await?;
        if !skill.use_in.matches_channel(&ctx.channel) {
            return Ok(ManagementResult::refused(format!(
                "The skill {} is not available in this channel. Call request_access to ask the user for it.",
                skill.name
            )));
        }
        self.channel_set_skill_enabled(
            &Caller::Hub,
            ChannelSetSkillEnabled {
                channel_id: ctx.channel.id,
                skill_id: skill.id,
                enabled: a.enabled,
            },
        )
        .await?;
        let mut result = self
            .applied(
                ctx,
                approved,
                format!(
                    "The skill {} is {} in this channel.",
                    skill.name,
                    if a.enabled { "on" } else { "off" }
                ),
            )
            .await?;
        result.skill_id = Some(skill.id);
        result.channel_id = Some(ctx.channel.id);
        Ok(result)
    }

    async fn m_request_access(
        &self,
        ctx: &ToolCtx,
        a: RequestAccessArgs,
        approved: bool,
    ) -> Result<ManagementResult> {
        let channels = self.all_channels().await?;
        let channel = &ctx.channel;
        match (a.connection.as_deref(), a.skill.as_deref()) {
            (Some(key), None) => {
                let connection = self.find_connection(key).await?;
                if connection.use_in.matches_channel(channel) {
                    return Ok(ManagementResult::refused(format!(
                        "{} is available in this channel already. Call set_channel_connector to turn it on here.",
                        connection.name
                    )));
                }
                let wider = widen_to(&connection.use_in, channel, &channels);
                if !approved {
                    return self
                        .ask_first(
                            ctx,
                            HubTool::RequestAccess,
                            &a,
                            format!("Make {} available in {}.", connection.name, channel.name),
                            format!(
                                "{} becomes available in {}. Its Use in becomes {}.",
                                connection.name,
                                channel.name,
                                describe_use_in(&wider, &channels)
                            ),
                        )
                        .await;
                }
                let updated = self
                    .update_connection_fields(connection.id, None, Some(wider), None)
                    .await?;
                let mut result = ManagementResult::applied(format!(
                    "{} is available in {} now.",
                    updated.name, channel.name
                ));
                result.connection_id = Some(updated.id);
                Ok(result)
            }
            (None, Some(key)) => {
                let skill = self.find_skill(key).await?;
                if skill.use_in.matches_channel(channel) {
                    return Ok(ManagementResult::refused(format!(
                        "The skill {} is available in this channel already. Call set_channel_skill to turn it on here.",
                        skill.name
                    )));
                }
                let wider = widen_to(&skill.use_in, channel, &channels);
                if !approved {
                    return self
                        .ask_first(
                            ctx,
                            HubTool::RequestAccess,
                            &a,
                            format!(
                                "Make the skill {} available in {}.",
                                skill.name, channel.name
                            ),
                            format!(
                                "The skill {} becomes available in {}. Its Use in becomes {}.",
                                skill.name,
                                channel.name,
                                describe_use_in(&wider, &channels)
                            ),
                        )
                        .await;
                }
                let updated = self
                    .update_skill_fields(skill.id, None, Some(wider), false)
                    .await?;
                let mut result = ManagementResult::applied(format!(
                    "The skill {} is available in {} now.",
                    updated.name, channel.name
                ));
                result.skill_id = Some(updated.id);
                Ok(result)
            }
            _ => Ok(ManagementResult::refused(
                "Name one connection or one skill.",
            )),
        }
    }
}
