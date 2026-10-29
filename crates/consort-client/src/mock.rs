//! Fixed content that reproduces the approved designs, so the app can be
//! shown without a hub (`consort-desktop --mock`). The mock also answers
//! commands, so buttons do what they will do against a real hub.

use crate::{
    derive::{refresh, waiting_detail},
    view::*,
};

const EVERYTHING: &str = "chn_everything";
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

/// A fixed "now" for the mock, so ages and times match the designs.
pub const MOCK_NOW: i64 = 1_793_300_000_000;

fn text(s: &str) -> String {
    s.to_owned()
}

fn channel(id: &str, name: &str, purpose: &str, icon: &str, group: Option<&str>) -> ChannelView {
    ChannelView {
        id: id.into(),
        kind: if id == EVERYTHING {
            ChannelKind::Everything
        } else {
            ChannelKind::Standard
        },
        name: name.into(),
        purpose: purpose.into(),
        instructions: String::new(),
        icon: icon.into(),
        group_id: group.map(Into::into),
        coordinator_worker: Some(text("hub")),
        coordinator_account: Some(text("Claude Personal")),
        repos: Vec::new(),
        preferred_workers: Vec::new(),
        task_selection: Some(text("Claude Personal, Opus 5.5")),
        default_isolation: Isolation::Worktree,
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
        task_id: T_SAYSO.into(),
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
    })
}

