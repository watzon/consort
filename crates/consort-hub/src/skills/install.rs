//! `skill.preview`, `skill.install`, the install card, and the
//! `search_skills` and `add_skill` hub tools.
//!
//! Every source ends in the same checks (`package.rs`) and the same store
//! (`Hub::store_prepared`). A preview keeps what it fetched for one hour, so
//! the install right after it stores exactly what the user saw.

use std::sync::Arc;

use consort_proto::client_api::{
    SkillInstall, SkillInstallSource, SkillPreview, SkillPreviewResult, SkillResult,
};
use consort_proto::error::ErrorCode;
use consort_proto::ids::ApprovalId;
use consort_proto::model::{
    Approval, ApprovalDetails, ApprovalStatus, AuditStatus, HUB_CARD_ALLOW, SKILL_UPLOAD_MAX_BYTES,
    Skill, SkillAudit, SkillInstallCard, SkillSource, UseIn,
};
use consort_proto::tools::{AddSkillArgs, ManagementResult, SearchSkillsArgs, SearchSkillsResult};
use serde_json::Value;

use super::directory::looks_like_entry_id;
use super::directory::with_ref;
use super::git::{check_ref, normalize_repo_path, parse_git_url};
use super::package::{Prepared, prepare, skill_invalid};
use super::remote::{Remote, RemoteFiles};
use super::upload::unpack_zip;
use crate::error::{HubError, Result};
use crate::hub::{Caller, Hub, ToolCtx};
use crate::ops::waiting::resume_if_nothing_waits;
use crate::store::{skills as skill_store, work};

/// The most entries that `search_skills` returns. The result goes into the
/// model's context.
const SEARCH_SKILLS_MAX: usize = 20;

/// A fetched and checked skill, not stored yet.
#[derive(Debug, Clone)]
pub(crate) struct Fetched {
    /// Where it comes from, as the skill records it.
    pub source: SkillSource,
    /// The commit that the install pins. Empty for an upload.
    pub commit: Option<String>,
    pub prepared: Prepared,
    /// The audits that the directory gave.
    pub audits: Vec<SkillAudit>,
}

impl Fetched {
    pub fn blocked_reason(&self) -> Option<String> {
        blocked_reason(&self.audits)
    }
}

/// Why the hub blocks a skill: the first audit that failed.
pub(crate) fn blocked_reason(audits: &[SkillAudit]) -> Option<String> {
    audits
        .iter()
        .find(|a| a.status == AuditStatus::Failed)
        .map(|a| format!("The audit of {} failed.", a.source))
}

/// The preview cache key of an install source.
fn source_key(source: &SkillInstallSource) -> String {
    match source {
        SkillInstallSource::Upload {
            blob_sha256, size, ..
        } => format!("upload:{blob_sha256}:{size}"),
        other => format!(
            "source:{}",
            serde_json::to_string(other).unwrap_or_default()
        ),
    }
}

/// The preview cache key of an install card.
fn card_key(id: ApprovalId) -> String {
    format!("card:{id}")
}

/// The file name of an upload, as a label: its last part, with no control
/// characters, at most 200 characters.
fn upload_label(file_name: &str) -> String {
    let last = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    let clean: String = last.chars().filter(|c| !c.is_control()).take(200).collect();
    if clean.trim().is_empty() {
        "skill.zip".to_owned()
    } else {
        clean
    }
}

fn skill_blocked(message: impl Into<String>) -> HubError {
    HubError::new(ErrorCode::SkillBlocked, message)
}

/// The location text that a `SkillSource::Directory` records: a git repo with
/// the ref it follows, or the marker of another type.
fn directory_location(got: &RemoteFiles) -> String {
    match Remote::parse(&got.url) {
        Remote::Git(url) => with_ref(&url, got.git_ref.as_deref()),
        _ => got.url.clone(),
    }
}

