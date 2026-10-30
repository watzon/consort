//! The reducers of connectors and skills: hub entities to view records, and
//! view records back to the wire types that commands send.

use consort_proto::{
    client_api::SkillInstallSource,
    ids::ChannelId,
    model::{
        self, ApprovalDetails, ApprovalRule, AuditStatus, AuthSchemeKind, ConnectionStatus,
        SkillDirectoryKind, SkillFileChangeKind, SkillSource, SkillUpdateState, ToolApprovals,
        ToolKind, UseIn,
    },
};

use crate::{
    ClientError,
    cache::Cache,
    connectors::*,
    format,
    skills::*,
    view::{ApprovalChoice, ProviderLogo, provider_of},
};

pub fn use_in_view(use_in: &UseIn) -> UseInView {
    match use_in {
        UseIn::EveryChannel => UseInView::EveryChannel,
        UseIn::Tags { tags } => UseInView::Tags(tags.clone()),
        UseIn::Channels { channel_ids } => {
            UseInView::Channels(channel_ids.iter().map(|c| c.to_string()).collect())
        }
    }
}

/// The wire scope of a view scope. A channel ID that is not valid is an
/// error, not a smaller scope.
pub fn use_in_proto(use_in: &UseInView) -> Result<UseIn, ClientError> {
    Ok(match use_in {
        UseInView::EveryChannel => UseIn::EveryChannel,
        UseInView::Tags(tags) => {
            if tags.is_empty() {
                return Err(ClientError::Hub {
                    code: "invalid".into(),
                    message: "Choose at least one tag, or choose Every channel.".into(),
                });
            }
            UseIn::Tags { tags: tags.clone() }
        }
        UseInView::Channels(ids) => UseIn::Channels {
            channel_ids: ids
                .iter()
                .map(|id| {
                    id.parse::<ChannelId>().map_err(|_| {
                        ClientError::Other(
                            "A channel in this scope is not valid. Choose the channels again."
                                .into(),
                        )
                    })
                })
                .collect::<Result<_, _>>()?,
        },
    })
}

pub fn rule_view(rule: ApprovalRule) -> ApprovalRuleView {
    match rule {
        ApprovalRule::RunWithoutAsking => ApprovalRuleView::RunWithoutAsking,
        ApprovalRule::AskOncePerTask => ApprovalRuleView::AskOncePerTask,
        ApprovalRule::AskEveryTime => ApprovalRuleView::AskEveryTime,
    }
}

pub fn rule_proto(rule: ApprovalRuleView) -> ApprovalRule {
    match rule {
        ApprovalRuleView::RunWithoutAsking => ApprovalRule::RunWithoutAsking,
        ApprovalRuleView::AskOncePerTask => ApprovalRule::AskOncePerTask,
        ApprovalRuleView::AskEveryTime => ApprovalRule::AskEveryTime,
    }
}

pub fn approvals_view(a: &ToolApprovals) -> ToolApprovalsView {
    ToolApprovalsView {
        read: rule_view(a.read),
        change: rule_view(a.change),
        delete: rule_view(a.delete),
    }
}

pub fn approvals_proto(a: &ToolApprovalsView) -> ToolApprovals {
    ToolApprovals {
        read: rule_proto(a.read),
        change: rule_proto(a.change),
        delete: rule_proto(a.delete),
    }
}

pub fn kind_view(kind: ToolKind) -> ToolKindView {
    match kind {
        ToolKind::Read => ToolKindView::Read,
        ToolKind::Change => ToolKindView::Change,
        ToolKind::Delete => ToolKindView::Delete,
    }
}

pub fn auth_kind_view(kind: AuthSchemeKind) -> AuthKind {
    match kind {
        AuthSchemeKind::Oauth2 => AuthKind::OAuth2,
        AuthSchemeKind::Oauth1 => AuthKind::OAuth1,
        AuthSchemeKind::ApiKey => AuthKind::ApiKey,
        AuthSchemeKind::BearerToken => AuthKind::BearerToken,
        AuthSchemeKind::Basic => AuthKind::Basic,
        AuthSchemeKind::NoAuth => AuthKind::NoAuth,
        AuthSchemeKind::Other => AuthKind::Other,
    }
}

