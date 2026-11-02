//! Settings > Hub > Reach from other devices: what the hub says, as cards that
//! the app draws. The hub does the Tailscale work. This module holds the words
//! and the actions, so the desktop app and later the mobile app show the same
//! thing.

use consort_proto::auth::{PairingCode, PairingInvite};
use consort_proto::client_api::{NetworkState, ReachInfo, TailscaleState};
use consort_proto::model::Sha256Hash;

pub use consort_proto::client_api::ReachMethod;

use crate::connectors::LoadState;
use crate::view::Connection;

/// The page to get Tailscale from.
pub const GET_TAILSCALE_URL: &str = "https://tailscale.com/download";
/// The command that shows the state on a server hub.
pub const SERVER_STATUS_COMMAND: &str = "sudo consort hub reach status";
/// The command that turns on Tailscale on a server hub.
pub const SERVER_TAILSCALE_COMMAND: &str = "sudo consort hub reach tailscale";

/// The notice in Pair a new app, for a hub with only a loopback address.
pub const LOOPBACK_DEVICE_NOTICE: &str =
    "The hub has only an address on its own computer, so another device cannot use this code.";
/// The notice in Add a worker, for a hub with only a loopback address.
pub const LOOPBACK_WORKER_NOTICE: &str = "The hub has only an address on its own computer. This command works there, and not on another machine.";

/// What the cache keeps for the page.
#[derive(Clone, Debug, Default)]
pub struct ReachCache {
    pub info: Option<ReachInfo>,
    pub load: LoadState,
    /// A change that waits for the hub.
    pub busy: Option<ReachMethod>,
    /// What went wrong with the last change.
    pub error: Option<String>,
}

/// What a button does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReachAction {
    /// Turns a way on or off.
    Set { method: ReachMethod, on: bool },
    /// Opens a page in the browser.
    OpenUrl(String),
    /// Opens the Tailscale app on this computer.
    OpenTailscale,
}

/// A button on a card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReachButton {
    pub label: &'static str,
    pub action: ReachAction,
    pub primary: bool,
}

/// One state of one way, as text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReachCard {
    pub title: String,
    pub body: String,
    /// The hub answers this way: the title has a status dot.
    pub answering: bool,
    /// A check line under the text: "Tailscale runs on this computer as
    /// macbook-pro."
    pub check: Option<String>,
    /// The address, to select and copy.
    pub address: Option<String>,
    /// A command, to select and copy.
    pub command: Option<String>,
    /// A smaller line under the rest.
    pub note: Option<String>,
    /// What the program said, to select and copy.
    pub detail: Option<String>,
    pub buttons: Vec<ReachButton>,
}

/// An address that a new device or worker can use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReachRoute {
    pub method: ReachMethod,
    pub label: &'static str,
    pub hub_url: String,
    pub tls_sha256: Option<Sha256Hash>,
}

/// The page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReachView {
    pub load: LoadState,
    /// The hub may change its reach.
    pub can_change: bool,
    /// The line under the title.
    pub line: String,
    /// The short line in Settings > Hub.
    pub summary: String,
    /// `None` until the hub answers.
    pub tailscale: Option<ReachCard>,
    pub this_network: Option<ReachCard>,
    /// A change that waits for the hub.
    pub busy: Option<ReachMethod>,
    /// What went wrong with the last change.
    pub error: Option<String>,
    /// The ways that are on. Add a device shows one of them.
    pub routes: Vec<ReachRoute>,
}

/// What Add a device shows for one address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddDeviceView {
    /// What to do on the device.
    pub body: &'static str,
    pub code: String,
    pub hub_url: String,
    /// The text of the QR code: the invite as JSON.
    pub qr_text: String,
    /// `consort://pair?...`.
    pub link: String,
}

