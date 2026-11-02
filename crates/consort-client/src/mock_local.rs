//! The mock states for a hub on this computer (Paper W2, W3, W4 and the
//! restart with tasks in V2).

use consort_proto::local::InstallStep;
use consort_proto::release::InstallKind;

use crate::admin::{BackupView, HubStatusView, LastBackupView};
use crate::local_hub::{
    HubRun, InstallPhase, InstallProgress, LocalHubView, LocalSupport, StepState, SwapState,
};
use crate::view::{
    AccountStatus, AuthState, HarnessAccountView, ProviderLogo, SignInChoice, SignInStep,
    SignInView, ToolState, ToolView, ViewState, WorkerView,
};
use crate::{LoadState, mock, mock_offline};

const ADDRESS: &str = "http://127.0.0.1:53135";
/// The account of the sign-in screens.
pub const SETUP_ACCOUNT: &str = "acc_setup";
/// The sign-in of the screen where the code fails.
pub const SETUP_SIGN_IN: &str = "sgn_setup";
const BACKUP_FOLDER: &str = "~/Library/Application Support/Consort/local/hub/backups";

/// The state of the Settings > Hub page with a hub on this computer.
pub fn local_status(online: bool) -> HubStatusView {
    HubStatusView {
        load: LoadState::Loaded,
        online,
        summary: "This hub runs on this computer.".to_owned(),
        address: ADDRESS.to_owned(),
        hub_version: "0.8.0".to_owned(),
        install: Some(InstallKind::Local),
        protocol: "1".to_owned(),
        protocol_line: "This app also speaks protocol 1.".to_owned(),
        data_dir: "~/Library/Application Support/Consort/local/hub".to_owned(),
        backup: BackupView {
            folder: BACKUP_FOLDER.to_owned(),
            ..BackupView::default()
        },
        ..HubStatusView::default()
    }
}

/// A view of an installed hub that runs.
pub fn running_view() -> LocalHubView {
    LocalHubView {
        support: LocalSupport::Available,
        installed: true,
        has_data: true,
        address: Some(ADDRESS.to_owned()),
        run: HubRun::Running,
        start_at_login: true,
        folder: "~/Library/Application Support/Consort".to_owned(),
        log_file: "~/Library/Application Support/Consort/local/logs/local.log".into(),
        probed: true,
        ..LocalHubView::default()
    }
}

/// The setup screens, for `--mock --screen <name>`.
pub const SETUP_SCREENS: [&str; 19] = [
    "setup-where",
    "setup-windows",
    "setup-bubblewrap",
    "setup-found",
    "setup-install",
    "setup-failed",
    "setup-connect",
    "setup-connect-unreachable",
    "setup-connect-refused",
    "setup-connect-busy",
    "setup-server",
    "setup-neutral",
    "setup-provider",
    "setup-no-host",
    "setup-sign-in",
    "setup-tool-installing",
    "setup-tool-failed",
    "setup-code-refused",
    "setup-no-account",
];

/// The states of the app for these screens.
pub fn screen_state(screen: &str) -> Option<ViewState> {
    let mut state = match screen {
        "local-stopped" => {
            let mut state = mock_offline::hub_offline();
            state.hub_url = ADDRESS.to_owned();
            return Some(state);
        }
        "hub-local" | "hub-local-remove" | "hub-local-connect" => mock::view_state(),
        "hub-local-move" => {
            let mut state = mock::view_state();
            state.admin.hub = local_status(true);
            state.admin.hub.backup.last = LastBackupView::Done {
                line: "Today at 3:12 PM".to_owned(),
                file_name: "consort-2026-10-31-1512.tar".to_owned(),
                size: "38 MB".to_owned(),
            };
            state.hub_url = ADDRESS.to_owned();
            return Some(state);
        }
        "restart-tasks" => mock::view_state(),
        screen if screen.starts_with("setup-") => return setup_state(screen),
        "local-install" | "local-windows" | "local-bubblewrap" | "local-failed" | "local-found" => {
            mock::view_state()
        }
        _ => return None,
    };
    state.admin.hub = local_status(true);
    state.hub_url = ADDRESS.to_owned();
    Some(state)
}

/// The session host of a new hub: no account, and the tools of the designs.
fn new_host() -> WorkerView {
    let tool = |id: &str, name: &str, logo, state, version: Option<&str>| ToolView {
        id: id.to_owned(),
        name: name.to_owned(),
        logo,
        version: version.map(str::to_owned),
        latest: None,
        state,
        detail: None,
    };
    let mut host = mock::view_state().session_host.unwrap_or_default();
    host.accounts.clear();
    host.tools = vec![
        tool(
            "claude-code",
            "Claude Code",
            ProviderLogo::ClaudeCode,
            ToolState::Ready,
            Some("2.1.320"),
        ),
        tool(
            "codex",
            "Codex",
            ProviderLogo::Codex,
            ToolState::Missing,
            None,
        ),
        tool(
            "opencode",
            "OpenCode",
            ProviderLogo::OpenCode,
            ToolState::Missing,
            None,
        ),
        tool(
            "gemini",
            "Gemini CLI",
            ProviderLogo::GeminiCli,
            ToolState::Missing,
            None,
        ),
    ];
    host
}

