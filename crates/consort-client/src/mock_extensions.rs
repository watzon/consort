//! Mock content for connectors, skills and the hub cards, from the Paper
//! designs: Settings > Connectors and Skills, the dialogs, "Connectors in a
//! channel" and "Everything manages connectors and channels".

use std::sync::Arc;

use crate::{
    connectors::*,
    skills::*,
    view::{
        ApprovalCardView, ApprovalChoice, ApprovalView, MessageView, ProviderLogo, RequestState,
        SignInChoice, Speaker, TimelineItem,
    },
};

pub const ACME: &str = "chn_acme";
pub const HIRING: &str = "chn_hiring";

pub const CON_GMAIL_WORK: &str = "con_mock_gmail_work";
pub const CON_GMAIL_PERSONAL: &str = "con_mock_gmail_personal";
pub const CON_CALENDAR: &str = "con_mock_calendar";
pub const CON_GITHUB: &str = "con_mock_github";
pub const CON_LINEAR: &str = "con_mock_linear";
pub const CON_NOTION: &str = "con_mock_notion";

pub const SKL_RESEARCH: &str = "skl_mock_deep_research";
pub const SKL_PDF: &str = "skl_mock_pdf";
pub const SKL_CONTRACT: &str = "skl_mock_contract_review";
pub const SKL_RUST: &str = "skl_mock_rust";
pub const SKL_BROWSER: &str = "skl_mock_browser";

/// The approval IDs of the mock cards.
pub const APR_SEND_EMAIL: &str = "apr_mock_send_email";
pub const APR_CONNECT_GITHUB: &str = "apr_mock_connect_github";
pub const APR_SIGN_IN: &str = "apr_mock_sign_in";
pub const APR_ACCESS: &str = "apr_mock_access";
pub const APR_INSTALL: &str = "apr_mock_install";
pub const APR_CONNECT_DRIVE: &str = "apr_mock_connect_drive";

fn logo_file(slug: &str) -> Option<&'static [u8]> {
    Some(match slug {
        "gmail" => include_bytes!("../fixtures/service-logos/gmail.svg"),
        "googlecalendar" => include_bytes!("../fixtures/service-logos/googlecalendar.svg"),
        "googledrive" => include_bytes!("../fixtures/service-logos/googledrive.svg"),
        "github" => include_bytes!("../fixtures/service-logos/github.svg"),
        "linear" => include_bytes!("../fixtures/service-logos/linear.svg"),
        "notion" => include_bytes!("../fixtures/service-logos/notion.svg"),
        "jira" => include_bytes!("../fixtures/service-logos/jira.svg"),
        "discord" => include_bytes!("../fixtures/service-logos/discord.svg"),
        "todoist" => include_bytes!("../fixtures/service-logos/todoist.svg"),
        "stripe" => include_bytes!("../fixtures/service-logos/stripe.svg"),
        "hubspot" => include_bytes!("../fixtures/service-logos/hubspot.svg"),
        _ => return None,
    })
}

/// The logo of a mock service, from the local copies of the Composio logos.
pub fn logo(slug: &str, name: &str) -> ServiceLogo {
    ServiceLogo {
        url: Some(format!("https://logos.composio.dev/api/{slug}")),
        image: logo_file(slug).map(|bytes| LogoImage {
            key: format!("mock-{slug}"),
            bytes: Arc::new(bytes.to_vec()),
        }),
        monogram: monogram(name),
    }
}

fn tags(t: &[&str]) -> UseInView {
    UseInView::Tags(t.iter().map(|t| (*t).to_owned()).collect())
}

fn connection(
    id: &str,
    name: &str,
    service: (&str, &str),
    identity: &str,
    use_in: UseInView,
    status: ConnectionStatusView,
    at: i64,
) -> ConnectionView {
    ConnectionView {
        id: id.into(),
        name: name.into(),
        handle: consort_proto::model::connection_handle(name),
        service: service.0.into(),
        service_name: service.1.into(),
        logo: logo(service.0, service.1),
        identity: Some(identity.into()).filter(|i: &String| !i.is_empty()),
        use_in,
        approvals: ToolApprovalsView::default(),
        status,
        created_at: at,
    }
}