impl Hub {
    /// Fetches and checks a skill, or takes it from a recent preview.
    /// `typed_by_user` is true for a request of a client: the user gave the
    /// URL, so git may use the credentials of the hub's user for it (see
    /// `git.rs`). A tool of the assistant passes false.
    pub(crate) async fn fetch_install_source(
        &self,
        source: &SkillInstallSource,
        typed_by_user: bool,
    ) -> Result<Arc<Fetched>> {
        let key = source_key(source);
        if let Some(fetched) = self.skill_sources.preview(&key) {
            return Ok(fetched);
        }
        let fetched = match source {
            SkillInstallSource::Git { url, path, git_ref } => {
                let options = self.git_options_for(url, typed_by_user).await?;
                let target = parse_git_url(url, options)?;
                let path = normalize_repo_path(path.as_deref().or(target.path.as_deref()))?;
                let git_ref = git_ref
                    .as_deref()
                    .map(str::trim)
                    .filter(|r| !r.is_empty())
                    .map(str::to_owned)
                    .or(target.git_ref);
                if let Some(r) = &git_ref {
                    check_ref(r)?;
                }
                let got = self
                    .fetch_remote(
                        &target.url,
                        path.as_deref(),
                        git_ref.as_deref(),
                        typed_by_user,
                    )
                    .await?;
                Fetched {
                    source: SkillSource::Git {
                        url: got.url,
                        path,
                        git_ref,
                    },
                    commit: Some(got.commit),
                    prepared: prepare(got.files, None)?,
                    audits: Vec::new(),
                }
            }
            SkillInstallSource::Directory { source, entry_id } => {
                let entry = self.directory_entry(source.as_deref(), entry_id).await?;
                let repo = entry.repo_url.clone().ok_or_else(|| {
                    HubError::not_found(format!(
                        "The skill {} has no source that the hub can install from.",
                        entry.name
                    ))
                })?;
                // The directory's location is checked like one that the user
                // types. A git entry follows the branch that the directory
                // names, or the default branch.
                let path = normalize_repo_path(entry.path.as_deref())?;
                let got = self
                    .fetch_remote(&repo, path.as_deref(), None, false)
                    .await?;
                Fetched {
                    source: SkillSource::Directory {
                        directory: entry.directory,
                        entry_id: entry.entry_id.clone(),
                        url: directory_location(&got),
                        path,
                    },
                    commit: Some(got.commit),
                    prepared: prepare(got.files, entry.license.clone())?,
                    audits: entry.audits.clone(),
                }
            }
            SkillInstallSource::Upload {
                blob_sha256,
                size,
                file_name,
            } => {
                if *size > SKILL_UPLOAD_MAX_BYTES {
                    return Err(skill_invalid(format!(
                        "The zip is larger than {SKILL_UPLOAD_MAX_BYTES} bytes."
                    )));
                }
                let path = self.store.blobs_dir().join(blob_sha256.as_str());
                let meta = tokio::fs::metadata(&path).await.map_err(|_| {
                    HubError::not_found("The uploaded zip is not on the hub. Upload it again.")
                })?;
                if meta.len() != *size {
                    return Err(HubError::invalid(
                        "The size of the uploaded zip is not the size in the request. Upload it again.",
                    ));
                }
                let bytes = tokio::fs::read(&path).await?;
                if crate::util::sha256(&bytes) != *blob_sha256 {
                    return Err(HubError::invalid(
                        "The uploaded zip does not match its hash. Upload it again.",
                    ));
                }
                let files = tokio::task::spawn_blocking(move || unpack_zip(&bytes))
                    .await
                    .map_err(|_| HubError::internal("the zip reader stopped"))??;
                Fetched {
                    source: SkillSource::Upload {
                        file_name: upload_label(file_name),
                    },
                    commit: None,
                    prepared: prepare(files, None)?,
                    audits: Vec::new(),
                }
            }
        };
        let fetched = Arc::new(fetched);
        self.skill_sources.keep_preview(key, fetched.clone());
        Ok(fetched)
    }

    async fn installed_named(&self, name: &str) -> Result<Option<Skill>> {
        let name = name.to_owned();
        self.store
            .read(move |conn| skill_store::find_skill(conn, &name))
            .await
    }

    pub(crate) async fn skill_preview(
        &self,
        _caller: &Caller,
        request: SkillPreview,
    ) -> Result<SkillPreviewResult> {
        let fetched = self.fetch_install_source(&request.source, true).await?;
        let p = &fetched.prepared;
        let installed = self.installed_named(&p.name).await?;
        Ok(SkillPreviewResult {
            name: p.name.clone(),
            description: p.description.clone(),
            commit: fetched.commit.clone(),
            content_hash: p.content_hash.clone(),
            license: p.license.clone(),
            scripts: p.scripts.clone(),
            audits: fetched.audits.clone(),
            files: p.infos.clone(),
            has_hooks: p.hooks_removed,
            blocked_reason: fetched.blocked_reason(),
            installed_skill_id: installed.map(|s| s.id),
        })
    }

