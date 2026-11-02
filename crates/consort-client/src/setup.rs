//! The setup gate: which screen a client opens, from the state of the hub and
//! of this computer. It holds no saved step. The rule is one pure function,
//! [`gate`], so a user who quits in the middle comes back to the same place.
//! See `docs/notes/design-onboarding.md` and Paper O1.

use crate::cache::Cache;
use crate::error::ClientError;
use crate::local_hub::LocalHubView;
use crate::view::{Connection, ProviderLogo, SelectionView, ToolState, WorkerView};
use consort_proto::model::ChannelKind;

/// What the gate knows about the messages on the hub.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum History {
    /// Tutti has not loaded yet.
    #[default]
    Unknown,
    /// Tutti has no messages, and the hub has no other channel and no task.
    None,
    /// The hub was used before.
    Some,
}

/// What the gate reads from the cache of the hub.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HubFacts {
    /// The first load of the hub is in the cache.
    pub loaded: bool,
    /// The session host is online, and its first inventory has not arrived.
    pub inventory_pending: bool,
    /// One account can run a turn: it is signed in, or it has an API key.
    pub signed_in: bool,
    pub history: History,
    /// The user chose Do this later. The hub keeps this for every client.
    pub later: bool,
    /// Where the guided start in Tutti is.
    pub guide: consort_proto::model::GuideState,
}

/// Reads [`HubFacts`] from the cache.
pub fn hub_facts(cache: &Cache) -> HubFacts {
    let hosts = || {
        cache.workers.values().filter(|w| {
            w.revoked_at.is_none() && w.kind == consort_proto::model::MachineKind::SessionHost
        })
    };
    let inventory_pending = hosts()
        .any(|w| w.inventory.is_none() && w.state == consort_proto::model::WorkerState::Online);
    let signed_in = cache
        .workers
        .values()
        .filter(|w| w.revoked_at.is_none())
        .filter_map(|w| w.inventory.as_ref())
        .flat_map(|i| i.accounts.iter())
        .any(|a| a.enabled && a.auth_state == consort_proto::model::AuthState::SignedIn);
    let tutti = cache
        .channels
        .values()
        .find(|c| c.kind == ChannelKind::Tutti)
        .map(|c| c.id.to_string());
    let tutti_messages = tutti.as_ref().is_some_and(|id| {
        cache
            .messages
            .values()
            .any(|m| m.channel_id.to_string() == *id)
    });
    let other_channels = cache
        .channels
        .values()
        .any(|c| c.kind != ChannelKind::Tutti);
    let tutti_loaded = tutti
        .as_ref()
        .is_some_and(|id| cache.channels_loaded.contains(id));
    let history = if tutti_messages || other_channels || !cache.tasks.is_empty() {
        History::Some
    } else if tutti_loaded {
        History::None
    } else {
        History::Unknown
    };
    HubFacts {
        loaded: cache.loaded,
        inventory_pending,
        signed_in,
        history,
        later: cache.preferences.setup_later,
        guide: cache.preferences.guide,
    }
}

/// Whether a hub is on this computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LocalPresence {
    /// The app has not asked yet.
    #[default]
    Unknown,
    None,
    Installed,
}

impl From<&LocalHubView> for LocalPresence {
    fn from(view: &LocalHubView) -> Self {
        match (view.probed, view.installed) {
            (false, _) => LocalPresence::Unknown,
            (true, true) => LocalPresence::Installed,
            (true, false) => LocalPresence::None,
        }
    }
}

/// What opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Gate {
    /// The facts are not known yet. It shows no step, so nothing flickers.
    #[default]
    Neutral,
    /// W1: no key, and no hub on this computer.
    Where,
    /// W1 with the W3 card: no key, and a hub is on this computer.
    LocalFound,
    /// The hub refused this app. It pairs again.
    PairAgain,
    /// S3: the hub has no account that can run, and nothing says to leave it.
    Provider,
    /// Tutti, with a signed-in account or in a state that is not for setup.
    Main,
    /// T8: Tutti, with no signed-in account.
    NoAccount,
}