pub fn connections() -> Vec<ConnectionView> {
    let active = ConnectionStatusView::Active;
    vec![
        connection(
            CON_GMAIL_WORK,
            "Gmail Work",
            ("gmail", "Gmail"),
            "chris@acme.dev",
            tags(&["work"]),
            active.clone(),
            1,
        ),
        connection(
            CON_GMAIL_PERSONAL,
            "Gmail Personal",
            ("gmail", "Gmail"),
            "cawatson1993@gmail.com",
            UseInView::EveryChannel,
            active.clone(),
            2,
        ),
        connection(
            CON_CALENDAR,
            "Calendar",
            ("googlecalendar", "Google Calendar"),
            "cawatson1993@gmail.com",
            UseInView::EveryChannel,
            active.clone(),
            3,
        ),
        connection(
            CON_GITHUB,
            "GitHub",
            ("github", "GitHub"),
            "watzon",
            UseInView::EveryChannel,
            active.clone(),
            4,
        ),
        connection(
            CON_LINEAR,
            "Linear",
            ("linear", "Linear"),
            "chris@acme.dev",
            UseInView::EveryChannel,
            ConnectionStatusView::NeedsSignIn {
                message: Some("The Linear token was revoked on Oct 28.".into()),
            },
            5,
        ),
        connection(
            CON_NOTION,
            "Notion",
            ("notion", "Notion"),
            "Chris's workspace",
            tags(&["personal"]),
            active,
            6,
        ),
    ]
}

pub fn composio() -> ComposioView {
    ComposioView {
        load: LoadState::Loaded,
        key_present: true,
        key_hint: Some("…k3Fq".into()),
        plan: Some("Hobby".into()),
        used: Some(12_340),
        limit: Some(100_000),
        error: None,
    }
}

pub fn categories() -> Vec<CategoryView> {
    [
        ("email", "Email and chat"),
        ("developer", "Developer tools"),
        ("productivity", "Productivity"),
        ("crm", "Sales and CRM"),
        ("finance", "Finance"),
    ]
    .iter()
    .map(|(slug, name)| CategoryView {
        slug: (*slug).into(),
        name: (*name).into(),
    })
    .collect()
}

fn oauth() -> Vec<AuthSchemeView> {
    vec![AuthSchemeView {
        kind: AuthKind::OAuth2,
        managed: true,
        fields: Vec::new(),
    }]
}

fn api_key(label: &str) -> Vec<AuthSchemeView> {
    vec![AuthSchemeView {
        kind: AuthKind::ApiKey,
        managed: false,
        fields: vec![CredentialFieldView {
            name: "api_key".into(),
            label: label.into(),
            description: Some("Make a key in the service's settings, then paste it here.".into()),
            secret: true,
            required: true,
        }],
    }]
}

/// Every service of the mock catalog, in the order of "Popular".
pub fn catalog_services() -> Vec<ServiceView> {
    let s = |slug: &str,
             name: &str,
             description: &str,
             cat: &str,
             tools: u32,
             auth: Vec<AuthSchemeView>| {
        ServiceView {
            slug: slug.into(),
            name: name.into(),
            description: description.into(),
            logo: logo(slug, name),
            categories: vec![cat.into()],
            tool_count: tools,
            auth_schemes: auth,
            connected_count: 0,
        }
    };
    vec![
        s(
            "gmail",
            "Gmail",
            "Read, search, draft and send mail.",
            "email",
            62,
            oauth(),
        ),
        s(
            "googlecalendar",
            "Google Calendar",
            "Find free time and manage events.",
            "productivity",
            28,
            oauth(),
        ),
        s(
            "googledrive",
            "Google Drive",
            "Find, read and share files.",
            "productivity",
            41,
            oauth(),
        ),
        s(
            "github",
            "GitHub",
            "Issues, pull requests, repos and actions.",
            "developer",
            940,
            oauth(),
        ),
        s(
            "notion",
            "Notion",
            "Pages, databases and comments.",
            "productivity",
            35,
            oauth(),
        ),
        s(
            "jira",
            "Jira",
            "Issues, sprints and boards.",
            "developer",
            120,
            oauth(),
        ),
        s(
            "discord",
            "Discord",
            "Servers, channels and messages.",
            "email",
            54,
            oauth(),
        ),
        s(
            "todoist",
            "Todoist",
            "Tasks, projects and due dates.",
            "productivity",
            22,
            api_key("API token"),
        ),
        s(
            "stripe",
            "Stripe",
            "Customers, invoices and payments.",
            "finance",
            76,
            api_key("Secret key"),
        ),
        s(
            "linear",
            "Linear",
            "Issues, projects and cycles.",
            "developer",
            48,
            oauth(),
        ),
        s(
            "hubspot",
            "HubSpot",
            "Contacts, deals and companies.",
            "crm",
            150,
            oauth(),
        ),
    ]
}