pub fn auth_kind_proto(kind: AuthKind) -> AuthSchemeKind {
    match kind {
        AuthKind::OAuth2 => AuthSchemeKind::Oauth2,
        AuthKind::OAuth1 => AuthSchemeKind::Oauth1,
        AuthKind::ApiKey => AuthSchemeKind::ApiKey,
        AuthKind::BearerToken => AuthSchemeKind::BearerToken,
        AuthKind::Basic => AuthSchemeKind::Basic,
        AuthKind::NoAuth => AuthSchemeKind::NoAuth,
        AuthKind::Other => AuthSchemeKind::Other,
    }
}

pub fn status_view(status: &ConnectionStatus) -> ConnectionStatusView {
    match status {
        ConnectionStatus::Connecting {
            redirect_url,
            expires_at,
        } => ConnectionStatusView::Connecting {
            redirect_url: redirect_url.clone().filter(|u| !u.is_empty()),
            expires_at: expires_at.as_millis(),
        },
        ConnectionStatus::Active => ConnectionStatusView::Active,
        ConnectionStatus::NeedsSignIn { message } => ConnectionStatusView::NeedsSignIn {
            message: message.clone(),
        },
        ConnectionStatus::Failed { message } => ConnectionStatusView::Failed {
            message: message.clone(),
        },
    }
}

/// The logo of a service, with its image when the client has loaded it.
pub fn logo(url: Option<&str>, name: &str, cache: &Cache) -> ServiceLogo {
    ServiceLogo {
        url: url.map(str::to_owned),
        image: url.and_then(|u| cache.extensions.logos.get(u).cloned()),
        monogram: monogram(name),
    }
}

pub fn connection_view(c: &model::Connection, cache: &Cache) -> ConnectionView {
    ConnectionView {
        id: c.id.to_string(),
        name: c.name.clone(),
        handle: c.handle.clone(),
        service: c.service.clone(),
        service_name: c.service_name.clone(),
        logo: logo(c.logo_url.as_deref(), &c.service_name, cache),
        identity: c.identity.clone(),
        use_in: use_in_view(&c.use_in),
        approvals: approvals_view(&c.approvals),
        status: status_view(&c.status),
        created_at: c.created_at.as_millis(),
    }
}

/// A catalog service as the dialog shows it. The logo image is filled in
/// when the view is built, see [`with_logo`].
pub fn service_view(s: &model::ConnectorService) -> ServiceView {
    ServiceView {
        slug: s.slug.clone(),
        name: s.name.clone(),
        description: s.description.clone(),
        logo: ServiceLogo {
            url: s.logo_url.clone(),
            image: None,
            monogram: monogram(&s.name),
        },
        categories: s.categories.clone(),
        tool_count: s.tool_count,
        auth_schemes: s
            .auth_schemes
            .iter()
            .map(|a| AuthSchemeView {
                kind: auth_kind_view(a.kind),
                managed: a.managed,
                fields: a
                    .fields
                    .iter()
                    .map(|f| CredentialFieldView {
                        name: f.name.clone(),
                        label: f.label.clone(),
                        description: f.description.clone(),
                        secret: f.secret,
                        required: f.required,
                    })
                    .collect(),
            })
            .collect(),
        connected_count: s.connected_count,
    }
}

/// The logo with the image that the client loaded for its URL.
pub fn with_logo(mut logo: ServiceLogo, cache: &Cache) -> ServiceLogo {
    if logo.image.is_none()
        && let Some(url) = &logo.url
    {
        logo.image = cache.extensions.logos.get(url).cloned();
    }
    logo
}

pub fn audit_view(a: &model::SkillAudit) -> AuditView {
    AuditView {
        source: a.source.clone(),
        status: match a.status {
            AuditStatus::Passed => AuditStatusView::Passed,
            AuditStatus::Warning => AuditStatusView::Warning,
            AuditStatus::Failed => AuditStatusView::Failed,
            AuditStatus::Unknown => AuditStatusView::Unknown,
        },
        summary: a.summary.clone(),
        url: a.url.clone(),
    }
}

pub fn directory_kind(kind: SkillDirectoryKind) -> DirectoryKind {
    match kind {
        SkillDirectoryKind::Smithery => DirectoryKind::Smithery,
        SkillDirectoryKind::Anthropic => DirectoryKind::Anthropic,
        SkillDirectoryKind::Marketplace => DirectoryKind::Marketplace,
        SkillDirectoryKind::SkillsSh => DirectoryKind::SkillsSh,
    }
}