    /// Stores a fetched skill. A skill whose audit failed is refused. A
    /// skill with scripts is stored off, whatever `enabled` says.
    pub(crate) async fn install_fetched(
        &self,
        fetched: &Fetched,
        use_in: UseIn,
        enabled: Option<bool>,
    ) -> Result<Skill> {
        if let Some(reason) = fetched.blocked_reason() {
            return Err(skill_blocked(format!(
                "The skill {} is blocked: {reason}",
                fetched.prepared.name
            )));
        }
        let enabled = if fetched.prepared.scripts.is_empty() {
            enabled
        } else {
            Some(false)
        };
        self.store_prepared(
            fetched.prepared.clone(),
            super::StoreMeta {
                source: fetched.source.clone(),
                commit: fetched.commit.clone(),
                use_in,
                enabled,
                audits: fetched.audits.clone(),
                license: None,
            },
        )
        .await
    }

    pub(crate) async fn skill_install(
        &self,
        _caller: &Caller,
        request: SkillInstall,
    ) -> Result<SkillResult> {
        if let Some(id) = request.approval_id {
            self.pending_install_card(id).await?;
        }
        let fetched = self.fetch_install_source(&request.source, true).await?;
        let skill = self
            .install_fetched(&fetched, request.use_in, request.enabled)
            .await?;
        if let Some(id) = request.approval_id {
            self.close_install_card(id, &skill).await?;
        }
        Ok(SkillResult { skill })
    }

    /// The install card with this ID, while it waits.
    async fn pending_install_card(&self, id: ApprovalId) -> Result<Approval> {
        let approval = self
            .store
            .read(move |conn| work::approval(conn, id))
            .await?
            .ok_or_else(|| HubError::not_found("The install card does not exist."))?;
        if !approval.is_hub_card() || !matches!(approval.details, ApprovalDetails::SkillInstall(_))
        {
            return Err(HubError::invalid("This approval is not an install card."));
        }
        if approval.status != ApprovalStatus::Pending {
            return Err(HubError::conflict(
                "This install card is already answered or closed.",
            ));
        }
        Ok(approval)
    }

    /// Puts the installed skill on its card.
    async fn link_card_to_skill(&self, id: ApprovalId, skill: &Skill) -> Result<Option<Approval>> {
        let skill_id = skill.id;
        self.store
            .write(move |tx| {
                let Some(mut approval) = work::approval(tx.db(), id)? else {
                    return Ok(None);
                };
                let mut answered_now = false;
                if approval.status == ApprovalStatus::Pending {
                    approval.status = ApprovalStatus::Allowed;
                    approval.answer_option_id = Some(HUB_CARD_ALLOW.to_owned());
                    approval.answered_at = Some(tx.now());
                    answered_now = true;
                }
                if let ApprovalDetails::SkillInstall(card) = &mut approval.details {
                    card.skill_id = Some(skill_id);
                }
                tx.save_approval(&approval)?;
                if answered_now {
                    resume_if_nothing_waits(tx, approval.task_id)?;
                }
                Ok(Some(approval))
            })
            .await
    }

    /// Closes an install card after `skill.install` with its ID: marks it
    /// allowed, puts the skill on it, and tells the session that posted it.
    async fn close_install_card(&self, id: ApprovalId, skill: &Skill) -> Result<()> {
        if let Some(approval) = self.link_card_to_skill(id, skill).await? {
            self.send_card_answered(&approval, installed_text(skill))
                .await?;
        }
        Ok(())
    }

    /// Installs the skill of a card that the user allowed. It installs the
    /// files that the card showed: the ones of its preview, or the same
    /// commit again. Returns the text for the session.
    pub(crate) async fn install_from_card(
        &self,
        approval: &Approval,
        card: &SkillInstallCard,
    ) -> Result<String> {
        let fetched = match self.skill_sources.preview(&card_key(approval.id)) {
            Some(f) if f.commit == card.commit => f,
            _ => Arc::new(self.fetch_card_again(card).await?),
        };
        let skill = self
            .install_fetched(&fetched, card.use_in.clone(), None)
            .await?;
        self.link_card_to_skill(approval.id, &skill).await?;
        Ok(installed_text(&skill))
    }