/// The size of one page of the mock catalog.
pub const CATALOG_PAGE: usize = 9;

/// The tools of a mock service: as many of each kind as the design shows
/// for Gmail, and a few for the others.
pub fn service_tools(service: &str) -> ServiceToolsView {
    let (read, change, delete) = match service {
        "gmail" => (31, 27, 4),
        "github" => (12, 9, 3),
        _ => (6, 4, 1),
    };
    let upper = service.to_uppercase();
    let mut tools = Vec::new();
    for (kind, n, word) in [
        (ToolKindView::Read, read, "GET"),
        (ToolKindView::Change, change, "UPDATE"),
        (ToolKindView::Delete, delete, "DELETE"),
    ] {
        for i in 0..n {
            tools.push(ConnectorToolView {
                slug: format!("{upper}_{word}_{i}"),
                description: String::new(),
                kind,
            });
        }
    }
    ServiceToolsView {
        service: service.into(),
        tools,
        load: LoadState::Loaded,
    }
}

fn audit(source: &str, status: AuditStatusView) -> AuditView {
    AuditView {
        source: source.into(),
        status,
        summary: None,
        url: None,
    }
}

fn anthropic(entry: &str) -> SkillSourceView {
    SkillSourceView::Directory {
        directory: DirectoryKind::Anthropic,
        entry_id: entry.into(),
        url: "https://github.com/anthropics/skills".into(),
        path: Some(entry.into()),
    }
}

pub fn skills() -> Vec<SkillView> {
    vec![
        SkillView {
            id: SKL_RESEARCH.into(),
            name: "deep-research".into(),
            description: "Research a topic across many sources and write a cited report.".into(),
            source: anthropic("deep-research"),
            commit: Some("3f2a91c".into()),
            license: Some("Apache-2.0".into()),
            scripts: vec!["scripts/fetch.py".into(), "scripts/cite.py".into()],
            audits: vec![
                audit("Socket", AuditStatusView::Passed),
                audit("Snyk", AuditStatusView::Passed),
                audit("Gen Agent Trust Hub", AuditStatusView::Passed),
            ],
            enabled: true,
            use_in: UseInView::EveryChannel,
            file_count: 4,
            size_bytes: 18_000,
            installed_label: "Oct 29".into(),
            ..SkillView::default()
        },
        SkillView {
            id: SKL_PDF.into(),
            name: "pdf".into(),
            description: "Read, fill and create PDF files.".into(),
            source: anthropic("pdf"),
            commit: Some("3f2a91c".into()),
            license: Some("Apache-2.0".into()),
            audits: vec![audit("Socket", AuditStatusView::Passed)],
            enabled: true,
            use_in: UseInView::EveryChannel,
            file_count: 6,
            installed_label: "Oct 29".into(),
            ..SkillView::default()
        },
        SkillView {
            id: SKL_CONTRACT.into(),
            name: "contract-review".into(),
            description: "Check a contract against the Acme playbook.".into(),
            source: SkillSourceView::Git {
                url: "https://github.com/watzon/acme-skills".into(),
                path: Some("contract-review".into()),
                git_ref: None,
            },
            commit: Some("8c41d02".into()),
            license: Some("Proprietary".into()),
            enabled: true,
            use_in: tags(&["work"]),
            file_count: 3,
            installed_label: "Oct 28".into(),
            ..SkillView::default()
        },
        SkillView {
            id: SKL_RUST.into(),
            name: "rust-best-practices".into(),
            description: "Idiomatic Rust: errors, ownership, tests and clippy rules.".into(),
            source: SkillSourceView::Directory {
                directory: DirectoryKind::Smithery,
                entry_id: "rust-best-practices".into(),
                url: "https://github.com/smithery-ai/rust-skills".into(),
                path: Some("rust-best-practices".into()),
            },
            commit: Some("b71e0aa".into()),
            license: Some("MIT".into()),
            audits: vec![
                audit("Socket", AuditStatusView::Passed),
                audit("Snyk", AuditStatusView::Passed),
            ],
            enabled: true,
            use_in: UseInView::EveryChannel,
            update: SkillUpdateView::Available {
                commit: Some("c90f4d2".into()),
                changes: vec![
                    ("SKILL.md".into(), FileChangeKind::Modified),
                    ("reference/clippy.md".into(), FileChangeKind::Added),
                ],
                scripts_changed: false,
            },
            file_count: 5,
            installed_label: "Oct 20".into(),
            ..SkillView::default()
        },
        SkillView {
            id: SKL_BROWSER.into(),
            name: "browser-automation".into(),
            description: "Drive a headless browser to fill forms and take screenshots.".into(),
            source: SkillSourceView::Directory {
                directory: DirectoryKind::Smithery,
                entry_id: "browser-automation".into(),
                url: "https://github.com/smithery-ai/browser-skills".into(),
                path: None,
            },
            commit: Some("41aa9be".into()),
            license: Some("MIT".into()),
            scripts: vec!["scripts/browse.js".into()],
            audits: vec![audit("Socket", AuditStatusView::Warning)],
            enabled: false,
            use_in: UseInView::EveryChannel,
            file_count: 7,
            installed_label: "Today".into(),
            ..SkillView::default()
        },
    ]
}

