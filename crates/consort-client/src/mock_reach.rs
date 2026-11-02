//! The reach screens of the designs (Paper R1, R2 and R3), for
//! `--mock --screen reach-off`, `reach-on` and `reach-states`.

use consort_proto::client_api::{
    NetworkState, PairingAddress, ReachInfo, ReachMethod, TailscaleState,
};
use consort_proto::model::Sha256Hash;

use crate::admin::{ClientPairingView, PAIRING_CODE_MS, qr_text};
use crate::connectors::LoadState;
use crate::format;
use crate::mock;
use crate::reach::{ReachCache, ReachCard, ReachView, network_card, reach_view, tailscale_card};
use crate::view::ViewState;

const OWN_URL: &str = "http://127.0.0.1:7600";
const TAILNET_URL: &str = "https://macbook-pro.tail1234.ts.net";
const TAILNET_8443: &str = "https://macbook-pro.tail1234.ts.net:8443";
const NETWORK_URL: &str = "https://macbook-pro.local:7601";

fn available() -> TailscaleState {
    TailscaleState::Available {
        name: "macbook-pro".to_owned(),
        address: TAILNET_URL.to_owned(),
        port_in_use: false,
    }
}

fn network_on() -> NetworkState {
    NetworkState::On {
        address: NETWORK_URL.to_owned(),
        tls_sha256: Sha256Hash::from_bytes([0x5a; 32]),
    }
}

/// The page with these states.
pub fn view(tailscale: TailscaleState, this_network: NetworkState) -> ReachView {
    let mut view = reach_view(
        &ReachCache {
            info: Some(ReachInfo {
                can_change: true,
                tailscale,
                this_network,
                pairing: PairingAddress {
                    hub_url: OWN_URL.to_owned(),
                    tls_sha256: None,
                },
            }),
            load: LoadState::Loaded,
            ..ReachCache::default()
        },
        OWN_URL,
    );
    view.load = LoadState::Loaded;
    view
}

/// R1: nothing is on.
pub fn off() -> ReachView {
    view(available(), NetworkState::Off)
}

/// R2: Tailscale is on.
pub fn on() -> ReachView {
    view(
        TailscaleState::On {
            address: TAILNET_URL.to_owned(),
            port_in_use: false,
        },
        NetworkState::Off,
    )
}

/// The turned-on version of the page after a command in the mock.
pub fn after_set(current: &ReachView, method: ReachMethod, on_now: bool) -> ReachView {
    let tailscale_on = current
        .routes
        .iter()
        .any(|r| r.method == ReachMethod::Tailscale);
    let network = current
        .routes
        .iter()
        .any(|r| r.method == ReachMethod::ThisNetwork);
    let tailscale_on = if method == ReachMethod::Tailscale {
        on_now
    } else {
        tailscale_on
    };
    let network = if method == ReachMethod::ThisNetwork {
        on_now
    } else {
        network
    };
    view(
        if tailscale_on {
            TailscaleState::On {
                address: TAILNET_URL.to_owned(),
                port_in_use: false,
            }
        } else {
            available()
        },
        if network {
            network_on()
        } else {
            NetworkState::Off
        },
    )
}