/// The first account: Claude, as it is before its sign-in.
fn first_account() -> HarnessAccountView {
    HarnessAccountView {
        id: SETUP_ACCOUNT.to_owned(),
        label: "Claude".to_owned(),
        harness: "claude".to_owned(),
        provider: "Claude Code".to_owned(),
        logo: ProviderLogo::ClaudeCode,
        detail: "Not signed in yet.".to_owned(),
        status: AccountStatus::NotSignedIn,
        auth: AuthState::SignedOut,
        sign_in_methods: vec![
            SignInChoice::ClaudePlan,
            SignInChoice::ClaudeConsole,
            SignInChoice::ApiKey {
                variable: "ANTHROPIC_API_KEY".to_owned(),
            },
        ],
        ..HarnessAccountView::default()
    }
}

/// The states of the setup screens (Paper W1 to S5, and T8).
fn setup_state(screen: &str) -> Option<ViewState> {
    let mut state = mock::view_state();
    state.model_groups.clear();
    let mut host = new_host();
    match screen {
        "setup-where"
        | "setup-windows"
        | "setup-bubblewrap"
        | "setup-found"
        | "setup-install"
        | "setup-failed"
        | "setup-connect"
        | "setup-connect-unreachable"
        | "setup-connect-refused"
        | "setup-connect-busy"
        | "setup-server"
        | "setup-neutral"
        | "setup-provider" => {}
        "setup-no-account" => {
            let tutti = state.tutti().map(|tutti| tutti.id.clone());
            for timeline in &mut state.timelines {
                if Some(&timeline.channel_id) == tutti.as_ref() {
                    timeline.items.clear();
                }
            }
        }
        "setup-no-host" => {
            state.session_host = None;
            return Some(state);
        }
        "setup-sign-in" => host.accounts.push(first_account()),
        "setup-tool-installing" | "setup-tool-failed" => {
            let failed = screen == "setup-tool-failed";
            let mut account = first_account();
            account.sign_in_methods.clear();
            host.accounts.push(account);
            if let Some(tool) = host.tools.first_mut() {
                tool.version = None;
                tool.state = if failed {
                    ToolState::Failed
                } else {
                    ToolState::Installing
                };
                tool.detail = failed.then(|| "npm: the download timed out.".to_owned());
            }
        }
        "setup-code-refused" => {
            host.accounts.push(first_account());
            state.sign_ins.push(SignInView {
                id: SETUP_SIGN_IN.to_owned(),
                worker_id: host.id.clone(),
                account_id: SETUP_ACCOUNT.to_owned(),
                method: Some(SignInChoice::ClaudePlan),
                step: SignInStep::Failed {
                    message: "Claude did not accept the code.".to_owned(),
                },
            });
        }
        _ => return None,
    }
    state.session_host = Some(host);
    Some(state)
}

/// The local hub for these screens.
pub fn screen_view(screen: &str) -> Option<LocalHubView> {
    let base = running_view();
    let not_here = LocalHubView {
        installed: false,
        has_data: false,
        address: None,
        run: HubRun::Unknown,
        ..base.clone()
    };
    Some(match screen {
        "setup-found" => base,
        "setup-install" => screen_view("local-install")?,
        "setup-windows" => screen_view("local-windows")?,
        "setup-bubblewrap" => screen_view("local-bubblewrap")?,
        "setup-failed" => screen_view("local-failed")?,
        screen if screen.starts_with("setup-") => not_here,
        "hub-local" | "hub-local-move" | "hub-local-remove" | "hub-local-connect" => base,
        "local-stopped" => LocalHubView {
            run: HubRun::Stopped,
            ..base
        },
        "restart-tasks" => LocalHubView {
            swap: SwapState::NeedsChoice { working: 2 },
            ..base
        },
        "local-install" => LocalHubView {
            installed: false,
            has_data: false,
            install: InstallProgress::after_line(InstallStep::Checked),
            ..base
        },
        "local-windows" => LocalHubView {
            installed: false,
            has_data: false,
            support: LocalSupport::NotOnThisSystem,
            ..base
        },
        "local-bubblewrap" => LocalHubView {
            installed: false,
            has_data: false,
            support: LocalSupport::NeedsBubblewrap {
                command: "sudo apt install bubblewrap".to_owned(),
            },
            ..base
        },
        "local-failed" => LocalHubView {
            installed: false,
            has_data: false,
            install: InstallProgress {
                phase: InstallPhase::Failed,
                steps: [
                    StepState::Done,
                    StepState::Working,
                    StepState::Waiting,
                    StepState::Waiting,
                ],
                error: Some(
                    "The hub did not answer in 60 seconds. The log is ~/Library/Application Support/Consort/local/logs/local.log".to_owned(),
                ),
            },
            ..base
        },
        "local-found" => base,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_screen_has_a_state_and_a_view() {
        for screen in [
            "hub-local",
            "hub-local-move",
            "hub-local-remove",
            "hub-local-connect",
            "local-stopped",
            "restart-tasks",
            "local-install",
            "local-windows",
            "local-bubblewrap",
            "local-failed",
            "local-found",
        ] {
            assert!(screen_state(screen).is_some(), "{screen}");
            assert!(screen_view(screen).is_some(), "{screen}");
        }
        for screen in SETUP_SCREENS {
            assert!(screen_state(screen).is_some(), "{screen}");
            assert!(screen_view(screen).is_some(), "{screen}");
        }
        assert!(screen_state("tutti").is_none());
        assert!(screen_view("tutti").is_none());
    }

    #[test]
    fn the_stopped_screen_meets_the_rule_of_the_card() {
        let state = screen_state("local-stopped").unwrap();
        let view = screen_view("local-stopped").unwrap();
        assert!(view.stopped_for(&state.hub_url, &state.connection));
    }
}