/// What Add a device shows. `None` for a code that cannot be read.
pub fn add_device(route: &ReachRoute, code: &str) -> Option<AddDeviceView> {
    let code: PairingCode = code.parse().ok()?;
    let invite = PairingInvite {
        hub_url: route.hub_url.clone(),
        code,
        tls_sha256: route.tls_sha256.clone(),
    };
    Some(AddDeviceView {
        body: match route.method {
            ReachMethod::ThisNetwork => {
                "Put the device on the same network. Then scan this code in Consort, or open the pairing link. The code works once and expires after 10 minutes."
            }
            _ => {
                "Sign in to Tailscale on the device. Then scan this code in Consort, or enter the address and the setup code. The code works once and expires after 10 minutes."
            }
        },
        code: invite.code.to_string(),
        hub_url: route.hub_url.clone(),
        qr_text: serde_json::to_string(&invite).ok()?,
        link: invite.to_link(),
    })
}

fn set(method: ReachMethod, on: bool, label: &'static str, primary: bool) -> ReachButton {
    ReachButton {
        label,
        action: ReachAction::Set { method, on },
        primary,
    }
}

/// The note about the certificate log.
fn certificate_note(address: &str, name: &str) -> String {
    format!(
        "The hub gets the address {address}. The HTTPS certificate puts the name {name} in public certificate logs. Your tailnet stays private."
    )
}

const TAILSCALE_BODY: &str = "Works from anywhere, with no open ports. Only devices on your tailnet can reach the hub. Each device still needs a pairing code.";
const PORT_IN_USE_TITLE: &str = "Another service uses the main address";
const PORT_IN_USE_BODY: &str = "Consort leaves that service alone, and uses port 8443 for the hub.";