fn account(
    id: &str,
    label: &str,
    detail: &str,
    code: &[&str],
    auth: AuthState,
) -> HarnessAccountView {
    HarnessAccountView {
        id: id.into(),
        label: label.into(),
        detail: detail.into(),
        detail_code: code.iter().map(|c| (*c).to_owned()).collect(),
        auth,
    }
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
    labels: &[&str],
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
        labels: labels.iter().map(|l| (*l).to_owned()).collect(),
        accounts,
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
            "Claude Code 2.1.312 through ACP. Claude Max plan.",
            &[],
            AuthState::SignedIn,
        )
    };
    let codex = |id: &str| {
        account(
            id,
            "Codex Personal",
            "Codex 0.165.0. ChatGPT Pro plan.",
            &[],
            AuthState::SignedIn,
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
            vec![
                claude("acc_hub_claude"),
                account(
                    "acc_hub_pi",
                    "Pi with Kimi K3",
                    "Pi 0.31.0. API key from FIREWORKS_API_KEY.",
                    &["FIREWORKS_API_KEY"],
                    AuthState::KeyFound,
                ),
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
            &["home"],
            vec![
                claude("acc_homelab_claude"),
                codex("acc_homelab_codex"),
                account(
                    "acc_homelab_opencode",
                    "OpenCode with Fireworks",
                    "OpenCode 2.2.0. API key from FIREWORKS_API_KEY.",
                    &["FIREWORKS_API_KEY"],
                    AuthState::KeyFound,
                ),
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
            &["windows-test", "gpu"],
            vec![
                codex("acc_bigboi_codex"),
                account(
                    "acc_bigboi_codex_work",
                    "Codex Work",
                    "Codex 0.165.0. ChatGPT Business plan.",
                    &[],
                    AuthState::SignedIn,
                ),
                claude("acc_bigboi_claude"),
                account(
                    "acc_bigboi_opencode",
                    "OpenCode with Fireworks",
                    "OpenCode 2.2.0. API key from FIREWORKS_API_KEY.",
                    &["FIREWORKS_API_KEY"],
                    AuthState::KeyFound,
                ),
                account(
                    "acc_bigboi_gemini",
                    "Gemini CLI",
                    "Gemini CLI through ACP. Run gemini on bigboi to sign in.",
                    &["gemini"],
                    AuthState::SignedOut,
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
            &["laptop"],
            vec![
                claude("acc_mac_claude"),
                codex("acc_mac_codex"),
                account(
                    "acc_mac_codex_work",
                    "Codex Work",
                    "Codex 0.165.0. ChatGPT Business plan.",
                    &[],
                    AuthState::SignedIn,
                ),
                account(
                    "acc_mac_pi",
                    "Pi with Kimi K3",
                    "Pi 0.31.0. API key from FIREWORKS_API_KEY.",
                    &["FIREWORKS_API_KEY"],
                    AuthState::KeyFound,
                ),
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
            vec![claude("acc_truenas_claude")],
            policy(ApprovalMode::Ask, &["/mnt/tank"], "bash"),
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
        channel_id: EVERYTHING.into(),
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
            channel_id: EVERYTHING.into(),
            has_older: true,
            items: vec![
                day("day_everything_today", "Today"),
                brief,
                message(
                    "msg_everything_you",
                    EVERYTHING,
                    Speaker::You,
                    "9:12 AM",
                    "What is left on the Sayso Windows fix?",
                ),
                message(
                    "msg_everything_reply",
                    EVERYTHING,
                    Speaker::Consort,
                    "9:12 AM",
                    "One step is left. Codex wants to build the Windows engine on bigboi, and it waits for your OK. After the build, it pushes the fix to [PR #62](https://github.com/watzon/sayso/pull/62).\n\nThe CI workflow change is still a patch in the PR description, because GitHub does not let a task push workflow files.",
                ),
                card(
                    "msg_everything_card",
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
                    Some(
                        "The new rules are in `rules/new_members.toml`. Tests pass, and a dry run on last week's messages flags 41 of 43 spam posts.",
                    ),
                ),
            ],
        },
        TimelineView {
            channel_id: EXT4.into(),
            has_older: false,
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
            channel_id: LIGHTS.into(),
            has_older: false,
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
            })
        };
    vec![
        TaskThreadView {
            task_id: T_SAYSO.into(),
            title: String::new(),
            status: TaskStatus::NeedsInput,
            status_detail: waiting_detail(1, 1),
            placement: placement(
                "bigboi",
                "Windows",
                true,
                "Codex Personal, high effort",
                r"C:\Code\sayso",
                Some("fix/37"),
            ),
            posts: vec![
                post(
                    "pst_sayso_1",
                    Speaker::Task,
                    "7:06 PM",
                    vec![
                        PostBody::Activity {
                            summary: text("Ran 14 commands and changed 6 files"),
                            items: vec![
                                text("cargo test --workspace"),
                                text("Edited engine/src/cpu.rs"),
                                text("Edited engine/build.rs"),
                                text("Edited .github/workflows/release.yml"),
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
            posts: vec![post(
                "pst_spam_1",
                Speaker::Task,
                "8:52 AM",
                vec![
                    PostBody::Activity {
                        summary: text("Ran 9 commands and changed 3 files"),
                        items: vec![text("pytest tests/test_rules.py")],
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
                        items: vec![text("rsync -a truenas:/mnt/tank/backups/ /srv/backups/")],
                    },
                    PostBody::Text(text("Copied 412 of 690 GB. About 20 minutes left.")),
                ],
            )],
            ..TaskThreadView::default()
        },
        TaskThreadView {
            task_id: T_ROOF.into(),
            placement: placement(
                "hub",
                "Linux",
                true,
                "Pi with Kimi K3",
                "/srv/consort/house-lights",
                None,
            ),
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
        channel(
            EVERYTHING,
            "Everything",
            "All channels, one memory",
            "brace",
            None,
        ),
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
        }];
        sayso.preferred_workers = vec![text("wrk_bigboi"), text("wrk_macbook")];
        sayso.task_selection = Some(text("Codex Personal, GPT-5.5"));
    }
    let mut state = ViewState {
        connection: Connection::Demo,
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
        timelines: timelines(),
        tasks: tasks(),
        threads: threads(),
        tasks_panel: TasksPanelView::default(),
        workers: workers(),
        selections: vec![
            text("Claude Personal, Opus 5.5"),
            text("Claude Personal, Sonnet 5.5"),
            text("Codex Personal, GPT-5.5"),
            text("Codex Work, GPT-5.5"),
            text("Pi with Kimi K3"),
        ],
        worker_pairing: None,
    };
    refresh(&mut state);
    state
}

/// The id of the Everything channel in the mock.
pub fn everything_id() -> &'static str {
    EVERYTHING
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
        assert!(state.everything().is_some());
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