/// Every entry of the mock skill directories.
pub fn directory_entries() -> Vec<DirectoryEntryView> {
    let e = |name: &str,
             author: &str,
             description: &str,
             audits: Vec<AuditView>,
             installed: Option<&str>| {
        DirectoryEntryView {
            entry_id: name.into(),
            directory: DirectoryKind::Smithery,
            source_id: Some("smithery".into()),
            name: name.into(),
            description: description.into(),
            author: Some(author.into()),
            url: Some(format!("https://smithery.ai/skills/{name}")),
            license: Some("MIT".into()),
            installs: Some(1_200),
            audits,
            installed_skill_id: installed.map(Into::into),
        }
    };
    vec![
        e(
            "rust-best-practices",
            "Smithery, verified",
            "Idiomatic Rust: errors, ownership, tests and clippy rules.",
            vec![
                audit("Socket", AuditStatusView::Passed),
                audit("Snyk", AuditStatusView::Passed),
            ],
            Some(SKL_RUST),
        ),
        e(
            "rust-async-patterns",
            "Smithery",
            "Tokio tasks, cancellation and backpressure.",
            Vec::new(),
            None,
        ),
        e(
            "crate-release",
            "Smithery",
            "Bump versions, write the changelog and publish to crates.io.",
            vec![audit("Socket", AuditStatusView::Warning)],
            None,
        ),
        e(
            "rust-unsafe-helper",
            "Smithery",
            "Write unsafe blocks for FFI.",
            vec![audit("Socket", AuditStatusView::Failed)],
            None,
        ),
        e(
            "deep-research",
            "Anthropic",
            "Research a topic across many sources and write a cited report.",
            vec![audit("Socket", AuditStatusView::Passed)],
            Some(SKL_RESEARCH),
        ),
    ]
}

pub fn directory_sources() -> Vec<DirectorySourceView> {
    vec![
        DirectorySourceView {
            id: "smithery".into(),
            kind: DirectoryKind::Smithery,
            name: "Smithery".into(),
            url: "https://smithery.ai".into(),
            removable: false,
        },
        DirectorySourceView {
            id: "anthropic".into(),
            kind: DirectoryKind::Anthropic,
            name: "Anthropic's skills".into(),
            url: "https://github.com/anthropics/skills".into(),
            removable: false,
        },
    ]
}