/// R3: every state, each with the label of the design.
pub fn gallery() -> Vec<(&'static str, ReachCard)> {
    let tailscale = |state: TailscaleState, can_change: bool| tailscale_card(&state, can_change);
    vec![
        (
            "Tailscale, not installed",
            tailscale(TailscaleState::NotInstalled, true),
        ),
        (
            "Tailscale, stopped or signed out",
            tailscale(TailscaleState::NotConnected, true),
        ),
        (
            "Tailscale, HTTPS is off for the tailnet",
            tailscale(
                TailscaleState::HttpsOff {
                    action_url: "https://login.tailscale.com/f/serve?node=abc".to_owned(),
                    detail: Some(
                        "Serve is not enabled on your tailnet.\nTo enable, visit: https://login.tailscale.com/f/serve?node=abc"
                            .to_owned(),
                    ),
                },
                true,
            ),
        ),
        (
            "Tailscale, port 443 is in use",
            tailscale(
                TailscaleState::Available {
                    name: "macbook-pro".to_owned(),
                    address: TAILNET_8443.to_owned(),
                    port_in_use: true,
                },
                true,
            ),
        ),
        (
            "Tailscale, Linux needs one permission",
            tailscale(
                TailscaleState::NeedsPermission {
                    command: "sudo tailscale set --operator=$USER".to_owned(),
                    detail: "Access denied: serve config denied".to_owned(),
                },
                true,
            ),
        ),
        (
            "Server hub, the app can only show this",
            tailscale(
                TailscaleState::On {
                    address: TAILNET_URL.to_owned(),
                    port_in_use: false,
                },
                false,
            ),
        ),
        (
            "Later, someone removed the Tailscale address",
            tailscale(
                TailscaleState::Removed {
                    address: TAILNET_URL.to_owned(),
                },
                true,
            ),
        ),
        ("This network, on", network_card(&network_on(), true)),
    ]
}

/// The state of a reach screen.
pub fn screen_state(screen: &str) -> Option<ViewState> {
    let reach = match screen {
        "reach-off" | "reach-states" | "pair-client-loopback" | "add-worker-loopback" => off(),
        "reach-on" => on(),
        "reach-unreachable" => {
            let mut state = crate::mock_offline::hub_offline();
            state.tailnet_notice = crate::reach::tailnet_notice(TAILNET_URL, &state.connection);
            return Some(state);
        }
        _ => return None,
    };
    let mut state = mock::view_state();
    state.admin.reach = reach;
    if screen == "pair-client-loopback" {
        state.admin.pairing = Some(ClientPairingView {
            code: "ABCD-EFGH".to_owned(),
            hub_url: OWN_URL.to_owned(),
            expires_at: format::now_ms() + PAIRING_CODE_MS,
            qr_text: qr_text(OWN_URL, "ABCD-EFGH", None),
            tls_sha256: None,
            loopback_only: true,
            paired: None,
        });
    }
    if screen == "add-worker-loopback" {
        state.worker_pairing = Some(crate::view::PairingCodeView {
            code: "ABCD-EFGH".to_owned(),
            hub_url: OWN_URL.to_owned(),
            tls_sha256: None,
            expires_at: format::now_ms() + PAIRING_CODE_MS,
            loopback_only: true,
        });
    }
    if screen == "reach-on" {
        state.admin.pairing = Some(ClientPairingView {
            code: "ABCD-EFGH".to_owned(),
            hub_url: TAILNET_URL.to_owned(),
            expires_at: format::now_ms() + PAIRING_CODE_MS,
            qr_text: qr_text(TAILNET_URL, "ABCD-EFGH", None),
            tls_sha256: None,
            loopback_only: false,
            paired: None,
        });
    }
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_of_the_design_has_a_card() {
        let cards = gallery();
        assert_eq!(cards.len(), 8);
        for (label, card) in &cards {
            assert!(!card.title.is_empty(), "{label}");
        }
        assert!(screen_state("reach-off").is_some());
        assert_eq!(
            screen_state("reach-on").unwrap().admin.reach.routes.len(),
            1
        );
        assert!(screen_state("hub").is_none());
        assert!(
            screen_state("reach-unreachable")
                .unwrap()
                .tailnet_notice
                .is_some()
        );
    }

    #[test]
    fn the_mock_turns_a_way_on_and_off() {
        let page = after_set(&off(), ReachMethod::ThisNetwork, true);
        assert_eq!(page.routes.len(), 1);
        let page = after_set(&page, ReachMethod::Tailscale, true);
        assert_eq!(page.routes.len(), 2);
        let page = after_set(&page, ReachMethod::ThisNetwork, false);
        assert_eq!(page.routes.len(), 1);
    }
}