pub fn source_view(source: &SkillSource) -> SkillSourceView {
    match source {
        SkillSource::Git { url, path, git_ref } => SkillSourceView::Git {
            url: url.clone(),
            path: path.clone(),
            git_ref: git_ref.clone(),
        },
        SkillSource::Directory {
            directory,
            entry_id,
            url,
            path,
        } => SkillSourceView::Directory {
            directory: directory_kind(*directory),
            entry_id: entry_id.clone(),
            url: url.clone(),
            path: path.clone(),
        },
        SkillSource::Upload { file_name } => SkillSourceView::Upload {
            file_name: file_name.clone(),
        },
    }
}

pub fn skill_view(s: &model::Skill, now: i64) -> SkillView {
    SkillView {
        id: s.id.to_string(),
        name: s.name.clone(),
        description: s.description.clone(),
        source: source_view(&s.source),
        commit: s.commit.as_deref().map(short_commit),
        license: s.license.clone(),
        scripts: s.scripts.clone(),
        audits: s.audits.iter().map(audit_view).collect(),
        enabled: s.enabled,
        use_in: use_in_view(&s.use_in),
        update: match &s.update {
            SkillUpdateState::UpToDate => SkillUpdateView::UpToDate,
            SkillUpdateState::Available {
                commit,
                changes,
                scripts_changed,
                ..
            } => SkillUpdateView::Available {
                commit: commit.as_deref().map(short_commit),
                changes: changes
                    .iter()
                    .map(|c| {
                        (
                            c.path.clone(),
                            match c.change {
                                SkillFileChangeKind::Added => FileChangeKind::Added,
                                SkillFileChangeKind::Modified => FileChangeKind::Modified,
                                SkillFileChangeKind::Removed => FileChangeKind::Removed,
                            },
                        )
                    })
                    .collect(),
                scripts_changed: *scripts_changed,
            },
            SkillUpdateState::CheckFailed { message } => SkillUpdateView::CheckFailed {
                message: message.clone(),
            },
        },
        blocked_reason: s.blocked_reason.clone(),
        hooks_removed: s.hooks_removed,
        file_count: s.file_count,
        size_bytes: s.size_bytes,
        installed_label: format::day_label(s.installed_at.as_millis(), now),
    }
}

pub fn directory_entry_view(e: &model::SkillDirectoryEntry) -> DirectoryEntryView {
    DirectoryEntryView {
        entry_id: e.entry_id.clone(),
        directory: directory_kind(e.directory),
        source_id: e.source_id.clone(),
        name: e.name.clone(),
        description: e.description.clone(),
        author: e.author.clone(),
        url: e.url.clone(),
        license: e.license.clone(),
        installs: e.installs,
        audits: e.audits.iter().map(audit_view).collect(),
        installed_skill_id: e.installed_skill_id.map(|i| i.to_string()),
    }
}

/// A skill zip that is on the hub: its hash and size, from the upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadedZip {
    pub sha256: consort_proto::model::Sha256Hash,
    pub size: u64,
}

/// The file name of a skill zip, as the user gave it.
pub fn zip_file_name(file_path: &str) -> String {
    std::path::Path::new(file_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| file_path.to_owned())
}

/// Refuses a skill zip that the hub would refuse, before the upload: a file
/// that is not a zip, or one over the skill limit (25 MB).
pub fn check_skill_zip(file_path: &str, size: u64) -> Result<(), ClientError> {
    let invalid = |message: String| ClientError::Hub {
        code: "invalid".into(),
        message,
    };
    if !zip_file_name(file_path)
        .to_ascii_lowercase()
        .ends_with(".zip")
    {
        return Err(invalid(
            "This file is not a zip file. Choose a zip file with a SKILL.md at its top.".into(),
        ));
    }
    let limit = consort_proto::model::SKILL_UPLOAD_MAX_BYTES;
    if size > limit {
        return Err(invalid(format!(
            "This zip file is {} MB. The limit for a skill is {} MB. Choose a smaller file.",
            size.div_ceil(1024 * 1024),
            limit / (1024 * 1024)
        )));
    }
    Ok(())
}