/// The card of Tailscale. A hub that may not change it gets a command in place
/// of a button.
pub fn tailscale_card(state: &TailscaleState, can_change: bool) -> ReachCard {
    let t = ReachMethod::Tailscale;
    match state {
        TailscaleState::NotInstalled => ReachCard {
            title: if can_change {
                "Tailscale is not on this computer".to_owned()
            } else {
                "Tailscale is not on the server".to_owned()
            },
            body: "Install it and sign in. This page finds it by itself.".to_owned(),
            buttons: can_change
                .then(|| ReachButton {
                    label: "Get Tailscale",
                    action: ReachAction::OpenUrl(GET_TAILSCALE_URL.to_owned()),
                    primary: false,
                })
                .into_iter()
                .collect(),
            ..ReachCard::default()
        },
        TailscaleState::NotConnected => ReachCard {
            title: "Tailscale is not connected".to_owned(),
            body: if can_change {
                "Open Tailscale and sign in. This page finds it by itself.".to_owned()
            } else {
                "Start Tailscale on the server and sign in. This page finds it by itself."
                    .to_owned()
            },
            buttons: can_change
                .then_some(ReachButton {
                    label: "Open Tailscale",
                    action: ReachAction::OpenTailscale,
                    primary: false,
                })
                .into_iter()
                .collect(),
            ..ReachCard::default()
        },
        TailscaleState::HttpsOff { action_url, detail } => {
            let asked = detail.is_some();
            let mut buttons = vec![ReachButton {
                label: "Open Tailscale settings",
                action: ReachAction::OpenUrl(action_url.clone()),
                primary: true,
            }];
            if can_change && !asked {
                buttons.push(set(t, true, "Turn on", false));
            }
            ReachCard {
                title: "Your tailnet needs HTTPS".to_owned(),
                body: if asked {
                    "An admin of the tailnet must turn it on, one time. Tailscale gives this link. The hub turns on after that, with no other step."
                        .to_owned()
                } else {
                    "An admin of the tailnet must turn it on, one time. Then turn on Tailscale here."
                        .to_owned()
                },
                detail: detail.clone(),
                buttons,
                ..ReachCard::default()
            }
        }
        TailscaleState::NeedsPermission { command, detail } => ReachCard {
            title: "Consort cannot change Tailscale yet".to_owned(),
            body: "Your user needs permission. Run this one time, and then try again.".to_owned(),
            command: Some(command.clone()),
            detail: Some(detail.clone()),
            buttons: vec![set(t, true, "Try again", false)],
            ..ReachCard::default()
        },
        TailscaleState::Available {
            name,
            address,
            port_in_use,
        } => {
            if !can_change {
                return ReachCard {
                    title: "The hub does not use Tailscale".to_owned(),
                    body: "To turn it on, run this on the server.".to_owned(),
                    command: Some(SERVER_TAILSCALE_COMMAND.to_owned()),
                    note: Some(certificate_note(address, name)),
                    ..ReachCard::default()
                };
            }
            let (title, body) = if *port_in_use {
                (PORT_IN_USE_TITLE, PORT_IN_USE_BODY)
            } else {
                ("Tailscale", TAILSCALE_BODY)
            };
            ReachCard {
                title: title.to_owned(),
                body: body.to_owned(),
                check: Some(format!("Tailscale runs on this computer as {name}.")),
                address: port_in_use.then(|| address.clone()),
                note: Some(certificate_note(address, name)),
                buttons: vec![set(t, true, "Turn on", true)],
                ..ReachCard::default()
            }
        }
        TailscaleState::NoFreePort { .. } => ReachCard {
            title: "Other services use both addresses".to_owned(),
            body: "Consort leaves them alone. Free port 443 or 8443 in Tailscale. This page finds it by itself."
                .to_owned(),
            ..ReachCard::default()
        },
        TailscaleState::On {
            address,
            port_in_use,
        } => {
            let note = port_in_use.then(|| PORT_IN_USE_BODY.to_owned());
            if can_change {
                ReachCard {
                    title: "The hub answers on your tailnet".to_owned(),
                    answering: true,
                    address: Some(address.clone()),
                    note,
                    buttons: vec![set(t, false, "Turn off", false)],
                    ..ReachCard::default()
                }
            } else {
                ReachCard {
                    title: "The hub uses Tailscale".to_owned(),
                    body: "The install script turned this on. To change it, run this on the server."
                        .to_owned(),
                    answering: true,
                    address: Some(address.clone()),
                    command: Some(SERVER_STATUS_COMMAND.to_owned()),
                    note,
                    ..ReachCard::default()
                }
            }
        }
        TailscaleState::Removed { address } => ReachCard {
            title: "Your other devices cannot reach the hub".to_owned(),
            body: if can_change {
                "The hub is no longer on your tailnet. Tailscale was reset or changed on this computer."
                    .to_owned()
            } else {
                "The hub is no longer on your tailnet. Tailscale was reset or changed on the server. To add it again, run this on the server."
                    .to_owned()
            },
            address: Some(address.clone()),
            command: (!can_change).then(|| SERVER_TAILSCALE_COMMAND.to_owned()),
            buttons: if can_change {
                vec![
                    set(t, true, "Turn on again", false),
                    set(t, false, "Turn off", false),
                ]
            } else {
                Vec::new()
            },
            ..ReachCard::default()
        },
        TailscaleState::Failed { detail } => ReachCard {
            title: "Tailscale gave an error".to_owned(),
            body: "This is what it said. Fix the problem, then try again.".to_owned(),
            detail: Some(detail.clone()),
            buttons: if can_change {
                vec![set(t, true, "Try again", false)]
            } else {
                Vec::new()
            },
            ..ReachCard::default()
        },
        TailscaleState::Unknown => ReachCard {
            title: "This hub is newer than this app".to_owned(),
            body: "Update Consort to see the state of Tailscale.".to_owned(),
            ..ReachCard::default()
        },
    }
}