/// What the Claude Personal account on bigboi brings from claude.ai.
pub fn claude_extras() -> ProviderExtrasView {
    ProviderExtrasView {
        mcp_servers: ExtrasGroupView {
            count: 19,
            names: [
                "Gmail",
                "Gmail",
                "Gmail",
                "Google Calendar",
                "Google Calendar",
                "Google Drive",
                "Google Drive",
                "Google Drive",
                "Linear",
                "Slack",
                "Miss Lucy",
            ]
            .iter()
            .map(|n| (*n).to_owned())
            .collect(),
        },
        plugins: ExtrasGroupView {
            count: 2,
            names: Vec::new(),
        },
        skills: ExtrasGroupView {
            count: 6,
            names: Vec::new(),
        },
        cannot_turn_off: vec!["Plugins that you installed by hand in this profile".into()],
    }
}

fn card(
    id: &str,
    task: Option<&str>,
    summary: &str,
    card: HubCardView,
    labels: (&str, &str),
) -> ApprovalView {
    ApprovalView {
        id: id.into(),
        task_id: task.map(Into::into),
        summary: summary.into(),
        folder: None,
        detail: None,
        choices: vec![ApprovalChoice::AllowOnce, ApprovalChoice::Deny],
        state: RequestState::Pending,
        answer: None,
        allow_always_label: None,
        card: Some(Box::new(card)),
        button_labels: vec![
            (ApprovalChoice::AllowOnce, labels.0.into()),
            (ApprovalChoice::Deny, labels.1.into()),
        ],
    }
}

fn card_item(message_id: &str, time: &str, approval: ApprovalView) -> TimelineItem {
    TimelineItem::Approval(ApprovalCardView {
        message_id: message_id.into(),
        speaker: Speaker::Consort,
        time_label: time.into(),
        created_at: 0,
        approval,
    })
}

fn message(id: &str, channel: &str, speaker: Speaker, time: &str, body: &str) -> TimelineItem {
    TimelineItem::Message(MessageView {
        id: id.into(),
        channel_id: channel.into(),
        speaker,
        time_label: time.into(),
        body: body.into(),
        ..MessageView::default()
    })
}

/// The acme timeline of "Connectors in a channel": a reply that waits for
/// the connector approval card.
pub fn acme_items() -> Vec<TimelineItem> {
    vec![
        message(
            "msg_acme_ask",
            ACME,
            Speaker::You,
            "2:40 PM",
            "Reply to Dana about the contract renewal and offer Thursday afternoon for a call.",
        ),
        message(
            "msg_acme_reply",
            ACME,
            Speaker::Consort,
            "2:40 PM",
            "Dana asked for the renewal terms by Friday. I wrote a reply that confirms the terms and offers Thursday from 2 to 5 PM. It needs your OK before it goes out.",
        ),
        card_item(
            "msg_acme_card",
            "2:40 PM",
            card(
                APR_SEND_EMAIL,
                None,
                "Send an email with Gmail Work",
                HubCardView::Connector(ConnectorCardView {
                    connection_id: CON_GMAIL_WORK.into(),
                    connection_name: "Gmail Work".into(),
                    service_name: "Gmail".into(),
                    logo: logo("gmail", "Gmail"),
                    tool: "GMAIL_SEND_EMAIL".into(),
                    kind: ToolKindView::Change,
                    fields: vec![
                        CardFieldView {
                            label: "To".into(),
                            value: "Dana Ortiz <dana@acme.dev>".into(),
                        },
                        CardFieldView {
                            label: "Subject".into(),
                            value: "Re: Contract renewal".into(),
                        },
                        CardFieldView {
                            label: "Body".into(),
                            value: "Hi Dana, the renewal terms from your note work for us. Could we talk on Thursday? Any time from 2 to 5 PM is free on my side.".into(),
                        },
                    ],
                    rule_note: rule_note(ToolKindView::Change, ApprovalRuleView::AskEveryTime),
                }),
                ("Allow", "Don't"),
            ),
        ),
    ]
}