/// The source of a skill for `skill.preview` and `skill.install`. An upload
/// needs the blob that the client sent first (see
/// `hub::files::Shared::skill_source`).
pub fn install_source(
    input: &SkillInstallInput,
    uploaded: Option<&UploadedZip>,
) -> Result<SkillInstallSource, ClientError> {
    match input {
        SkillInstallInput::Directory { source, entry_id } => Ok(SkillInstallSource::Directory {
            source: source.clone(),
            entry_id: entry_id.clone(),
        }),
        SkillInstallInput::Git { url, path, git_ref } => {
            let url = url.trim().to_owned();
            if url.is_empty() {
                return Err(ClientError::Hub {
                    code: "invalid".into(),
                    message: "Enter a git URL, such as https://github.com/anthropics/skills."
                        .into(),
                });
            }
            Ok(SkillInstallSource::Git {
                url,
                path: path.clone().filter(|p| !p.trim().is_empty()),
                git_ref: git_ref.clone().filter(|r| !r.trim().is_empty()),
            })
        }
        SkillInstallInput::Upload { file_path } => match uploaded {
            Some(zip) => Ok(SkillInstallSource::Upload {
                blob_sha256: zip.sha256.clone(),
                size: zip.size,
                file_name: zip_file_name(file_path),
            }),
            None => Err(ClientError::Other(
                "Consort did not upload the zip file. Try again".into(),
            )),
        },
    }
}