/// The screen of a client. See the table "What opens" in Paper O1, and
/// [`History`] for what counts as a hub that was used.
///
/// The provider screen needs all of these: no account can run, the hub has no
/// history, and the user did not choose Do this later. An account that is signed
/// out does not stop it, so a user who quit during a sign-in comes back to it.
/// A hub with history and no signed-in account opens T8, because its user has
/// used Consort before. Before the facts arrive the gate is neutral. Setup never
/// opens over data that the app can show: a client that cannot reach its hub, or
/// that has no common protocol with it, opens Main.
pub fn gate(connection: &Connection, local: LocalPresence, hub: &HubFacts) -> Gate {
    match connection {
        Connection::Demo | Connection::Incompatible { .. } => Gate::Main,
        Connection::Refused { .. } => Gate::PairAgain,
        Connection::NotPaired => match local {
            LocalPresence::Unknown => Gate::Neutral,
            LocalPresence::None => Gate::Where,
            LocalPresence::Installed => Gate::LocalFound,
        },
        Connection::Online | Connection::Connecting | Connection::Offline { .. } => {
            let online = *connection == Connection::Online;
            if !hub.loaded {
                // An offline client has nothing to read yet. Main says why.
                return match connection {
                    Connection::Offline { .. } => Gate::Main,
                    _ => Gate::Neutral,
                };
            }
            if hub.signed_in {
                return Gate::Main;
            }
            if hub.later || hub.history == History::Some {
                return Gate::NoAccount;
            }
            match connection {
                // The saved data can be old, so this waits for the hub.
                Connection::Connecting => Gate::Neutral,
                _ if online && (hub.inventory_pending || hub.history == History::Unknown) => {
                    Gate::Neutral
                }
                _ => Gate::Provider,
            }
        }
    }
}

/// Which dabs of the mark are lit. A dab lights when its stage starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dabs {
    /// W1: all are dark.
    #[default]
    None,
    /// The hub stage: blue.
    Hub,
    /// The account stage: blue and green.
    Account,
    /// Tutti: all three.
    All,
}

impl Dabs {
    pub fn hub(self) -> bool {
        self != Dabs::None
    }

    pub fn account(self) -> bool {
        matches!(self, Dabs::Account | Dabs::All)
    }

    pub fn assistant(self) -> bool {
        self == Dabs::All
    }
}

/// A provider that the first account can use.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderChoice {
    /// The harness, such as `claude`.
    pub harness: &'static str,
    pub name: &'static str,
    /// The line under the name.
    pub line: &'static str,
    pub logo: ProviderLogo,
}

/// The built-in providers, in the order of the designs.
pub const PROVIDERS: [ProviderChoice; 4] = [
    ProviderChoice {
        harness: "claude",
        name: "Claude Code",
        line: "Claude plan or Anthropic API key",
        logo: ProviderLogo::ClaudeCode,
    },
    ProviderChoice {
        harness: "codex",
        name: "Codex",
        line: "ChatGPT plan or OpenAI API key",
        logo: ProviderLogo::Codex,
    },
    ProviderChoice {
        harness: "opencode",
        name: "OpenCode",
        line: "API keys for Fireworks, OpenRouter and more",
        logo: ProviderLogo::OpenCode,
    },
    ProviderChoice {
        harness: "gemini",
        name: "Gemini CLI",
        line: "Gemini API key or Vertex AI",
        logo: ProviderLogo::GeminiCli,
    },
];

/// The providers that the session host can run. A host that reports no tools is
/// too old to say, so all are offered.
pub fn provider_choices(host: &WorkerView) -> Vec<ProviderChoice> {
    if host.tools.is_empty() {
        return PROVIDERS.to_vec();
    }
    PROVIDERS
        .into_iter()
        .filter(|p| host.tools.iter().any(|tool| tool.name == p.name))
        .collect()
}

/// The short name of a provider, such as "Claude" for Claude Code. It is the
/// default name of the first account, and the name on the sign-in screen.
pub fn provider_short_name(provider: &str) -> &str {
    provider.split(' ').next().unwrap_or(provider)
}

