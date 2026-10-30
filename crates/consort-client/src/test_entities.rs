//! Hub entities for the tests of the cache and the reducers.

use consort_proto::{
    ids::{ConnectionId, SkillId},
    model::{
        Connection, ConnectionStatus, Skill, SkillSource, SkillUpdateState, ToolApprovals, UseIn,
    },
    time::Timestamp,
};

pub(crate) fn connection(name: &str, use_in: UseIn, at: i64) -> Connection {
    Connection {
        id: ConnectionId::new(),
        name: name.into(),
        handle: consort_proto::model::connection_handle(name),
        service: "gmail".into(),
        service_name: "Gmail".into(),
        logo_url: Some("https://logos.example/gmail.svg".into()),
        identity: Some("chris@work.example".into()),
        use_in,
        approvals: ToolApprovals::default(),
        status: ConnectionStatus::Active,
        last_used_at: None,
        created_at: Timestamp::from_millis(at),
        updated_at: Timestamp::from_millis(at),
    }
}

pub(crate) fn skill(name: &str, at: i64) -> Skill {
    Skill {
        id: SkillId::new(),
        name: name.into(),
        description: "Fill and read PDF forms.".into(),
        source: SkillSource::Git {
            url: "https://github.com/anthropics/skills".into(),
            path: Some("document-skills/pdf".into()),
            git_ref: None,
        },
        commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
        content_hash: "a".repeat(64).parse().expect("a hash"),
        license: Some("Apache-2.0".into()),
        scripts: Vec::new(),
        audits: Vec::new(),
        enabled: true,
        use_in: UseIn::EveryChannel,
        update: SkillUpdateState::UpToDate,
        blocked_reason: None,
        hooks_removed: false,
        file_count: 2,
        size_bytes: 20,
        installed_at: Timestamp::from_millis(at),
        updated_at: Timestamp::from_millis(at),
    }
}

/// A plain channel with the given ID and name.
pub(crate) fn channel(
    id: consort_proto::ids::ChannelId,
    name: &str,
) -> consort_proto::model::Channel {
    use consort_proto::model::{ApprovalMode, Channel, ChannelKind, Isolation};
    Channel {
        id,
        kind: ChannelKind::Standard,
        name: name.into(),
        icon: None,
        color: None,
        group_id: None,
        position: 0,
        purpose: String::new(),
        instructions: String::new(),
        coordinator_selection: None,
        task_selection: None,
        preferred_workers: vec![],
        repo_ids: vec![],
        default_isolation: Isolation::Worktree,
        default_approval_mode: ApprovalMode::AutoEdits,
        allowed_local_mcp_servers: vec![],
        memory_enabled: true,
        disabled_connections: Vec::new(),
        disabled_skills: Vec::new(),
        archived_at: None,
        created_at: Timestamp::from_millis(1),
        updated_at: Timestamp::from_millis(1),
        tags: Vec::new(),
    }
}