/// The card of This network.
pub fn network_card(state: &NetworkState, can_change: bool) -> ReachCard {
    let n = ReachMethod::ThisNetwork;
    match state {
        NetworkState::Off => ReachCard {
            title: "This network".to_owned(),
            body: "Works only while the device is on the same network as this computer. Traffic is encrypted with a key that the hub makes. Each device must scan a code or open a pairing link."
                .to_owned(),
            buttons: if can_change {
                vec![set(n, true, "Turn on", false)]
            } else {
                Vec::new()
            },
            note: (!can_change)
                .then(|| "A hub on a server does not open this port. Use Tailscale or a reverse proxy.".to_owned()),
            ..ReachCard::default()
        },
        NetworkState::On { address, .. } => ReachCard {
            title: "The hub answers on this network".to_owned(),
            body: "Add a device with its code or pairing link. Typing the address is not enough, because the device must also learn the key of the hub."
                .to_owned(),
            answering: true,
            address: Some(address.clone()),
            buttons: if can_change {
                vec![set(n, false, "Turn off", false)]
            } else {
                Vec::new()
            },
            ..ReachCard::default()
        },
        NetworkState::Failed { address, detail } => ReachCard {
            title: "This network did not start".to_owned(),
            body: "The hub could not open its port. Turn this off, and then on again.".to_owned(),
            address: Some(address.clone()),
            detail: Some(detail.clone()),
            buttons: if can_change {
                vec![set(n, false, "Turn off", false)]
            } else {
                Vec::new()
            },
            ..ReachCard::default()
        },
        NetworkState::Unknown => ReachCard {
            title: "This hub is newer than this app".to_owned(),
            body: "Update Consort to see the state of this network.".to_owned(),
            ..ReachCard::default()
        },
    }
}

/// The ways that are on, Tailscale first.
fn routes_of(info: &ReachInfo) -> Vec<ReachRoute> {
    let mut routes = Vec::new();
    if let TailscaleState::On { address, .. } = &info.tailscale {
        routes.push(ReachRoute {
            method: ReachMethod::Tailscale,
            label: "Tailscale",
            hub_url: address.clone(),
            tls_sha256: None,
        });
    }
    if let NetworkState::On {
        address,
        tls_sha256,
    } = &info.this_network
    {
        routes.push(ReachRoute {
            method: ReachMethod::ThisNetwork,
            label: "This network",
            hub_url: address.clone(),
            tls_sha256: Some(tls_sha256.clone()),
        });
    }
    routes
}

/// The host of a URL, or the URL.
fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| url.to_owned())
}

/// True for an address that works only on the computer of the hub.
pub fn is_loopback_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    match parsed.host() {
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Builds the page. `own_url` is the address of this app. It stays as it is
/// when a way turns on, because the saved data of the app belongs to it.
pub fn reach_view(cache: &ReachCache, own_url: &str) -> ReachView {
    let Some(info) = &cache.info else {
        return ReachView {
            load: cache.load.clone(),
            busy: cache.busy,
            error: cache.error.clone(),
            line: String::new(),
            summary: "Not known yet".to_owned(),
            ..ReachView::default()
        };
    };
    let routes = routes_of(info);
    let names: Vec<&str> = routes.iter().map(|r| r.label).collect();
    let line = match names.as_slice() {
        [] if is_loopback_url(own_url) => {
            "Only this computer can reach the hub now. Choose how your phone and your other computers get to it."
                .to_owned()
        }
        [] => "Choose how your phone and your other computers get to the hub.".to_owned(),
        names => format!(
            "{} {} on. This app still uses {}, and nothing changes for it.",
            names.join(" and "),
            if names.len() == 1 { "is" } else { "are" },
            host_of(own_url)
        ),
    };
    let summary = match names.as_slice() {
        [] => "Off".to_owned(),
        names => format!("{} on", names.join(" and ")),
    };
    ReachView {
        load: LoadState::Loaded,
        can_change: info.can_change,
        line,
        summary,
        tailscale: Some(tailscale_card(&info.tailscale, info.can_change)),
        this_network: Some(network_card(&info.this_network, info.can_change)),
        busy: cache.busy,
        error: cache.error.clone(),
        routes,
    }
}

/// The error of a client whose hub is on the tailnet and that cannot connect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailnetNotice {
    pub title: &'static str,
    pub body: &'static str,
}

/// The notice for a link that is down to a `.ts.net` address. The usual reason
/// on such a device is that Tailscale is off or signed out.
pub fn tailnet_notice(hub_url: &str, connection: &Connection) -> Option<TailnetNotice> {
    let on_tailnet = url::Url::parse(hub_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| host.trim_end_matches('.').ends_with(".ts.net"));
    (on_tailnet && matches!(connection, Connection::Offline { .. })).then_some(TailnetNotice {
        title: "This device cannot reach your tailnet",
        body: "The hub address is on Tailscale. Open Tailscale on this device and sign in. Your messages wait here.",
    })
}