/// The hiring timeline: one of each other hub card. The channel's assistant
/// asks for access to a connector and offers a skill. The sign-in card and
/// a second connect card are here too, so `--mock` shows every card kind.
pub fn hiring_items() -> Vec<TimelineItem> {
    vec![
        message(
            "msg_hiring_ask",
            HIRING,
            Speaker::You,
            "3:10 PM",
            "Use the contract-review skill and my Notion notes for the offer letters.",
        ),
        card_item(
            "msg_hiring_access",
            "3:10 PM",
            card(
                APR_ACCESS,
                None,
                "Give hiring access to Notion",
                HubCardView::Management(ManagementCardView {
                    tool: "request_access".into(),
                    change: "Use Notion in channels tagged personal or work".into(),
                    effect: "Notion becomes available in 3 more channels: acme, hiring and Tutti."
                        .into(),
                }),
                ("Allow", "Don't"),
            ),
        ),
        card_item(
            "msg_hiring_install",
            "3:11 PM",
            card(
                APR_INSTALL,
                None,
                "Add the offer-letters skill",
                HubCardView::SkillInstall(InstallCardView {
                    name: "offer-letters".into(),
                    description: "Write offer letters from the Acme templates.".into(),
                    source: "github.com/watzon/acme-skills".into(),
                    commit: Some("5d0e7c1".into()),
                    license: Some("Proprietary".into()),
                    scripts: vec!["scripts/render.py".into()],
                    audits: Vec::new(),
                    use_in: tags(&["work"]),
                    blocked_reason: None,
                    skill_id: None,
                }),
                ("Add", "Not now"),
            ),
        ),
        card_item(
            "msg_hiring_sign_in",
            "3:12 PM",
            card(
                APR_SIGN_IN,
                None,
                "Sign in to Claude Work on bigboi",
                HubCardView::SignIn(SignInCardView {
                    worker_id: "wrk_bigboi".into(),
                    account_id: "acc_bigboi_claude_work".into(),
                    account_label: "Claude Work".into(),
                    harness: "claude".into(),
                    provider: "Claude Code".into(),
                    logo: ProviderLogo::ClaudeCode,
                    method: Some(SignInChoice::ClaudePlan),
                    sign_in_id: None,
                }),
                ("Sign in", "Not now"),
            ),
        ),
        card_item(
            "msg_hiring_connect",
            "3:12 PM",
            card(
                APR_CONNECT_DRIVE,
                None,
                "Connect Google Drive as Drive Work",
                HubCardView::ConnectService(ConnectCardView {
                    service: "googledrive".into(),
                    service_name: "Google Drive".into(),
                    logo: logo("googledrive", "Google Drive"),
                    name: "Drive Work".into(),
                    use_in: UseInView::Channels(vec![HIRING.into()]),
                    approvals: ToolApprovalsView::default(),
                    connection_id: None,
                }),
                ("Continue in browser", "Not now"),
            ),
        ),
    ]
}

/// The end of the Everything timeline of "Everything manages connectors and
/// channels": the request and the connect card. Only `--screen
/// everything-manages` shows them, so the usual Everything stays as designed.
pub fn everything_items() -> Vec<TimelineItem> {
    vec![
        message(
            "msg_everything_hiring",
            crate::mock::everything_id(),
            Speaker::You,
            "3:05 PM",
            "Set up a hiring channel for work, and connect my work GitHub so it can see our repos there.",
        ),
        message(
            "msg_everything_ready",
            crate::mock::everything_id(),
            Speaker::Consort,
            "3:05 PM",
            "The hiring channel is ready, and its work runs only on your work machines. For GitHub, sign in with your work account below. I'll make it available only in channels tagged work, so it never reaches your personal channels.",
        ),
        card_item(
            "msg_everything_connect",
            "3:05 PM",
            card(
                APR_CONNECT_GITHUB,
                None,
                "Connect GitHub as GitHub Work",
                HubCardView::ConnectService(ConnectCardView {
                    service: "github".into(),
                    service_name: "GitHub".into(),
                    logo: logo("github", "GitHub"),
                    name: "GitHub Work".into(),
                    use_in: tags(&["work"]),
                    approvals: ToolApprovalsView::default(),
                    connection_id: None,
                }),
                ("Continue in browser", "Not now"),
            ),
        ),
    ]
}
