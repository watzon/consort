//! Fixed content that reproduces the approved designs, so the app can be
//! shown without a hub (`consort-desktop --mock`). The mock also answers
//! commands, so buttons do what they will do against a real hub.

use crate::{
    connectors::CatalogView,
    derive::{refresh, waiting_detail},
    skills::SkillDirectoryView,
    view::*,
};

const TUTTI_ID: &str = "chn_tutti";
const SAYSO: &str = "chn_sayso";
const LUCY: &str = "chn_miss_lucy_tg";
const EXT4: &str = "chn_mac_ext4";
const HOMELAB: &str = "chn_homelab";
const LIGHTS: &str = "chn_house_lights";

const T_SAYSO: &str = "tsk_0192f0c4-sayso-issues";
const T_SPAM: &str = "tsk_0192f0c4-spam-rules";
const T_BACKUPS: &str = "tsk_0192f0c4-nightly-backups";
const T_ROOF: &str = "tsk_0192f0c4-roofline";
const T_EXT4: &str = "tsk_0192f0c4-ext4-build";
const T_CRASH: &str = "tsk_0192f0c4-crash-reports";
const T_CERT: &str = "tsk_0192f0c4-tls-cert";
const T_TOKEN: &str = "tsk_0192f0c4-bot-token";
const T_REPO: &str = "tsk_0192f0c4-sayso-repo";

/// What the spam-rules task said when it went to Ready for review. The card
/// shows its first lines, and the thread shows it whole.
const SPAM_SUMMARY: &str = "The new rules are in `rules/new_members.toml`.\n\n```toml\n[new_members]\nhold_first_link = true\nwindow_minutes = 1440\n```\n\nTests pass, and a dry run on last week's messages flags 41 of 43 spam posts and no real ones. Members who joined more than a day ago keep the current rules, so nothing changes for them.\n\n## What to check\n\n- Read the dry run in `reports/dry-run-2026-10-22.md`.\n- Look at the two posts that the rules missed. Both are in `reports/missed.md`.\n- Decide if `window_minutes = 1440` is the right window. A shorter window holds fewer first messages, and lets more spam through.";

/// The brief of the spam-rules task.
const SPAM_BRIEF: &str = "New accounts keep posting crypto links in the first minute. Tune the spam rules for new members.\n\n- Hold a first message with a link for review.\n- Keep the current rules for members who joined more than a day ago.\n- Run the new rules on last week's messages before you finish, and report how many spam posts they catch and how many real posts they flag.\n\nDo not change the rules for admins.";

/// A fixed "now" for the mock, so ages and times match the designs.
pub const MOCK_NOW: i64 = 1_793_300_000_000;

fn text(s: &str) -> String {
    s.to_owned()
}

fn channel(id: &str, name: &str, purpose: &str, icon: &str, group: Option<&str>) -> ChannelView {
    ChannelView {
        id: id.into(),
        kind: if id == TUTTI_ID {
            ChannelKind::Tutti
        } else {
            ChannelKind::Standard
        },
        name: name.into(),
        purpose: purpose.into(),
        instructions: String::new(),
        icon: icon.into(),
        group_id: group.map(Into::into),
        coordinator_worker: Some(text("hub")),
        coordinator_selection: Some(EffectiveSelection {
            selection: assistant_model(),
            source: if id == TUTTI_ID {
                SelectionSource::Channel
            } else {
                SelectionSource::Tutti
            },
        }),
        repos: Vec::new(),
        preferred_workers: Vec::new(),
        task_selection: Some(selection("Claude Personal", "claude-opus-5-5", "Opus 5.5")),
        default_isolation: Isolation::Worktree,
        tags: Vec::new(),
        disabled_connections: Vec::new(),
        disabled_skills: Vec::new(),
        default_approval_mode: ApprovalMode::AutoEdits,
        allowed_local_mcp_servers: Vec::new(),
        memory_enabled: true,
    }
}

#[allow(clippy::too_many_arguments)]
fn task(
    id: &str,
    channel_id: &str,
    channel_name: &str,
    title: &str,
    status: TaskStatus,
    place: (Option<&str>, Option<&str>, Option<&str>),
    age: &str,
    minutes_ago: i64,
) -> TaskView {
    let (worker, account, reason) = place;
    TaskView {
        id: id.into(),
        channel_id: channel_id.into(),
        channel_name: channel_name.into(),
        title: title.into(),
        status,
        meta: String::new(),
        worker_name: worker.map(Into::into),
        account_label: account.map(Into::into),
        status_reason: reason.map(Into::into),
        age_label: age.into(),
        updated_at: MOCK_NOW - minutes_ago * 60_000,
        card_message_id: format!("msg_card_{id}"),
        approval_mode: match id {
            T_SAYSO | T_EXT4 => ApprovalMode::Ask,
            T_ROOF => ApprovalMode::Auto,
            _ => ApprovalMode::AutoEdits,
        },
        session_ended: false,
    }
}

fn day(key: &str, label: &str) -> TimelineItem {
    TimelineItem::Day(DayView {
        key: key.into(),
        label: label.into(),
    })
}

fn message(id: &str, channel_id: &str, speaker: Speaker, time: &str, body: &str) -> TimelineItem {
    TimelineItem::Message(MessageView {
        id: id.into(),
        channel_id: channel_id.into(),
        speaker,
        time_label: time.into(),
        body: body.into(),
        ..MessageView::default()
    })
}

fn sayso_approval() -> ApprovalView {
    ApprovalView {
        id: text("apr_0192f0c4-windows-build"),
        task_id: Some(T_SAYSO.into()),
        summary: text("Codex wants to run a command"),
        folder: Some(text(r"C:\Code\sayso")),
        detail: Some(text(
            "cargo build --release --target x86_64-pc-windows-msvc",
        )),
        choices: vec![
            ApprovalChoice::AllowOnce,
            ApprovalChoice::AllowForTask,
            ApprovalChoice::Deny,
        ],
        state: RequestState::Pending,
        answer: None,
        allow_always_label: Some(text("Yes, and don't ask again for cargo build commands")),
        card: None,
        button_labels: Vec::new(),
    }
}