/// The tool of a provider on the session host.
pub fn tool_of<'a>(host: &'a WorkerView, provider: &str) -> Option<&'a crate::view::ToolView> {
    host.tools.iter().find(|tool| tool.name == provider)
}

/// What the sign-in screen shows about the tool of its provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolStage {
    /// It installs first. The sign-in starts when this ends.
    Installing,
    Failed {
        detail: Option<String>,
    },
    Ready {
        version: Option<String>,
    },
}

/// The stage of a tool. A tool that the host does not name needs no install.
pub fn tool_stage(tool: Option<&crate::view::ToolView>) -> ToolStage {
    match tool {
        None => ToolStage::Ready { version: None },
        Some(tool) => match tool.state {
            ToolState::Missing | ToolState::Installing => ToolStage::Installing,
            ToolState::Failed => ToolStage::Failed {
                detail: tool.detail.clone(),
            },
            ToolState::Ready | ToolState::UpdateAvailable => ToolStage::Ready {
                version: tool.version.clone(),
            },
        },
    }
}

/// The model that Tutti gets after the first sign-in: the first model of the
/// first signed-in account. `None` until a model list has arrived.
pub fn default_tutti_selection(groups: &[crate::view::ModelGroupView]) -> Option<SelectionView> {
    let (group, model) = groups
        .iter()
        .find_map(|g| g.models.first().map(|model| (g, model)))?;
    Some(SelectionView {
        account_label: group.account_label.clone(),
        model_id: model.id.clone(),
        model_name: model.name.clone(),
        options: Vec::new(),
    })
}