/// The view of a hub card, and the words of its two buttons. `None` for an
/// approval from a harness.
pub fn hub_card(
    details: &ApprovalDetails,
    options: &[model::RequestOption],
    cache: &Cache,
) -> Option<(HubCardView, Vec<(ApprovalChoice, String)>)> {
    if !details.is_hub_card() {
        return None;
    }
    let card = match details {
        ApprovalDetails::Connector(card) => {
            let connection = cache.connections.get(&card.connection_id.to_string());
            let kind = kind_view(card.tool_kind);
            let rule = connection
                .map(|c| rule_view(c.approvals.rule_for(card.tool_kind)))
                .unwrap_or(ApprovalRuleView::AskEveryTime);
            HubCardView::Connector(ConnectorCardView {
                connection_id: card.connection_id.to_string(),
                connection_name: card.connection_name.clone(),
                service_name: card.service_name.clone(),
                logo: logo(
                    connection.and_then(|c| c.logo_url.as_deref()),
                    &card.service_name,
                    cache,
                ),
                tool: card.tool.clone(),
                kind,
                fields: card
                    .fields
                    .iter()
                    .map(|f| CardFieldView {
                        label: f.label.clone(),
                        value: f.value.clone(),
                    })
                    .collect(),
                rule_note: rule_note(kind, rule),
            })
        }
        ApprovalDetails::Management(card) => HubCardView::Management(ManagementCardView {
            tool: card.tool.clone(),
            change: card.change.clone(),
            effect: card.effect.clone(),
        }),
        ApprovalDetails::ConnectService(card) => HubCardView::ConnectService(ConnectCardView {
            service: card.service.clone(),
            service_name: card.service_name.clone(),
            logo: logo(card.logo_url.as_deref(), &card.service_name, cache),
            name: card.name.clone(),
            use_in: use_in_view(&card.use_in),
            approvals: approvals_view(&card.approvals),
            connection_id: card.connection_id.map(|c| c.to_string()),
        }),
        ApprovalDetails::SignIn(card) => {
            let (provider, logo) = provider_of(&card.harness);
            HubCardView::SignIn(SignInCardView {
                worker_id: card.worker_id.to_string(),
                account_id: card.account_id.to_string(),
                account_label: card.account_label.clone(),
                harness: card.harness.clone(),
                provider,
                logo: if matches!(logo, ProviderLogo::Registry { .. }) {
                    ProviderLogo::Generic
                } else {
                    logo
                },
                method: card.method.as_ref().map(crate::reduce::sign_in_choice),
                sign_in_id: card.sign_in_id.map(|s| s.to_string()),
            })
        }
        ApprovalDetails::SkillInstall(card) => HubCardView::SkillInstall(InstallCardView {
            name: card.name.clone(),
            description: card.description.clone(),
            source: source_view(&card.source).location(),
            commit: card.commit.as_deref().map(short_commit),
            license: card.license.clone(),
            scripts: card.scripts.clone(),
            audits: card.audits.iter().map(audit_view).collect(),
            use_in: use_in_view(&card.use_in),
            blocked_reason: card.blocked_reason.clone(),
            skill_id: card.skill_id.map(|s| s.to_string()),
        }),
        _ => return None,
    };
    // The labels come from the options the hub sent, so the buttons say
    // what the hub will do. The proto's labels fill in a missing one.
    let fallback = details.hub_card_options().unwrap_or_default();
    let mut labels = Vec::new();
    for option in options.iter().chain(fallback.iter()) {
        if let Some(choice) = crate::reduce::approval_choice(option.kind)
            && !labels.iter().any(|(c, _)| *c == choice)
        {
            labels.push((choice, option.label.clone()));
        }
    }
    Some((card, labels))
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::model::{DisplayField, ToolApprovals};

    #[test]
    fn scopes_and_rules_go_to_the_wire_and_back() {
        let id = ChannelId::new();
        for use_in in [
            UseIn::EveryChannel,
            UseIn::Tags {
                tags: vec!["work".into()],
            },
            UseIn::Channels {
                channel_ids: vec![id],
            },
        ] {
            assert_eq!(use_in_proto(&use_in_view(&use_in)).unwrap(), use_in);
        }
        assert!(use_in_proto(&UseInView::Tags(vec![])).is_err());
        assert!(use_in_proto(&UseInView::Channels(vec!["nope".into()])).is_err());
        let approvals = ToolApprovals::default();
        assert_eq!(approvals_proto(&approvals_view(&approvals)), approvals);
        assert_eq!(approvals_view(&approvals), ToolApprovalsView::default());
    }

    #[test]
    fn a_connector_card_has_its_fields_and_the_hub_labels() {
        let details = ApprovalDetails::Connector(Box::new(model::ConnectorCard {
            connection_id: consort_proto::ids::ConnectionId::new(),
            connection_name: "Gmail Work".into(),
            service_name: "Gmail".into(),
            tool: "GMAIL_SEND_EMAIL".into(),
            tool_kind: ToolKind::Change,
            fields: vec![DisplayField {
                label: "To".into(),
                value: "dana@acme.dev".into(),
            }],
            arguments: serde_json::json!({}),
        }));
        let options = details.hub_card_options().unwrap();
        let (card, labels) = hub_card(&details, &options, &Cache::default()).unwrap();
        let HubCardView::Connector(card) = card else {
            panic!("a connector card");
        };
        assert_eq!(card.fields[0].label, "To");
        assert_eq!(card.rule_note, "Create and change tools ask every time");
        assert_eq!(
            labels,
            vec![
                (ApprovalChoice::AllowOnce, "Allow".to_owned()),
                (ApprovalChoice::Deny, "Don't".to_owned())
            ]
        );
        let harness = ApprovalDetails::Other { text: None };
        assert!(hub_card(&harness, &[], &Cache::default()).is_none());
    }

    #[test]
    fn the_view_has_the_connections_in_order_with_their_logos() {
        use crate::test_entities::{connection, skill};
        let mut cache = Cache::default();
        let older = connection("Gmail Personal", UseIn::EveryChannel, 1);
        let newer = connection(
            "Gmail Work",
            UseIn::Tags {
                tags: vec!["work".into()],
            },
            5,
        );
        let url = newer.logo_url.clone().unwrap();
        cache.connections.insert(newer.id.to_string(), newer);
        cache.connections.insert(older.id.to_string(), older);
        let pdf = skill("pdf", 3);
        cache.skills.insert(pdf.id.to_string(), pdf);
        assert_eq!(cache.wanted_logos(), vec![url.clone()]);
        cache.extensions.logos_tried.insert(url.clone());
        assert!(cache.wanted_logos().is_empty(), "a logo is asked for once");
        cache.extensions.logos.insert(
            url,
            LogoImage {
                key: "k".into(),
                bytes: std::sync::Arc::new(b"<svg/>".to_vec()),
            },
        );
        cache.extensions.catalog.services = vec![ServiceView {
            slug: "gmail".into(),
            ..ServiceView::default()
        }];
        let state = crate::reduce::view_state(
            &cache,
            crate::view::Connection::Online,
            crate::view::UserView::default(),
            10,
        );
        let names: Vec<&str> = state.connections.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Gmail Personal", "Gmail Work"]);
        assert_eq!(state.connections[1].handle, "gmail_work");
        assert_eq!(
            state.connections[1].use_in,
            UseInView::Tags(vec!["work".into()])
        );
        assert!(state.connections[1].logo.image.is_some());
        assert_eq!(state.connections[1].logo.monogram, "G");
        assert_eq!(state.connector_catalog.services[0].connected_count, 2);
        assert_eq!(state.skills[0].name, "pdf");
        assert_eq!(state.skills[0].commit.as_deref(), Some("0123456"));
    }
}