fn sayso_decision() -> DecisionView {
    DecisionView {
        id: text("dec_0192f0c4-cpu-fallback"),
        task_id: Some(T_SAYSO.into()),
        question: text("Ship the CPU fallback to everyone in 0.9.3, or behind a setting first?"),
        context: Some(text(
            "No one has run the new builds on a real pre-AVX2 machine yet.",
        )),
        options: vec![
            DecisionOption {
                label: text("Behind a setting first"),
                consequence: text(
                    "Only users who turn it on get the new engine. Most affected users will not find the setting.",
                ),
            },
            DecisionOption {
                label: text("Ship to everyone"),
                consequence: text(
                    "Users on older CPUs get a working build today. A bad build reaches every Windows user.",
                ),
            },
            DecisionOption {
                label: text("Wait for a hardware test"),
                consequence: text(
                    "Nothing ships until someone runs it on a real pre-AVX2 machine. That can take a week.",
                ),
            },
        ],
        recommended: Some(0),
        allow_free_text: true,
        state: RequestState::Pending,
        chosen: None,
        answer_text: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn card(
    message_id: &str,
    task_id: &str,
    time: &str,
    channel_name: Option<&str>,
    worker: Option<(&str, bool, &str)>,
    approval: Option<ApprovalView>,
    summary: &str,
    latest: Option<&str>,
) -> TimelineItem {
    TimelineItem::TaskCard(TaskCardView {
        message_id: message_id.into(),
        task_id: task_id.into(),
        time_label: time.into(),
        created_at: 0,
        title: String::new(),
        status: TaskStatus::default(),
        channel_name: channel_name.map(Into::into),
        worker: worker.map(|(name, online, account)| WorkerChip {
            name: name.into(),
            online,
            account: account.into(),
        }),
        approval,
        summary: summary.into(),
        reply_count: 0,
        latest: latest.map(Into::into),
        approval_mode: ApprovalMode::default(),
    })
}

/// A model selection with no options.
/// The model of the assistant in Tutti: Opus 5.5 with high effort.
fn assistant_model() -> SelectionView {
    let mut chosen = selection("Claude Personal", "claude-opus-5-5", "Opus 5.5");
    chosen.options = vec![(
        crate::picker::EFFORT.to_owned(),
        SelectionOption::Text(text("high")),
    )];
    chosen
}

/// Effort levels, with the provider's default.
fn levels(values: &[(&str, &str)], default: Option<&str>) -> Option<EffortView> {
    Some(EffortView {
        levels: values
            .iter()
            .map(|(value, label)| EffortLevel {
                value: text(value),
                label: text(label),
            })
            .collect(),
        default: default.map(text),
    })
}

/// The favorite models of the mock: one from each account, and one of an
/// account that no worker has now, which the picker hides.
pub fn favorite_models() -> Vec<FavoriteModelView> {
    [
        ("Claude Personal", "claude-opus-5-5"),
        ("Claude Personal", "claude-sonnet-5-5"),
        ("Claude Work", "claude-opus-5-5"),
        ("Claude Old", "claude-opus-4-5"),
        ("Codex Personal", "gpt-6.1-sol"),
        ("OpenCode with Fireworks", "fireworks/glm-5p3"),
    ]
    .into_iter()
    .map(|(label, id)| FavoriteModelView {
        account_label: text(label),
        model_id: text(id),
    })
    .collect()
}

/// The model picker groups of the mock: three signed-in accounts on the
/// mock workers, and OpenCode with enough models to make the list scroll.
pub fn model_groups() -> Vec<ModelGroupView> {
    let claude = || {
        levels(
            &[
                ("low", "Low"),
                ("medium", "Medium"),
                ("high", "High"),
                ("max", "Max"),
            ],
            Some("medium"),
        )
    };
    let codex = || {
        levels(
            &[
                ("low", "Low"),
                ("medium", "Medium"),
                ("high", "High"),
                ("xhigh", "Extra high"),
            ],
            Some("medium"),
        )
    };
    let model = |id: &str, name: &str, effort: Option<EffortView>| ModelChoiceView {
        id: text(id),
        name: text(name),
        effort,
    };
    vec![
        ModelGroupView {
            account_label: text("Claude Personal"),
            provider: text("Claude Code"),
            logo: ProviderLogo::ClaudeCode,
            workers: vec![text("hub"), text("MacBook Pro")],
            models: vec![
                model("claude-opus-5-5", "Opus 5.5", claude()),
                model("claude-sonnet-5-5", "Sonnet 5.5", claude()),
                model("claude-haiku-5-5", "Haiku 5.5", claude()),
                model("claude-haiku-4-5", "Haiku 4.5", None),
            ],
        },
        ModelGroupView {
            account_label: text("Claude Work"),
            provider: text("Claude Code"),
            logo: ProviderLogo::ClaudeCode,
            workers: vec![text("MacBook Pro")],
            models: vec![
                model("claude-opus-5-5", "Opus 5.5", claude()),
                model("claude-sonnet-5-5", "Sonnet 5.5", claude()),
            ],
        },
        ModelGroupView {
            account_label: text("Codex Personal"),
            provider: text("Codex"),
            logo: ProviderLogo::Codex,
            workers: vec![text("hub"), text("bigboi")],
            models: vec![
                model("gpt-5.5", "GPT-5.5", codex()),
                model("gpt-6.1-sol", "GPT-6.1-Sol", codex()),
                model("gpt-6-astra", "GPT-6-Astra", codex()),
            ],
        },
        ModelGroupView {
            account_label: text("OpenCode with Fireworks"),
            provider: text("OpenCode"),
            logo: ProviderLogo::OpenCode,
            workers: vec![text("homelab")],
            models: vec![
                model(
                    "fireworks/deepseek-v4p1-flash",
                    "DeepSeek V4.1 Flash",
                    levels(&[("low", "Low"), ("high", "High"), ("max", "Max")], None),
                ),
                model(
                    "fireworks/glm-5p3",
                    "GLM 5.3",
                    levels(&[("low", "Low"), ("high", "High"), ("max", "Max")], None),
                ),
                model("fireworks/kimi-k3", "Kimi K3", None),
                model("fireworks/qwen-4-coder", "Qwen 4 Coder", None),
            ],
        },
    ]
}

fn selection(account: &str, model_id: &str, model_name: &str) -> SelectionView {
    SelectionView {
        account_label: account.into(),
        model_id: model_id.into(),
        model_name: model_name.into(),
        options: Vec::new(),
    }
}

/// The icons of three ACP registry agents, as the registry serves them.
pub const AMP_ICON: &str = include_str!("../fixtures/acp-icons/amp-acp.svg");
pub const GOOSE_ICON: &str = include_str!("../fixtures/acp-icons/goose.svg");
pub const AUGGIE_ICON: &str = include_str!("../fixtures/acp-icons/auggie.svg");

/// The sign-in methods that a worker offers for a new account of a
/// harness, as the mock answers them.
pub fn sign_in_methods(harness: &str) -> Vec<SignInChoice> {
    let key = |variable: &str| SignInChoice::ApiKey {
        variable: variable.into(),
    };
    match harness {
        "claude" => vec![
            SignInChoice::ClaudePlan,
            SignInChoice::ClaudeConsole,
            key("ANTHROPIC_API_KEY"),
        ],
        "codex" => vec![
            SignInChoice::ChatgptDevice,
            SignInChoice::ChatgptBrowser,
            key("OPENAI_API_KEY"),
        ],
        "opencode" => vec![key("FIREWORKS_API_KEY"), key("OPENROUTER_API_KEY")],
        "gemini" => vec![
            SignInChoice::AcpAgent {
                method_id: "oauth-personal".into(),
                name: "Google account".into(),
            },
            key("GEMINI_API_KEY"),
        ],
        "acp" => vec![SignInChoice::Terminal {
            command: "my-agent login".into(),
        }],
        _ => vec![SignInChoice::AcpAgent {
            method_id: "login".into(),
            name: "Sign in".into(),
        }],
    }
}

fn account(
    id: &str,
    label: &str,
    harness: &str,
    detail: &str,
    status: AccountStatus,
) -> HarnessAccountView {
    let (provider, logo) = provider_of(harness);
    let logo = match logo {
        ProviderLogo::Registry { id, .. } if id == "amp-acp" => ProviderLogo::Registry {
            id,
            svg: Some(AMP_ICON.into()),
        },
        other => other,
    };
    let provider = if harness == "acp:amp-acp" {
        "Amp".to_owned()
    } else {
        provider
    };
    let methods = sign_in_methods(harness);
    let secret_names = if status == AccountStatus::Ready {
        methods
            .iter()
            .find_map(|m| match m {
                SignInChoice::ApiKey { variable } => Some(variable.clone()),
                _ => None,
            })
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };
    HarnessAccountView {
        id: id.into(),
        label: label.into(),
        harness: harness.into(),
        provider,
        logo,
        detail: detail.into(),
        detail_code: Vec::new(),
        auth: match status {
            AccountStatus::SignedIn => AuthState::SignedIn,
            AccountStatus::Ready => AuthState::KeyFound,
            AccountStatus::NotSignedIn => AuthState::SignedOut,
            AccountStatus::Expired | AccountStatus::Error => AuthState::Error,
            _ => AuthState::Unknown,
        },
        status,
        action: status.action(),
        sign_in_methods: methods,
        secret_names,
        email: None,
        extras_enabled: true,
        extras: None,
    }
}

fn tool(id: &str, version: Option<&str>, latest: Option<&str>, state: ToolState) -> ToolView {
    let (name, logo) = provider_of(id);
    ToolView {
        id: id.into(),
        name,
        logo,
        version: version.map(Into::into),
        latest: latest.map(Into::into),
        state,
        detail: None,
    }
}

/// The tools of a worker with Claude Code and Codex.
fn tools(worker: &str, codex_update: bool) -> Vec<ToolView> {
    let mut gemini = tool("gemini-cli", None, None, ToolState::Missing);
    gemini.detail = Some(format!(
        "Not installed. {worker} installs it when an account needs it."
    ));
    vec![
        tool("claude-code", Some("2.1.312"), None, ToolState::Ready),
        if codex_update {
            tool(
                "codex",
                Some("0.164.0"),
                Some("0.165.0"),
                ToolState::UpdateAvailable,
            )
        } else {
            tool("codex", Some("0.165.0"), None, ToolState::Ready)
        },
        tool("opencode", Some("1.19.5"), None, ToolState::Ready),
        gemini,
    ]
}

#[allow(clippy::too_many_arguments)]
fn worker(
    id: &str,
    name: &str,
    os: Os,
    online: bool,
    sidebar: &str,
    list: &str,
    status: &str,
    tags: &[&str],
    detected: &[&str],
    accounts: Vec<HarnessAccountView>,
    policy: PolicyView,
) -> WorkerView {
    WorkerView {
        id: id.into(),
        name: name.into(),
        os,
        online,
        paused: false,
        sidebar_detail: sidebar.into(),
        list_detail: list.into(),
        status_line: status.into(),
        tags: tags.iter().map(|t| (*t).to_owned()).collect(),
        detected_tags: detected.iter().map(|t| (*t).to_owned()).collect(),
        local_mcp_servers: match name {
            "bigboi" => vec![text("windows-signing")],
            "homelab" => vec![text("home-assistant")],
            _ => Vec::new(),
        },
        accounts,
        tools: tools(name, name == "bigboi"),
        tools_folder: if os == Os::Windows {
            text(r"%USERPROFILE%\.consort\tools")
        } else {
            text("~/.consort/tools")
        },
        policy,
    }
}

fn policy(mode: ApprovalMode, roots: &[&str], shell: &str) -> PolicyView {
    PolicyView {
        max_approval_mode: mode,
        allowed_roots: roots.iter().map(|r| (*r).to_owned()).collect(),
        allow_shell: true,
        shell_name: shell.into(),
    }
}

fn workers() -> Vec<WorkerView> {
    let claude = |id: &str| {
        account(
            id,
            "Claude Personal",
            "claude",
            "Claude Max plan.",
            AccountStatus::SignedIn,
        )
    };
    let codex = |id: &str| {
        account(
            id,
            "Codex Personal",
            "codex",
            "ChatGPT Pro plan.",
            AccountStatus::SignedIn,
        )
    };
    let fireworks = |id: &str, worker: &str| {
        account(
            id,
            "OpenCode with Fireworks",
            "opencode",
            &format!("API key saved on {worker}."),
            AccountStatus::Ready,
        )
    };
    vec![
        worker(
            "wrk_hub",
            "hub",
            Os::Linux,
            true,
            "Linux, VPS",
            "Linux on the hub VPS, 2 accounts",
            "Online. Ubuntu 26.04, x64, worker 0.4.1. Connected since 7:58 AM.",
            &["vps"],
            &["linux", "x64", "ram-8gb", "cpu-4"],
            vec![
                claude("acc_hub_claude"),
                fireworks("acc_hub_opencode", "hub"),
            ],
            policy(ApprovalMode::Auto, &["/srv/consort"], "bash"),
        ),
        worker(
            "wrk_homelab",
            "homelab",
            Os::Linux,
            true,
            "Linux",
            "Linux, 3 accounts, 2 MCP servers",
            "Online. Debian 13, x64, worker 0.4.1. Connected since 7:59 AM.",
            &["personal", "home"],
            &["linux", "x64", "ram-32gb", "cpu-16"],
            vec![
                claude("acc_homelab_claude"),
                codex("acc_homelab_codex"),
                fireworks("acc_homelab_opencode", "homelab"),
            ],
            policy(ApprovalMode::Auto, &["/home/chris"], "bash"),
        ),
        worker(
            "wrk_bigboi",
            "bigboi",
            Os::Windows,
            true,
            "Windows",
            "Windows, 5 accounts, 1 task running",
            "Online. Windows 11, x64, worker 0.4.1. Connected since 8:02 AM.",
            &["personal", "windows-test"],
            &["windows", "x64", "ram-64gb", "cpu-24", "gpu-nvidia"],
            vec![
                claude("acc_bigboi_claude"),
                account(
                    "acc_bigboi_claude_work",
                    "Claude Work",
                    "claude",
                    "The login expired on Oct 27. Tasks skip this account until you sign in again.",
                    AccountStatus::Expired,
                ),
                codex("acc_bigboi_codex"),
                fireworks("acc_bigboi_opencode", "bigboi"),
                account(
                    "acc_bigboi_amp",
                    "Amp",
                    "acp:amp-acp",
                    "ACP agent from the registry. Not signed in yet.",
                    AccountStatus::NotSignedIn,
                ),
            ],
            PolicyView {
                max_approval_mode: ApprovalMode::AutoEdits,
                allowed_roots: vec![text(r"C:\Code"), text(r"D:\Builds")],
                allow_shell: true,
                shell_name: text("PowerShell"),
            },
        ),
        worker(
            "wrk_macbook",
            "MacBook Pro",
            Os::MacOs,
            true,
            "macOS",
            "macOS, 4 accounts",
            "Online. macOS 27, arm64, worker 0.4.1. Connected since 8:10 AM.",
            &["personal", "work"],
            &["macos", "arm64", "ram-36gb", "cpu-12", "gpu-apple"],
            vec![
                claude("acc_mac_claude"),
                codex("acc_mac_codex"),
                account(
                    "acc_mac_claude_work",
                    "Claude Work",
                    "claude",
                    "Claude Team plan.",
                    AccountStatus::SignedIn,
                ),
                fireworks("acc_mac_opencode", "MacBook Pro"),
            ],
            policy(ApprovalMode::Auto, &["/Users/chris/Code"], "zsh"),
        ),
        worker(
            "wrk_truenas",
            "truenas",
            Os::Linux,
            false,
            "Offline",
            "Offline since 6:12 PM",
            "Offline since 6:12 PM. TrueNAS SCALE, x64, worker 0.4.0.",
            &["storage"],
            &["linux", "x64", "ram-16gb", "cpu-8"],
            vec![claude("acc_truenas_claude")],
            policy(ApprovalMode::Ask, &["/mnt/tank"], "bash"),
        ),
    ]
}

/// The agents of the ACP registry, as the mock loads them.
pub fn acp_agents() -> Vec<AcpAgentView> {
    let agent = |id: &str, name: &str, description: &str, svg: Option<&str>, supported: bool| {
        AcpAgentView {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            version: "1.0.0".into(),
            logo: ProviderLogo::Registry {
                id: id.into(),
                svg: svg.map(Into::into),
            },
            supported,
        }
    };
    vec![
        agent(
            "amp-acp",
            "Amp",
            "Sourcegraph's coding agent, through its ACP adapter.",
            Some(AMP_ICON),
            true,
        ),
        agent(
            "goose",
            "Goose",
            "Block's open source agent. Works with many model providers.",
            Some(GOOSE_ICON),
            true,
        ),
        agent(
            "auggie",
            "Auggie CLI",
            "Augment Code's agent with its context engine.",
            Some(AUGGIE_ICON),
            true,
        ),
        agent(
            "uvx-agent",
            "Example uvx agent",
            "An agent that only ships as a Python package.",
            None,
            false,
        ),
    ]
}

fn tasks() -> Vec<TaskView> {
    vec![
        task(
            T_SAYSO,
            SAYSO,
            "sayso",
            "Go through open Sayso issues",
            TaskStatus::NeedsInput,
            (Some("bigboi"), Some("Codex Personal"), None),
            "2 min",
            2,
        ),
        task(
            T_SPAM,
            LUCY,
            "miss-lucy-tg",
            "Tune the spam rules for new members",
            TaskStatus::Review,
            (Some("hub"), Some("Claude Personal"), None),
            "1 h",
            60,
        ),
        task(
            T_BACKUPS,
            HOMELAB,
            "homelab",
            "Move the nightly backups to homelab",
            TaskStatus::Working,
            (Some("homelab"), Some("Claude Personal"), None),
            "40 min",
            40,
        ),
        task(
            T_ROOF,
            LIGHTS,
            "house-lights",
            "Price out roofline lighting for the house",
            TaskStatus::Working,
            (Some("hub"), Some("Pi with Kimi K3"), None),
            "12 min",
            12,
        ),
        task(
            T_EXT4,
            EXT4,
            "mac-ext4",
            "Build the ext4 extension on macOS 27",
            TaskStatus::Failed,
            (
                Some("MacBook Pro"),
                Some("Codex Personal"),
                Some("Code signing failed"),
            ),
            "3 h",
            180,
        ),
        task(
            T_CRASH,
            SAYSO,
            "sayso",
            "Sort the crash reports from 0.9.2",
            TaskStatus::UpNext,
            (None, None, Some("Waiting for bigboi to finish a task")),
            "5 min",
            5,
        ),
        task(
            T_CERT,
            HOMELAB,
            "homelab",
            "Renew the TLS certificate on truenas",
            TaskStatus::Scheduled,
            (None, None, Some("Starts Friday at 9:00 AM")),
            "1 d",
            1_440,
        ),
        task(
            T_TOKEN,
            LUCY,
            "miss-lucy-tg",
            "Rotate the bot token for Miss Lucy",
            TaskStatus::Scheduled,
            (None, None, Some("Starts Monday at 10:00 AM")),
            "2 d",
            2_880,
        ),
        task(
            T_REPO,
            SAYSO,
            "sayso",
            "Get up to speed on the Sayso repo",
            TaskStatus::Resolved,
            (Some("MacBook Pro"), Some("Claude Personal"), None),
            "1 d",
            1_600,
        ),
    ]
}

fn timelines() -> Vec<TimelineView> {
    let brief = TimelineItem::Message(MessageView {
        id: text("msg_brief"),
        channel_id: TUTTI_ID.into(),
        speaker: Speaker::Consort,
        time_label: text("8:00 AM"),
        title: Some(text("Good morning, Chris.")),
        body: text(
            "Two tasks need you, and one is still running. Your call with Boardy is at 1:00 PM.",
        ),
        items: vec![
            BriefItem {
                task_id: Some(T_SAYSO.into()),
                status: TaskStatus::NeedsInput,
                text: text("Codex needs your OK to build the Windows engine on bigboi."),
                channel_name: text("sayso"),
            },
            BriefItem {
                task_id: Some(T_SPAM.into()),
                status: TaskStatus::Review,
                text: text("The new spam rules are ready for review."),
                channel_name: text("miss-lucy-tg"),
            },
            BriefItem {
                task_id: Some(T_BACKUPS.into()),
                status: TaskStatus::Working,
                text: text("Backups are moving from truenas to homelab. About 20 minutes left."),
                channel_name: text("homelab"),
            },
        ],
        ..MessageView::default()
    });
    vec![
        TimelineView {
            channel_id: TUTTI_ID.into(),
            has_older: true,
            loaded: true,
            items: vec![
                day("day_tutti_today", "Today"),
                brief,
                message(
                    "msg_tutti_you",
                    TUTTI_ID,
                    Speaker::You,
                    "9:12 AM",
                    "What is left on the Sayso Windows fix?",
                ),
                message(
                    "msg_tutti_reply",
                    TUTTI_ID,
                    Speaker::Consort,
                    "9:12 AM",
                    "One step is left. Codex wants to build the Windows engine on bigboi, and it waits for your OK. After the build, it pushes the fix to [PR #62](https://github.com/watzon/sayso/pull/62).\n\nThe CI workflow change is still a patch in the PR description, because GitHub does not let a task push workflow files.",
                ),
                card(
                    "msg_tutti_card",
                    T_SAYSO,
                    "9:13 AM",
                    Some("sayso"),
                    Some(("bigboi", true, "Codex Personal")),
                    Some(sayso_approval()),
                    "",
                    None,
                ),
            ],
        },
        TimelineView {
            channel_id: SAYSO.into(),
            has_older: true,
            loaded: true,
            items: vec![
                day("day_sayso_yesterday", "Yesterday"),
                card(
                    "msg_card_repo",
                    T_REPO,
                    "4:10 PM",
                    None,
                    Some(("MacBook Pro", true, "Claude Personal")),
                    None,
                    "Resolved, 6 replies",
                    None,
                ),
                day("day_sayso_today", "Today"),
                message(
                    "msg_sayso_fix",
                    SAYSO,
                    Speaker::Consort,
                    "5:49 PM",
                    "The fix for #37 is up as [PR #62](https://github.com/watzon/sayso/pull/62). Sayso now ships three engine builds on x86_64 and picks the one that the CPU can run.\n\nAll three builds pass on emulated older CPUs. Nothing has run on real old hardware yet.",
                ),
                message(
                    "msg_sayso_you",
                    SAYSO,
                    Speaker::You,
                    "6:28 PM",
                    "Might as well have the agent patch the Windows engine too.",
                ),
                message(
                    "msg_sayso_sent",
                    SAYSO,
                    Speaker::Consort,
                    "6:28 PM",
                    "I sent the Windows engine fix to the same task. It goes on [PR #62](https://github.com/watzon/sayso/pull/62) with the other fixes for #37.",
                ),
                card(
                    &format!("msg_card_{T_SAYSO}"),
                    T_SAYSO,
                    "7:06 PM",
                    None,
                    Some(("bigboi", true, "Codex Personal")),
                    Some(sayso_approval()),
                    "",
                    None,
                ),
            ],
        },
        TimelineView {
            channel_id: LUCY.into(),
            has_older: true,
            loaded: true,
            items: vec![
                day("day_lucy_today", "Today"),
                message(
                    "msg_lucy_you",
                    LUCY,
                    Speaker::You,
                    "7:40 AM",
                    "New accounts keep posting crypto links in the first minute. Can the spam rules catch them?",
                ),
                message(
                    "msg_lucy_reply",
                    LUCY,
                    Speaker::Consort,
                    "7:41 AM",
                    "Yes. I started a task to tune the rules for new members. It holds a first message with a link for review, and it keeps the current rules for everyone else.",
                ),
                card(
                    &format!("msg_card_{T_SPAM}"),
                    T_SPAM,
                    "7:41 AM",
                    None,
                    Some(("hub", true, "Claude Personal")),
                    None,
                    "Ready for review, 4 replies",
                    Some(SPAM_SUMMARY),
                ),
            ],
        },
        TimelineView {
            channel_id: EXT4.into(),
            has_older: false,
            loaded: true,
            items: vec![
                day("day_ext4_today", "Today"),
                message(
                    "msg_ext4_you",
                    EXT4,
                    Speaker::You,
                    "6:05 AM",
                    "Build the extension against the macOS 27 SDK and tell me what breaks.",
                ),
                card(
                    &format!("msg_card_{T_EXT4}"),
                    T_EXT4,
                    "6:06 AM",
                    None,
                    Some(("MacBook Pro", true, "Codex Personal")),
                    None,
                    "Failed, 3 replies",
                    Some(
                        "Code signing failed: no identity matches `Developer ID Application`. Install the certificate on the MacBook Pro, then retry.",
                    ),
                ),
            ],
        },
        TimelineView {
            channel_id: HOMELAB.into(),
            has_older: true,
            loaded: true,
            items: vec![
                day("day_homelab_today", "Today"),
                message(
                    "msg_homelab_you",
                    HOMELAB,
                    Speaker::You,
                    "8:20 AM",
                    "truenas keeps dropping offline. Move the nightly backups to homelab for now.",
                ),
                card(
                    &format!("msg_card_{T_BACKUPS}"),
                    T_BACKUPS,
                    "8:21 AM",
                    None,
                    Some(("homelab", true, "Claude Personal")),
                    None,
                    "Working, 5 replies",
                    Some("Copied 412 of 690 GB. About 20 minutes left."),
                ),
            ],
        },
        TimelineView {
            channel_id: crate::mock_extensions::ACME.into(),
            has_older: false,
            loaded: true,
            items: {
                let mut items = vec![day("day_acme_today", "Today")];
                items.extend(crate::mock_extensions::acme_items());
                items
            },
        },
        TimelineView {
            channel_id: crate::mock_extensions::HIRING.into(),
            has_older: false,
            loaded: true,
            items: {
                let mut items = vec![day("day_hiring_today", "Today")];
                items.extend(crate::mock_extensions::hiring_items());
                items
            },
        },
        TimelineView {
            channel_id: LIGHTS.into(),
            has_older: false,
            loaded: true,
            items: vec![
                day("day_lights_today", "Today"),
                message(
                    "msg_lights_you",
                    LIGHTS,
                    Speaker::You,
                    "8:48 AM",
                    "Price out permanent roofline lights for the front of the house. About 120 feet.",
                ),
                card(
                    &format!("msg_card_{T_ROOF}"),
                    T_ROOF,
                    "8:49 AM",
                    None,
                    Some(("hub", true, "Pi with Kimi K3")),
                    None,
                    "Working, 2 replies",
                    Some(
                        "Compared four kits so far. Two work with Home Assistant without a cloud account.",
                    ),
                ),
            ],
        },
    ]
}

fn post(id: &str, speaker: Speaker, time: &str, bodies: Vec<PostBody>) -> ThreadPost {
    ThreadPost {
        id: id.into(),
        speaker,
        time_label: time.into(),
        bodies,
        pending: false,
    }
}

fn threads() -> Vec<TaskThreadView> {
    let placement =
        |worker: &str, os: &str, online: bool, harness: &str, path: &str, branch: Option<&str>| {
            Some(PlacementView {
                worker_name: worker.into(),
                worker_os: os.into(),
                worker_online: online,
                harness: harness.into(),
                isolation: Isolation::Worktree,
                path: path.into(),
                branch: branch.map(Into::into),
                repo: None,
            })
        };
    vec![
        TaskThreadView {
            task_id: T_SAYSO.into(),
            title: String::new(),
            status: TaskStatus::NeedsInput,
            status_detail: waiting_detail(1, 1),
            reason: None,
            placement: placement(
                "bigboi",
                "Windows",
                true,
                "Codex Personal, high effort",
                r"C:\Code\sayso",
                Some("fix/37"),
            ),
            brief: text(
                "Go through the open Sayso issues and fix the ones that block 0.9.3.\n\nStart with #37: Sayso crashes on a CPU without AVX2. Ship a build that picks the right engine for the CPU. Open one pull request for all the fixes.",
            ),
            review_summary: None,
            text_is_preview: false,
            approval_mode: ApprovalMode::Ask,
            model_label: Some(text("GPT-5.5, Codex Personal, High effort")),
            posts: vec![
                post(
                    "pst_sayso_1",
                    Speaker::Task,
                    "7:06 PM",
                    vec![
                        PostBody::Activity {
                            summary: text("Ran 14 commands and changed 6 files"),
                            items: vec![
                                ActivityLine::text("cargo test --workspace"),
                                ActivityLine::text("Edited engine/src/cpu.rs"),
                                ActivityLine::text("Edited engine/build.rs"),
                                ActivityLine::text("Edited .github/workflows/release.yml"),
                            ],
                        },
                        PostBody::Approval(sayso_approval()),
                    ],
                ),
                post(
                    "pst_sayso_2",
                    Speaker::Task,
                    "7:08 PM",
                    vec![PostBody::Decision(sayso_decision())],
                ),
            ],
        },
        TaskThreadView {
            task_id: T_SPAM.into(),
            status_detail: String::new(),
            placement: placement(
                "hub",
                "Linux",
                true,
                "Claude Personal, Opus 5.5",
                "/srv/consort/miss-lucy",
                Some("spam-rules"),
            ),
            brief: text(SPAM_BRIEF),
            review_summary: Some(text(SPAM_SUMMARY)),
            approval_mode: ApprovalMode::AutoEdits,
            model_label: Some(text("Opus 5.5, Claude Personal")),
            posts: vec![post(
                "pst_spam_1",
                Speaker::Task,
                "8:52 AM",
                vec![
                    PostBody::Activity {
                        summary: text("Ran 9 commands and changed 3 files"),
                        items: vec![ActivityLine::text("pytest tests/test_rules.py")],
                    },
                    PostBody::Text(text(
                        "The new rules are ready.\n\n- A new member's first message with a link waits for review.\n- Members who joined more than a day ago keep the current rules.\n- A dry run on last week's messages flags 41 of 43 spam posts and no real ones.\n\n```toml\n[new_members]\nhold_first_link = true\nwindow_minutes = 1440\n```",
                    )),
                ],
            )],
            ..TaskThreadView::default()
        },
        TaskThreadView {
            task_id: T_BACKUPS.into(),
            placement: placement(
                "homelab",
                "Linux",
                true,
                "Claude Personal, Sonnet 5.5",
                "/srv/backups",
                None,
            ),
            posts: vec![post(
                "pst_backups_1",
                Speaker::Task,
                "9:30 AM",
                vec![
                    PostBody::Activity {
                        summary: text("Ran 6 commands"),
                        items: vec![ActivityLine::text(
                            "rsync -a truenas:/mnt/tank/backups/ /srv/backups/",
                        )],
                    },
                    PostBody::Text(text("Copied 412 of 690 GB. About 20 minutes left.")),
                ],
            )],
            ..TaskThreadView::default()
        },
        TaskThreadView {
            task_id: T_ROOF.into(),
            placement: placement("hub", "Linux", true, "Pi with Kimi K3", "", None),
            posts: vec![post(
                "pst_roof_1",
                Speaker::Task,
                "8:58 AM",
                vec![PostBody::Text(text(
                    "Compared four kits so far. Two work with Home Assistant without a cloud account.",
                ))],
            )],
            ..TaskThreadView::default()
        },
        TaskThreadView {
            task_id: T_EXT4.into(),
            reason: Some(
                "Code signing failed: no identity matches `Developer ID Application`. Install the certificate on the MacBook Pro, then retry.".to_owned(),
            ),
            placement: placement(
                "MacBook Pro",
                "macOS",
                true,
                "Codex Personal, high effort",
                "/Users/chris/Code/mac-ext4",
                Some("macos-27"),
            ),
            posts: vec![post(
                "pst_ext4_1",
                Speaker::Task,
                "6:40 AM",
                vec![PostBody::Text(text(
                    "Code signing failed: no identity matches `Developer ID Application`.\n\n```sh\ncodesign --sign \"Developer ID Application\" build/ext4.appex\nerror: The specified item could not be found in the keychain.\n```\n\nInstall the certificate on the MacBook Pro, then retry the task.",
                ))],
            )],
            ..TaskThreadView::default()
        },
        TaskThreadView {
            task_id: T_REPO.into(),
            placement: placement(
                "MacBook Pro",
                "macOS",
                true,
                "Claude Personal, Opus 5.5",
                "/Users/chris/Code/sayso",
                None,
            ),
            posts: vec![post(
                "pst_repo_1",
                Speaker::Task,
                "4:40 PM",
                vec![PostBody::Text(text(
                    "I read the repo and wrote a summary to channel memory: the engine layout, the release flow, and the open issues for Windows.",
                ))],
            )],
            ..TaskThreadView::default()
        },
    ]
}

/// The view state from the designs.
pub fn view_state() -> ViewState {
    let mut channels = vec![
        channel(TUTTI_ID, TUTTI, "All channels, one memory", "brace", None),
        channel(
            SAYSO,
            "sayso",
            "Keep Sayso shipping on macOS, Windows and Linux",
            "mic",
            Some("grp_personal"),
        ),
        channel(
            LUCY,
            "miss-lucy-tg",
            "Run the Miss Lucy Telegram bot and its spam rules",
            "shield",
            Some("grp_personal"),
        ),
        channel(
            EXT4,
            "mac-ext4",
            "Read and write ext4 drives on macOS",
            "hard-drive",
            Some("grp_personal"),
        ),
        channel(
            crate::mock_extensions::ACME,
            "acme",
            "Contracts and support for Acme",
            "inbox",
            Some("grp_personal"),
        ),
        channel(
            crate::mock_extensions::HIRING,
            "hiring",
            "Hiring for the Acme team",
            "users",
            Some("grp_personal"),
        ),
        channel(
            HOMELAB,
            "homelab",
            "Keep the home servers and their backups healthy",
            "server",
            Some("grp_home"),
        ),
        channel(
            LIGHTS,
            "house-lights",
            "Smart lights and the roofline project",
            "lightbulb",
            Some("grp_home"),
        ),
    ];
    if let Some(sayso) = channels.iter_mut().find(|c| c.id == SAYSO) {
        sayso.instructions = text(
            "Never push to main. Open a PR.\n\nRun the engine tests on bigboi before a Windows release.",
        );
        sayso.repos = vec![RepoView {
            id: text("rep_sayso"),
            name: text("watzon/sayso"),
            checkouts: text("Checked out on bigboi and MacBook Pro"),
            url: text("https://github.com/watzon/sayso"),
        }];
        sayso.preferred_workers = vec![text("wrk_bigboi"), text("wrk_macbook")];
        sayso.task_selection = Some(selection("Codex Personal", "gpt-5.5", "GPT-5.5"));
        // sayso has its own model for the assistant.
        sayso.coordinator_selection = Some(EffectiveSelection {
            selection: selection("Claude Work", "claude-sonnet-5-5", "Sonnet 5.5"),
            source: SelectionSource::Channel,
        });
        sayso.tags = vec![text("personal")];
    }
    for channel in channels.iter_mut() {
        if channel.id == crate::mock_extensions::ACME
            || channel.id == crate::mock_extensions::HIRING
        {
            channel.tags = vec![text("work")];
        }
        if channel.id == crate::mock_extensions::ACME {
            channel.disabled_connections = vec![text(crate::mock_extensions::CON_GMAIL_PERSONAL)];
            channel.disabled_skills = vec![text(crate::mock_extensions::SKL_RUST)];
        }
    }
    let mut state = ViewState {
        connection: Connection::Demo,
        saved_data: None,
        user: UserView {
            name: text("Chris"),
            initials: text("CW"),
        },
        sidebar: SidebarView::default(),
        channels,
        groups: vec![
            GroupView {
                id: text("grp_personal"),
                name: text("Personal projects"),
            },
            GroupView {
                id: text("grp_home"),
                name: text("Home"),
            },
        ],
        archived_channels: vec![ArchivedChannelView {
            id: text("chn_0192f0c4-old-garden"),
            name: text("garden-sensors"),
            icon: text("hash"),
        }],
        timelines: timelines(),
        tasks: tasks(),
        threads: threads(),
        tasks_panel: TasksPanelView::default(),
        workers: workers(),
        tags: Vec::new(),
        model_groups: model_groups(),
        favorite_models: favorite_models(),
        sign_ins: Vec::new(),
        acp_registry: AcpRegistryView::default(),
        worker_pairing: None,
        send_failures: Vec::new(),
        connections: crate::mock_extensions::connections(),
        skills: crate::mock_extensions::skills(),
        composio: crate::mock_extensions::composio(),
        connector_catalog: CatalogView {
            categories: crate::mock_extensions::categories(),
            ..CatalogView::default()
        },
        service_tools: Vec::new(),
        skill_directory: SkillDirectoryView {
            sources: crate::mock_extensions::directory_sources(),
            ..SkillDirectoryView::default()
        },
        activity: activity(),
        uploads: Vec::new(),
        channel_files: crate::mock_files::channel_files(),
        files_loaded: vec![text(LIGHTS), text(EXT4)],
        admin: crate::mock_admin::admin(),
        routines: crate::mock_routines::routines(),
        memory_notes: crate::mock_routines::memory_notes(),
        memory_loaded: vec![text(crate::memory::GLOBAL), text(LIGHTS), text(SAYSO)],
        memory_deleted_loaded: vec![text(crate::memory::GLOBAL), text(LIGHTS), text(SAYSO)],
        convo: crate::convo::ConvoView::default(),
        proactivity: Default::default(),
    };
    stamp_times(&mut state.timelines);
    crate::mock_convo::decorate(&mut state);
    if let Some(account) = state
        .workers
        .iter_mut()
        .flat_map(|w| w.accounts.iter_mut())
        .find(|a| a.id == "acc_bigboi_claude")
    {
        account.email = Some(text("cawatson1993@gmail.com"));
        account.extras = Some(crate::mock_extensions::claude_extras());
    }
    stamp_times(&mut state.timelines);
    refresh(&mut state);
    state
}

/// The mock with Tutti as "Tutti manages connectors and
/// channels" shows it: a request and the connect card at the end.
/// The mock with the files of the phase 1 designs. See
/// [`crate::mock_files::with_files`].
pub fn view_state_with_files() -> ViewState {
    let mut state = crate::mock_files::with_files(view_state());
    refresh(&mut state);
    state
}

pub fn view_state_with_connect_card() -> ViewState {
    let mut state = view_state();
    if let Some(timeline) = state
        .timelines
        .iter_mut()
        .find(|t| t.channel_id == TUTTI_ID)
    {
        timeline.items.extend(crate::mock_extensions::tutti_items());
    }
    stamp_times(&mut state.timelines);
    refresh(&mut state);
    state
}

/// The mock with a coordinator tool approval and a coordinator decision in
/// the house-lights timeline, for `--screen channel-requests`.
pub fn view_state_with_requests() -> ViewState {
    let mut state = view_state();
    if let Some(timeline) = state.timelines.iter_mut().find(|t| t.channel_id == LIGHTS) {
        for item in &mut timeline.items {
            if let TimelineItem::TaskCard(card) = item {
                card.approval = Some(ApprovalView {
                    id: text("apr_lights_write"),
                    task_id: Some(T_ROOF.into()),
                    summary: text("Write roofline-quotes.md"),
                    folder: None,
                    detail: Some(text(
                        "/private/tmp/consort-e2e-data/worker/workspaces/tsk_0192f0c4-roofline/roofline-quotes.md",
                    )),
                    choices: vec![
                        ApprovalChoice::AllowOnce,
                        ApprovalChoice::AllowForTask,
                        ApprovalChoice::Deny,
                    ],
                    state: RequestState::Pending,
                    answer: None,
                    allow_always_label: Some(text(
                        "Yes, and don't ask again for roofline-quotes.md",
                    )),
                    card: None,
                    button_labels: Vec::new(),
                });
            }
        }
        timeline.items.push(TimelineItem::Approval(ApprovalCardView {
            message_id: text("msg_lights_approval"),
            speaker: Speaker::Consort,
            time_label: text("9:02 AM"),
            created_at: 0,
            approval: ApprovalView {
                id: text("apr_lights_post"),
                task_id: None,
                summary: text("Consort wants to use post_message from hub"),
                folder: None,
                detail: Some(text(
                    "{\n  \"body\": \"Two kits work with Home Assistant without a cloud account.\"\n}",
                )),
                choices: vec![
                    ApprovalChoice::AllowOnce,
                    ApprovalChoice::AllowForTask,
                    ApprovalChoice::Deny,
                ],
                state: RequestState::Pending,
                answer: None,
                allow_always_label: None,
                card: None,
                button_labels: Vec::new(),
            },
        }));
        timeline.items.push(TimelineItem::Decision(DecisionCardView {
            message_id: text("msg_lights_decision"),
            speaker: Speaker::Consort,
            time_label: text("9:03 AM"),
            created_at: 0,
            decision: DecisionView {
                id: text("dec_lights_kit"),
                task_id: None,
                question: text("Which roofline kit should I price in detail?"),
                context: Some(text("Both work with Home Assistant without a cloud account.")),
                options: vec![
                    DecisionOption {
                        label: text("Govee Permanent Outdoor Lights 2"),
                        consequence: text("Cheaper, and it installs in an afternoon. The app needs a Govee account for setup."),
                    },
                    DecisionOption {
                        label: text("Gemstone Lights"),
                        consequence: text("Brighter and made to last, but a dealer must install it."),
                    },
                ],
                recommended: Some(0),
                allow_free_text: true,
                state: RequestState::Pending,
                chosen: None,
                answer_text: None,
            },
            proactive: None,
        }));
    }
    stamp_times(&mut state.timelines);
    refresh(&mut state);
    state
}

/// The mock with a long sign-in error on bigboi, for `--screen
/// worker-added`.
pub fn view_state_with_long_detail() -> ViewState {
    let mut state = view_state();
    if let Some(work) = state
        .workers
        .iter_mut()
        .flat_map(|w| w.accounts.iter_mut())
        .find(|a| a.id == "acc_bigboi_claude_work")
    {
        work.detail = text(
            "Claude Code could not refresh the login of this account, because the organization changed its sign-in rules on Oct 27. Ask an admin of the organization to allow Claude Code on bigboi, then sign in again. The rules are at https://support.claude.com/en/articles/claude-code-organization-sign-in.",
        );
    }
    state
}

/// The mock's start of today, in Unix milliseconds. Only the gaps between
/// the times matter: the labels stay the ones that the designs show.
const MOCK_DAY: i64 = MOCK_NOW - 12 * 3_600_000;

/// Minutes after midnight for a label such as "9:12 AM".
fn minutes_of(label: &str) -> Option<i64> {
    let (clock, half) = label.trim().split_once(' ')?;
    let (h, m) = clock.split_once(':')?;
    let (h, m): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
    let h = h % 12
        + if half.eq_ignore_ascii_case("PM") {
            12
        } else {
            0
        };
    Some(h * 60 + m)
}

/// Gives the mock's items a time from their label, so the timeline can
/// place its time separators: "Today 8:00 AM", then "9:12 AM" after a long
/// gap. The day comes from the day item before them.
pub fn stamp_times(timelines: &mut [TimelineView]) {
    for timeline in timelines {
        let mut day = 0i64;
        for item in &mut timeline.items {
            let (label, at) = match item {
                TimelineItem::Day(d) => {
                    day = match d.label.as_str() {
                        "Yesterday" => -1,
                        _ => 0,
                    };
                    continue;
                }
                TimelineItem::Message(m) => (m.time_label.clone(), &mut m.created_at),
                TimelineItem::TaskCard(c) => (c.time_label.clone(), &mut c.created_at),
                TimelineItem::Approval(a) => (a.time_label.clone(), &mut a.created_at),
                TimelineItem::Decision(d) => (d.time_label.clone(), &mut d.created_at),
                TimelineItem::Notice(n) => (n.time_label.clone(), &mut n.created_at),
            };
            if *at == 0
                && let Some(minutes) = minutes_of(&label)
            {
                *at = MOCK_DAY + day * 86_400_000 + minutes * 60_000;
            }
        }
    }
}

/// What the assistants do in the mock. Tutti matches the design: a command
/// runs in sayso.
fn activity() -> Vec<crate::activity::ChannelActivity> {
    use crate::activity::{ActivityView, ChannelActivity, MarkState};
    let entry = |id: &str, state: MarkState, line: Option<&str>| ChannelActivity {
        channel_id: id.into(),
        activity: ActivityView {
            state,
            line: line.map(text),
        },
    };
    vec![
        entry(
            TUTTI_ID,
            MarkState::Working,
            Some("Running a command in sayso"),
        ),
        entry(SAYSO, MarkState::NeedsYou, None),
        entry(LUCY, MarkState::NeedsYou, None),
        entry(HOMELAB, MarkState::Working, Some("Running a command")),
        entry(LIGHTS, MarkState::Working, Some("Searching the web")),
    ]
}

/// The id of the Tutti channel in the mock.
pub fn tutti_id() -> &'static str {
    TUTTI_ID
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mock_matches_the_designs() {
        let state = view_state();
        assert_eq!(state.tasks_panel.need_you, 2);
        assert_eq!(state.tasks_panel.active_count, 5);
        assert_eq!(state.tasks_panel.up_next_count, 1);
        assert_eq!(state.tasks_panel.scheduled_count, 2);
        assert_eq!(state.sidebar.workers_online, 4);
        assert_eq!(state.sidebar.workers_total, 5);
        let personal = &state.sidebar.groups[0];
        assert_eq!(personal.name, "Personal projects");
        assert_eq!(personal.channels[0].name, "sayso");
        assert_eq!(personal.channels[0].status, Some(TaskStatus::NeedsInput));
        assert_eq!(personal.channels[1].status, Some(TaskStatus::Review));
        assert!(state.tutti().is_some());
        for timeline in &state.timelines {
            for item in &timeline.items {
                if let TimelineItem::TaskCard(card) = item {
                    assert!(
                        !card.title.is_empty(),
                        "card {} has its title",
                        card.task_id
                    );
                    assert!(state.task(&card.task_id).is_some());
                }
            }
        }
    }
}