    async fn fetch_card_again(&self, card: &SkillInstallCard) -> Result<Fetched> {
        let (location, path, git_ref) = match &card.source {
            SkillSource::Git { url, path, git_ref } => (url, path, git_ref.as_deref()),
            SkillSource::Directory { url, path, .. } => (url, path, None),
            SkillSource::Upload { .. } => {
                return Err(HubError::not_found(
                    "The uploaded zip of this card is gone. Upload it again.",
                ));
            }
        };
        // A git commit that the card shows is the ref to fetch.
        let wanted = match Remote::parse(location) {
            Remote::Git(_) => card.commit.as_deref().or(git_ref),
            _ => None,
        };
        let got = self
            .fetch_remote(location, path.as_deref(), wanted, false)
            .await?;
        if card.commit.as_ref().is_some_and(|c| *c != got.commit) {
            return Err(HubError::conflict(
                "The source no longer has the version that the card showed. Ask for the skill again.",
            ));
        }
        Ok(Fetched {
            source: card.source.clone(),
            commit: Some(got.commit),
            prepared: prepare(got.files, card.license.clone())?,
            audits: card.audits.clone(),
        })
    }

    // Hub tools ----------------------------------------------------------------

    pub(crate) async fn tool_search_skills(&self, args: SearchSkillsArgs) -> Result<Value> {
        let (mut entries, _) = self
            .search_directories(Some(&args.search), args.source.as_deref())
            .await?;
        entries.truncate(SEARCH_SKILLS_MAX);
        Ok(serde_json::to_value(SearchSkillsResult { entries })?)
    }

    /// `add_skill`: fetches and checks the skill, then posts an install card
    /// with the safety facts. The install runs when the user allows it.
    pub(crate) async fn tool_add_skill(&self, ctx: &ToolCtx, args: AddSkillArgs) -> Result<Value> {
        let source_text = args.source.trim();
        let source = if looks_like_entry_id(source_text) {
            SkillInstallSource::Directory {
                source: None,
                entry_id: source_text.to_owned(),
            }
        } else {
            SkillInstallSource::Git {
                url: source_text.to_owned(),
                path: args.path.clone(),
                git_ref: None,
            }
        };
        let use_in = self
            .check_use_in(
                args.use_in
                    .clone()
                    .unwrap_or_else(|| Self::default_use_in(ctx)),
            )
            .await?;
        // The assistant typed this source, not the user, so git uses no
        // credentials for it unless the user gave the hub this URL before.
        let fetched = self.fetch_install_source(&source, false).await?;
        let p = &fetched.prepared;
        if let Some(reason) = fetched.blocked_reason() {
            return Ok(serde_json::to_value(ManagementResult::refused(format!(
                "The skill {} is blocked: {reason}",
                p.name
            )))?);
        }
        if let Some(existing) = self.installed_named(&p.name).await? {
            let mut result = ManagementResult::refused(format!(
                "A skill named {} is installed already. Use update_skill to change it.",
                p.name
            ));
            result.skill_id = Some(existing.id);
            return Ok(serde_json::to_value(result)?);
        }
        let card = SkillInstallCard {
            name: p.name.clone(),
            description: p.description.clone(),
            source: fetched.source.clone(),
            commit: fetched.commit.clone(),
            license: p.license.clone(),
            scripts: p.scripts.clone(),
            audits: fetched.audits.clone(),
            use_in,
            blocked_reason: None,
            skill_id: None,
        };
        let id = ApprovalId::new();
        self.skill_sources
            .keep_preview(card_key(id), fetched.clone());
        let summary = format!("Add the skill {}", p.name);
        self.post_hub_card(
            ctx,
            id,
            summary,
            ApprovalDetails::SkillInstall(Box::new(card)),
        )
        .await?;
        let mut facts = Vec::new();
        if !p.scripts.is_empty() {
            facts.push(format!(
                "It has scripts ({}), so it stays off until the user turns it on.",
                p.scripts.join(", ")
            ));
        }
        if p.hooks_removed {
            facts.push("The hub removed its Claude-specific hooks.".to_owned());
        }
        Ok(serde_json::to_value(ManagementResult::waiting_for_user(
            id,
            format!(
                "The user decides on the install card for {}. {}The answer comes as a card_answered event.",
                p.name,
                facts.iter().map(|f| format!("{f} ")).collect::<String>()
            ),
        ))?)
    }
}

/// What the session learns after an install.
fn installed_text(skill: &Skill) -> String {
    if skill.has_scripts() && !skill.enabled {
        format!(
            "The user added the skill {}. It has scripts, so it stays off until the user turns it on.",
            skill.name
        )
    } else if skill.enabled {
        format!(
            "The user added the skill {}. New sessions in its channels get it.",
            skill.name
        )
    } else {
        format!("The user added the skill {}. It is off.", skill.name)
    }
}