/// The title and the text of a failed pairing (Paper S5, and V2 for a version
/// error). A failure with no title of its own has `None` as its title.
pub fn pair_failure(err: &ClientError) -> (Option<String>, String) {
    match err {
        ClientError::Incompatible { title, detail } => (Some(title.clone()), detail.clone()),
        ClientError::Unreachable { url } => (
            Some("Cannot reach this address".to_owned()),
            format!("Check the address, and check that the hub runs. The app tried {url}."),
        ),
        ClientError::BadCode => (
            Some("This setup code does not work".to_owned()),
            "It is wrong, it expired, or an app used it. Run consort hub setup-code for a new code."
                .to_owned(),
        ),
        other => (None, other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{ModelChoiceView, ModelGroupView, ToolView};

    fn offline() -> Connection {
        Connection::Offline {
            retry_in_ms: 1000,
            reason: "no route".to_owned(),
        }
    }

    fn fresh_hub() -> HubFacts {
        HubFacts {
            loaded: true,
            history: History::None,
            ..HubFacts::default()
        }
    }

    fn used_hub() -> HubFacts {
        HubFacts {
            history: History::Some,
            ..fresh_hub()
        }
    }

    fn set_up_hub() -> HubFacts {
        HubFacts {
            signed_in: true,
            ..used_hub()
        }
    }

    #[test]
    fn no_key_and_no_local_hub_asks_where_consort_runs() {
        assert_eq!(
            gate(
                &Connection::NotPaired,
                LocalPresence::None,
                &HubFacts::default()
            ),
            Gate::Where
        );
    }

    #[test]
    fn no_key_and_a_local_hub_offers_to_connect_to_it() {
        assert_eq!(
            gate(
                &Connection::NotPaired,
                LocalPresence::Installed,
                &HubFacts::default()
            ),
            Gate::LocalFound
        );
    }

    #[test]
    fn no_key_waits_until_the_app_has_looked_for_a_local_hub() {
        assert_eq!(
            gate(
                &Connection::NotPaired,
                LocalPresence::Unknown,
                &HubFacts::default()
            ),
            Gate::Neutral
        );
    }

    #[test]
    fn a_key_and_no_signed_in_account_opens_the_provider_step() {
        for local in [
            LocalPresence::None,
            LocalPresence::Installed,
            LocalPresence::Unknown,
        ] {
            assert_eq!(
                gate(&Connection::Online, local, &fresh_hub()),
                Gate::Provider
            );
        }
    }

    #[test]
    fn a_signed_in_account_opens_tutti() {
        assert_eq!(
            gate(&Connection::Online, LocalPresence::None, &set_up_hub()),
            Gate::Main
        );
    }

    #[test]
    fn a_second_client_on_a_set_up_hub_goes_straight_to_tutti() {
        // It has no saved step and no greeting: only the hub state counts.
        let facts = HubFacts {
            signed_in: true,
            ..fresh_hub()
        };
        assert_eq!(
            gate(&Connection::Online, LocalPresence::None, &facts),
            Gate::Main
        );
    }

    #[test]
    fn a_refused_client_pairs_again() {
        let refused = Connection::Refused {
            reason: "revoked".to_owned(),
        };
        assert_eq!(
            gate(&refused, LocalPresence::Installed, &set_up_hub()),
            Gate::PairAgain
        );
    }

    #[test]
    fn incompatible_never_shows_setup() {
        let incompatible = Connection::Incompatible {
            too_old: consort_proto::frame::TooOld::Hub,
            hub: None,
            app_version: "0.1.0".to_owned(),
            retry_in_ms: 1000,
        };
        for hub in [HubFacts::default(), fresh_hub(), used_hub()] {
            assert_eq!(gate(&incompatible, LocalPresence::None, &hub), Gate::Main);
        }
    }

    #[test]
    fn demo_is_not_gated() {
        assert_eq!(
            gate(
                &Connection::Demo,
                LocalPresence::Unknown,
                &HubFacts::default()
            ),
            Gate::Main
        );
    }

    #[test]
    fn the_gate_is_neutral_until_the_facts_arrive() {
        let local = LocalPresence::None;
        assert_eq!(
            gate(&Connection::Connecting, local, &HubFacts::default()),
            Gate::Neutral
        );
        assert_eq!(
            gate(&Connection::Online, local, &HubFacts::default()),
            Gate::Neutral
        );
        let no_inventory = HubFacts {
            inventory_pending: true,
            ..fresh_hub()
        };
        assert_eq!(
            gate(&Connection::Online, local, &no_inventory),
            Gate::Neutral
        );
        let no_tutti = HubFacts {
            history: History::Unknown,
            ..fresh_hub()
        };
        assert_eq!(gate(&Connection::Online, local, &no_tutti), Gate::Neutral);
    }

    #[test]
    fn saved_data_of_a_set_up_client_opens_main_while_it_connects() {
        assert_eq!(
            gate(&Connection::Connecting, LocalPresence::None, &set_up_hub()),
            Gate::Main
        );
    }

    #[test]
    fn saved_data_that_says_setup_waits_for_the_hub() {
        assert_eq!(
            gate(&Connection::Connecting, LocalPresence::None, &fresh_hub()),
            Gate::Neutral
        );
    }

    #[test]
    fn offline_on_a_set_up_client_opens_main() {
        assert_eq!(
            gate(&offline(), LocalPresence::None, &set_up_hub()),
            Gate::Main
        );
    }

    #[test]
    fn offline_with_nothing_saved_opens_main_and_its_reason() {
        assert_eq!(
            gate(&offline(), LocalPresence::None, &HubFacts::default()),
            Gate::Main
        );
    }

    #[test]
    fn offline_during_setup_stays_on_the_provider_step() {
        assert_eq!(
            gate(&offline(), LocalPresence::None, &fresh_hub()),
            Gate::Provider
        );
    }

    #[test]
    fn all_accounts_signed_out_on_a_hub_with_history_opens_t8() {
        assert_eq!(
            gate(&Connection::Online, LocalPresence::None, &used_hub()),
            Gate::NoAccount
        );
    }

    #[test]
    fn t8_opens_at_once_while_the_inventory_is_on_its_way() {
        let facts = HubFacts {
            inventory_pending: true,
            ..used_hub()
        };
        assert_eq!(
            gate(&Connection::Online, LocalPresence::None, &facts),
            Gate::NoAccount
        );
    }

    #[test]
    fn do_this_later_opens_t8_and_stays_after_a_restart() {
        let facts = HubFacts {
            later: true,
            ..fresh_hub()
        };
        for connection in [Connection::Online, Connection::Connecting, offline()] {
            assert_eq!(
                gate(&connection, LocalPresence::None, &facts),
                Gate::NoAccount
            );
        }
    }

    #[test]
    fn a_sign_in_after_do_this_later_opens_tutti() {
        let facts = HubFacts {
            later: true,
            signed_in: true,
            ..fresh_hub()
        };
        assert_eq!(
            gate(&Connection::Online, LocalPresence::None, &facts),
            Gate::Main
        );
    }

    #[test]
    fn the_dabs_light_in_order() {
        assert!(!Dabs::None.hub());
        assert!(Dabs::Hub.hub() && !Dabs::Hub.account());
        assert!(Dabs::Account.account() && !Dabs::Account.assistant());
        assert!(Dabs::All.assistant());
    }

    #[test]
    fn the_providers_come_from_the_tools_of_the_host() {
        let mut host = WorkerView::default();
        assert_eq!(
            provider_choices(&host).len(),
            4,
            "an old host names no tools"
        );
        host.tools = vec![ToolView {
            name: "Codex".to_owned(),
            ..ToolView::default()
        }];
        let names: Vec<_> = provider_choices(&host).iter().map(|p| p.name).collect();
        assert_eq!(names, ["Codex"]);
    }

    #[test]
    fn the_default_account_name_is_the_short_name() {
        assert_eq!(provider_short_name("Claude Code"), "Claude");
        assert_eq!(provider_short_name("Codex"), "Codex");
        assert_eq!(provider_short_name("Gemini CLI"), "Gemini");
    }

    #[test]
    fn the_tool_stage_follows_the_tool() {
        let tool = |state| ToolView {
            state,
            version: Some("2.1".to_owned()),
            detail: Some("no network".to_owned()),
            ..ToolView::default()
        };
        assert_eq!(
            tool_stage(Some(&tool(ToolState::Missing))),
            ToolStage::Installing
        );
        assert_eq!(
            tool_stage(Some(&tool(ToolState::Installing))),
            ToolStage::Installing
        );
        assert_eq!(
            tool_stage(Some(&tool(ToolState::Failed))),
            ToolStage::Failed {
                detail: Some("no network".to_owned())
            }
        );
        assert_eq!(
            tool_stage(Some(&tool(ToolState::Ready))),
            ToolStage::Ready {
                version: Some("2.1".to_owned())
            }
        );
        assert_eq!(tool_stage(None), ToolStage::Ready { version: None });
    }

    #[test]
    fn tuttis_default_is_the_first_model_of_the_first_account() {
        assert_eq!(default_tutti_selection(&[]), None);
        let empty = ModelGroupView {
            account_label: "Empty".to_owned(),
            ..ModelGroupView::default()
        };
        assert_eq!(default_tutti_selection(std::slice::from_ref(&empty)), None);
        let claude = ModelGroupView {
            account_label: "Claude".to_owned(),
            models: vec![ModelChoiceView {
                id: "opus".to_owned(),
                name: "Opus".to_owned(),
                effort: None,
            }],
            ..ModelGroupView::default()
        };
        let selection = default_tutti_selection(&[empty, claude]).unwrap();
        assert_eq!(selection.account_label, "Claude");
        assert_eq!(selection.model_id, "opus");
    }

    #[test]
    fn a_pairing_failure_has_the_words_of_the_design() {
        let (title, detail) = pair_failure(&ClientError::Unreachable {
            url: "https://hub.example.com".to_owned(),
        });
        assert_eq!(title.as_deref(), Some("Cannot reach this address"));
        assert!(detail.contains("https://hub.example.com"));
        let (title, detail) = pair_failure(&ClientError::BadCode);
        assert_eq!(title.as_deref(), Some("This setup code does not work"));
        assert!(detail.contains("consort hub setup-code"));
        let (title, _) = pair_failure(&ClientError::NoChannel);
        assert_eq!(title, None);
    }
}