/// True for text that a user pastes in place of an address: a pairing link or
/// the text of a QR code. Such text holds the code too.
pub fn is_invite_text(text: &str) -> bool {
    !matches!(PairingInvite::parse_text(text), Ok(None))
}

/// Opens the Tailscale app on this computer. `false` when this system has no
/// such step, and the caller opens the download page.
pub fn open_tailscale_app() -> bool {
    if cfg!(target_os = "macos") {
        return std::process::Command::new("open")
            .args(["-a", "Tailscale"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use consort_proto::client_api::PairingAddress;

    #[test]
    fn a_pairing_link_is_invite_text_and_an_address_is_not() {
        assert!(is_invite_text(
            "consort://pair?hub=https%3A%2F%2Fh&code=ABCD-EFGH"
        ));
        assert!(is_invite_text(
            r#"{"hub_url":"https://h","code":"ABCD-EFGH"}"#
        ));
        // A damaged link goes on to the pairing, which says what is wrong.
        assert!(is_invite_text("consort://pair?nope"));
        assert!(!is_invite_text("https://hub.example.com"));
    }

    const ADDRESS: &str = "https://macbook-pro.tail1234.ts.net";

    fn info(tailscale: TailscaleState, this_network: NetworkState) -> ReachInfo {
        ReachInfo {
            can_change: true,
            tailscale,
            this_network,
            pairing: PairingAddress {
                hub_url: "http://127.0.0.1:7600".to_owned(),
                tls_sha256: None,
            },
        }
    }

    fn available() -> TailscaleState {
        TailscaleState::Available {
            name: "macbook-pro".to_owned(),
            address: ADDRESS.to_owned(),
            port_in_use: false,
        }
    }

    #[test]
    fn r1_has_the_words_of_the_design_and_the_certificate_note() {
        let view = reach_view(
            &ReachCache {
                info: Some(info(available(), NetworkState::Off)),
                ..ReachCache::default()
            },
            "http://127.0.0.1:7600",
        );
        assert!(
            view.line
                .starts_with("Only this computer can reach the hub")
        );
        assert_eq!(view.summary, "Off");
        let card = view.tailscale.unwrap();
        assert_eq!(card.title, "Tailscale");
        assert_eq!(
            card.check.as_deref(),
            Some("Tailscale runs on this computer as macbook-pro.")
        );
        assert!(card.note.unwrap().contains("public certificate logs"));
        assert_eq!(card.buttons[0].label, "Turn on");
        assert!(card.buttons[0].primary);
        assert_eq!(
            card.buttons[0].action,
            ReachAction::Set {
                method: ReachMethod::Tailscale,
                on: true
            }
        );
        assert!(!view.this_network.unwrap().buttons[0].primary);
        assert!(view.routes.is_empty());
    }

    #[test]
    fn r2_says_that_this_app_keeps_its_address() {
        let view = reach_view(
            &ReachCache {
                info: Some(info(
                    TailscaleState::On {
                        address: ADDRESS.to_owned(),
                        port_in_use: false,
                    },
                    NetworkState::Off,
                )),
                ..ReachCache::default()
            },
            "http://127.0.0.1:7600",
        );
        assert_eq!(
            view.line,
            "Tailscale is on. This app still uses 127.0.0.1, and nothing changes for it."
        );
        let card = view.tailscale.unwrap();
        assert!(card.answering);
        assert_eq!(card.address.as_deref(), Some(ADDRESS));
        assert_eq!(view.routes[0].hub_url, ADDRESS);
    }

    #[test]
    fn a_hub_that_may_not_change_gets_commands_and_no_buttons() {
        for state in [
            available(),
            TailscaleState::On {
                address: ADDRESS.to_owned(),
                port_in_use: false,
            },
            TailscaleState::Removed {
                address: ADDRESS.to_owned(),
            },
            TailscaleState::NotInstalled,
            TailscaleState::NotConnected,
        ] {
            let card = tailscale_card(&state, false);
            assert!(card.buttons.is_empty(), "{state:?}");
        }
        assert_eq!(
            tailscale_card(&available(), false).command.as_deref(),
            Some(SERVER_TAILSCALE_COMMAND)
        );
        assert_eq!(
            tailscale_card(
                &TailscaleState::On {
                    address: ADDRESS.to_owned(),
                    port_in_use: false
                },
                false
            )
            .command
            .as_deref(),
            Some(SERVER_STATUS_COMMAND)
        );
        assert!(network_card(&NetworkState::Off, false).buttons.is_empty());
    }

    #[test]
    fn https_off_has_the_link_from_tailscale_and_the_output_for_details() {
        let card = tailscale_card(
            &TailscaleState::HttpsOff {
                action_url: "https://login.tailscale.com/f/serve?node=abc".to_owned(),
                detail: Some("Serve is not enabled".to_owned()),
            },
            true,
        );
        assert_eq!(card.buttons.len(), 1);
        assert!(card.buttons[0].primary);
        assert_eq!(
            card.buttons[0].action,
            ReachAction::OpenUrl("https://login.tailscale.com/f/serve?node=abc".to_owned())
        );
        assert_eq!(card.detail.as_deref(), Some("Serve is not enabled"));
        assert!(card.body.contains("with no other step"));
    }

    #[test]
    fn someone_removed_the_entry_offers_both_decisions() {
        let card = tailscale_card(
            &TailscaleState::Removed {
                address: ADDRESS.to_owned(),
            },
            true,
        );
        assert_eq!(card.title, "Your other devices cannot reach the hub");
        let labels: Vec<_> = card.buttons.iter().map(|b| b.label).collect();
        assert_eq!(labels, ["Turn on again", "Turn off"]);
    }

    #[test]
    fn add_device_holds_the_pin_only_for_this_network() {
        let tailnet = ReachRoute {
            method: ReachMethod::Tailscale,
            label: "Tailscale",
            hub_url: ADDRESS.to_owned(),
            tls_sha256: None,
        };
        let view = add_device(&tailnet, "ABCD-EFGH").unwrap();
        assert_eq!(
            view.qr_text,
            format!(r#"{{"hub_url":"{ADDRESS}","code":"ABCD-EFGH"}}"#)
        );
        assert!(!view.link.contains("tls_sha256"));

        let network = ReachRoute {
            method: ReachMethod::ThisNetwork,
            label: "This network",
            hub_url: "https://mac.local:7601".to_owned(),
            tls_sha256: Some(Sha256Hash::from_bytes([7; 32])),
        };
        let view = add_device(&network, "ABCD-EFGH").unwrap();
        assert!(view.qr_text.contains("tls_sha256"));
        assert!(view.link.contains("tls_sha256=0707"));
        assert!(add_device(&network, "nope").is_none());
    }

    #[test]
    fn only_a_hub_on_the_tailnet_gets_the_tailnet_notice() {
        let offline = Connection::Offline {
            retry_in_ms: 1,
            reason: String::new(),
        };
        assert!(tailnet_notice("https://mac.tail1234.ts.net:8443", &offline).is_some());
        assert!(tailnet_notice("https://hub.example.com", &offline).is_none());
        assert!(tailnet_notice("https://mac.tail1234.ts.net", &Connection::Online).is_none());
    }

    #[test]
    fn loopback_addresses_are_found() {
        for url in [
            "http://127.0.0.1:7600",
            "http://localhost:7600",
            "http://[::1]:7600",
            "http://x.localhost",
        ] {
            assert!(is_loopback_url(url), "{url}");
        }
        for url in [
            "http://192.168.1.5:7600",
            "https://hub.example.com",
            "https://m.ts.net",
            "nope",
        ] {
            assert!(!is_loopback_url(url), "{url}");
        }
    }
}
