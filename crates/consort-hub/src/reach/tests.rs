//! The Tailscale logic against a fake program. Nothing here runs Tailscale.

use std::sync::{Arc, Mutex};

use consort_proto::client_api::{NetworkState, ReachMethod, TailscaleState};
use consort_proto::time::Timestamp;

use super::program::{ProgramError, ProgramOutput, TailscaleProgram};
use super::tailscale::{self, TailnetRead, classify_failure, parse_serve, parse_status};
use super::{cli, local_host, serve_target, shown_state};
use crate::store::reach::{self as reach_store, ReachRow, TARGET_KEY};

const TARGET: &str = "http://127.0.0.1:7600";
const DNS: &str = "macbook-pro.tail1234.ts.net";

/// A fake `tailscale` with a Serve table that follows the commands.
#[derive(Default)]
pub(crate) struct FakeTailscale {
    pub missing: bool,
    pub backend: Mutex<String>,
    pub https: Mutex<bool>,
    /// `<port>` to `<proxy>`.
    pub entries: Mutex<Vec<(u16, String)>>,
    /// The answer to a `serve --bg`, in place of adding the entry.
    pub serve_answer: Mutex<Option<ProgramOutput>>,
    pub calls: Mutex<Vec<String>>,
}

impl FakeTailscale {
    fn running() -> Arc<Self> {
        Arc::new(Self {
            backend: Mutex::new("Running".to_owned()),
            https: Mutex::new(true),
            ..Self::default()
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn changes(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|c| c.starts_with("serve --"))
            .collect()
    }
}

#[async_trait::async_trait]
impl TailscaleProgram for FakeTailscale {
    async fn run(&self, args: &[String]) -> Result<ProgramOutput, ProgramError> {
        if self.missing {
            return Err(ProgramError::NotFound);
        }
        let line = args.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        let ok = |stdout: String| ProgramOutput {
            success: true,
            stdout,
            ..ProgramOutput::default()
        };
        if line == "status --json" {
            let backend = self.backend.lock().unwrap().clone();
            let caps = if *self.https.lock().unwrap() {
                r#"["funnel","https"]"#
            } else {
                r#"["funnel","https://tailscale.com/cap/ssh"]"#
            };
            return Ok(ok(format!(
                r#"{{"BackendState":"{backend}","Self":{{"DNSName":"{DNS}.","Capabilities":{caps}}},"CurrentTailnet":{{"MagicDNSEnabled":true}}}}"#
            )));
        }
        if line == "serve status --json" {
            let entries = self.entries.lock().unwrap();
            let tcp: Vec<String> = entries
                .iter()
                .map(|(port, _)| format!(r#""{port}":{{"HTTPS":true}}"#))
                .collect();
            let web: Vec<String> = entries
                .iter()
                .map(|(port, proxy)| {
                    format!(r#""{DNS}:{port}":{{"Handlers":{{"/":{{"Proxy":"{proxy}"}}}}}}"#)
                })
                .collect();
            return Ok(ok(format!(
                r#"{{"TCP":{{{}}},"Web":{{{}}}}}"#,
                tcp.join(","),
                web.join(",")
            )));
        }
        if args.first().map(String::as_str) == Some("serve")
            && args.last().map(String::as_str) == Some("off")
        {
            let port: u16 = args[2].trim_start_matches("--https=").parse().unwrap();
            self.entries.lock().unwrap().retain(|(p, _)| *p != port);
            return Ok(ok(String::new()));
        }
        if args.first().map(String::as_str) == Some("serve")
            && args.get(1).map(String::as_str) == Some("--bg")
        {
            if let Some(answer) = self.serve_answer.lock().unwrap().clone() {
                return Ok(answer);
            }
            let port: u16 = args[3].trim_start_matches("--https=").parse().unwrap();
            self.entries.lock().unwrap().push((port, args[4].clone()));
            return Ok(ok(format!("Available within your tailnet: https://{DNS}")));
        }
        panic!("a command that Consort must not run: {line}");
    }
}

fn row(port: u16, target: &str) -> ReachRow {
    ReachRow {
        method: ReachMethod::Tailscale,
        address: tailscale::address(DNS, port),
        port,
        target: Some(target.to_owned()),
        turned_on_at: Timestamp::from_millis(1),
    }
}

#[test]
fn the_status_json_gives_the_name_and_the_https_capability() {
    let read = parse_status(
        r#"{"BackendState":"Running","Self":{"DNSName":"mac.tail.ts.net.","Capabilities":["https"]},"CurrentTailnet":{"MagicDNSEnabled":true}}"#,
    );
    let TailnetRead::Ready(tailnet) = read else {
        panic!("{read:?}")
    };
    assert_eq!(tailnet.name, "mac");
    assert_eq!(tailnet.dns, "mac.tail.ts.net");
    assert!(tailnet.https);
    // The newer map holds the same capability.
    let read = parse_status(
        r#"{"BackendState":"Running","Self":{"DNSName":"m.t.ts.net.","CapMap":{"https":null}},"CurrentTailnet":{"MagicDNSEnabled":true}}"#,
    );
    assert!(matches!(read, TailnetRead::Ready(t) if t.https));
    // A capability with https in its URL is not the capability.
    let read = parse_status(
        r#"{"BackendState":"Running","Self":{"DNSName":"m.t.ts.net.","Capabilities":["https://tailscale.com/cap/ssh"]},"CurrentTailnet":{"MagicDNSEnabled":true}}"#,
    );
    assert!(matches!(read, TailnetRead::Ready(t) if !t.https));
    // No MagicDNS means no certificate.
    let read = parse_status(
        r#"{"BackendState":"Running","Self":{"DNSName":"m.t.ts.net.","Capabilities":["https"]},"CurrentTailnet":{"MagicDNSEnabled":false}}"#,
    );
    assert!(matches!(read, TailnetRead::Ready(t) if !t.https));
    for stopped in ["Stopped", "NeedsLogin", "NoState", "Starting"] {
        let read = parse_status(&format!(r#"{{"BackendState":"{stopped}"}}"#));
        assert_eq!(read, TailnetRead::NotConnected, "{stopped}");
    }
    assert!(matches!(parse_status("not json"), TailnetRead::Failed(_)));
}

#[test]
fn the_serve_json_gives_ports_and_proxies_also_for_foreground_entries() {
    let config = parse_serve(
        r#"{"TCP":{"443":{"HTTPS":true},"2222":{"TCPForward":"127.0.0.1:22"}},"Web":{"m.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:3000"}}},"m.ts.net:10000":{"Handlers":{"/api":{"Proxy":"http://x"}}}},"Foreground":{"abc":{"TCP":{"8443":{"HTTPS":true}}}}}"#,
    )
    .unwrap();
    assert_eq!(
        config.ports.iter().copied().collect::<Vec<_>>(),
        vec![443, 2222, 8443, 10000]
    );
    assert_eq!(
        config
            .proxies
            .get(&("m.ts.net".to_owned(), 443))
            .map(String::as_str),
        Some("http://127.0.0.1:3000")
    );
    assert_eq!(config.proxies.len(), 1);
    assert_eq!(parse_serve("").unwrap(), Default::default());
    assert_eq!(parse_serve("{}").unwrap(), Default::default());
    assert!(parse_serve("Error: nope").is_err());
}

#[tokio::test]
async fn each_state_of_the_tailnet_shows() {
    let fake = FakeTailscale {
        missing: true,
        ..Default::default()
    };
    assert_eq!(
        tailscale::read_state(&fake, None, TARGET).await,
        TailscaleState::NotInstalled
    );

    let fake = FakeTailscale::running();
    *fake.backend.lock().unwrap() = "NeedsLogin".to_owned();
    assert_eq!(
        tailscale::read_state(fake.as_ref(), None, TARGET).await,
        TailscaleState::NotConnected
    );

    let fake = FakeTailscale::running();
    *fake.https.lock().unwrap() = false;
    assert!(matches!(
        tailscale::read_state(fake.as_ref(), None, TARGET).await,
        TailscaleState::HttpsOff { action_url, detail: None } if action_url.starts_with("https://")
    ));

    let fake = FakeTailscale::running();
    assert_eq!(
        tailscale::read_state(fake.as_ref(), None, TARGET).await,
        TailscaleState::Available {
            name: "macbook-pro".to_owned(),
            address: format!("https://{DNS}"),
            port_in_use: false,
        }
    );
}

#[tokio::test]
async fn port_443_in_use_gives_8443_and_the_other_entry_stays() {
    let fake = FakeTailscale::running();
    fake.entries
        .lock()
        .unwrap()
        .push((443, "http://127.0.0.1:3000".to_owned()));
    assert_eq!(
        tailscale::read_state(fake.as_ref(), None, TARGET).await,
        TailscaleState::Available {
            name: "macbook-pro".to_owned(),
            address: format!("https://{DNS}:8443"),
            port_in_use: true,
        }
    );
    let on = tailscale::turn_on(fake.as_ref(), None, TARGET)
        .await
        .unwrap();
    assert_eq!(on.port, 8443);
    assert_eq!(on.address, format!("https://{DNS}:8443"));
    assert_eq!(
        fake.changes(),
        vec![format!("serve --bg --yes --https=8443 {TARGET}")]
    );
    let turned_on = row(8443, TARGET);
    tailscale::turn_off(fake.as_ref(), &turned_on)
        .await
        .unwrap();
    assert_eq!(
        *fake.entries.lock().unwrap(),
        vec![(443, "http://127.0.0.1:3000".to_owned())],
        "only the entry of the hub goes"
    );
    assert_eq!(
        fake.changes().last().unwrap(),
        &format!("serve --yes --https=8443 {TARGET} off")
    );
    assert!(
        fake.calls()
            .iter()
            .all(|c| !c.contains("reset") && !c.contains("funnel"))
    );
}

#[tokio::test]
async fn both_ports_in_use_change_nothing() {
    let fake = FakeTailscale::running();
    {
        let mut entries = fake.entries.lock().unwrap();
        entries.push((443, "http://a".to_owned()));
        entries.push((8443, "http://b".to_owned()));
    }
    assert!(matches!(
        tailscale::read_state(fake.as_ref(), None, TARGET).await,
        TailscaleState::NoFreePort { .. }
    ));
    assert!(matches!(
        tailscale::turn_on(fake.as_ref(), None, TARGET).await,
        Err(TailscaleState::NoFreePort { .. })
    ));
    assert!(fake.changes().is_empty());
}

#[tokio::test]
async fn an_entry_of_this_hub_is_found_and_not_added_twice() {
    let fake = FakeTailscale::running();
    fake.entries.lock().unwrap().push((8443, TARGET.to_owned()));
    let on = tailscale::turn_on(fake.as_ref(), None, TARGET)
        .await
        .unwrap();
    assert_eq!(on.port, 8443);
    assert!(fake.changes().is_empty());
}

#[tokio::test]
async fn an_entry_that_someone_changed_does_not_go_with_turn_off() {
    let fake = FakeTailscale::running();
    fake.entries
        .lock()
        .unwrap()
        .push((443, "http://127.0.0.1:3000".to_owned()));
    tailscale::turn_off(fake.as_ref(), &row(443, TARGET))
        .await
        .unwrap();
    assert!(fake.changes().is_empty());
    assert_eq!(fake.entries.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_saved_reach_with_no_entry_reads_as_removed() {
    let fake = FakeTailscale::running();
    let saved = row(443, TARGET);
    assert_eq!(
        tailscale::read_state(fake.as_ref(), Some(&saved), TARGET).await,
        TailscaleState::Removed {
            address: format!("https://{DNS}")
        }
    );
    fake.entries.lock().unwrap().push((443, TARGET.to_owned()));
    assert_eq!(
        tailscale::read_state(fake.as_ref(), Some(&saved), TARGET).await,
        TailscaleState::On {
            address: format!("https://{DNS}"),
            port_in_use: false,
        }
    );
    // The hub moved to another port, so the entry forwards to nothing.
    assert!(matches!(
        tailscale::read_state(fake.as_ref(), Some(&saved), "http://127.0.0.1:9000").await,
        TailscaleState::Removed { .. }
    ));
}

#[tokio::test]
async fn turning_on_again_removes_an_old_entry_of_this_hub_first() {
    let fake = FakeTailscale::running();
    fake.entries.lock().unwrap().push((443, TARGET.to_owned()));
    let saved = row(443, TARGET);
    let new_target = "http://127.0.0.1:9000";
    let on = tailscale::turn_on(fake.as_ref(), Some(&saved), new_target)
        .await
        .unwrap();
    assert_eq!(on.port, 443);
    assert_eq!(
        *fake.entries.lock().unwrap(),
        vec![(443, new_target.to_owned())]
    );
}

#[test]
fn a_failed_serve_gives_a_state_with_no_exact_text_needed() {
    let output = |stderr: &str, timed_out: bool| ProgramOutput {
        success: false,
        timed_out,
        stdout: String::new(),
        stderr: stderr.to_owned(),
    };
    // HTTPS off: the link is the action and the output stays for details.
    let state = classify_failure(
        &output(
            "Serve is not enabled on your tailnet.\nTo enable, visit:\n\n  https://login.tailscale.com/f/serve?node=abc.\n",
            true,
        ),
        true,
    );
    let TailscaleState::HttpsOff { action_url, detail } = state else {
        panic!("{state:?}")
    };
    assert_eq!(action_url, "https://login.tailscale.com/f/serve?node=abc");
    assert!(detail.unwrap().contains("not enabled"));

    // The Linux operator rule, on Linux only.
    let denied = output(
        "Access denied: serve config denied\nUse 'sudo tailscale set --operator=$USER' once",
        false,
    );
    assert!(matches!(
        classify_failure(&denied, true),
        TailscaleState::NeedsPermission { command, .. } if command == "sudo tailscale set --operator=$USER"
    ));
    assert!(matches!(
        classify_failure(&denied, false),
        TailscaleState::Failed { .. }
    ));

    // A text that nothing matches shows as it is.
    assert_eq!(
        classify_failure(&output("something new went wrong", false), true),
        TailscaleState::Failed {
            detail: "something new went wrong".to_owned()
        }
    );
    assert!(matches!(
        classify_failure(&output("", true), true),
        TailscaleState::Failed { detail } if detail.contains("did not answer")
    ));
}

#[test]
fn a_failed_change_shows_until_a_live_state_replaces_it() {
    let failure = TailscaleState::Failed {
        detail: "x".to_owned(),
    };
    let available = TailscaleState::Available {
        name: "m".to_owned(),
        address: "https://m".to_owned(),
        port_in_use: false,
    };
    assert_eq!(
        shown_state(available.clone(), Some(failure.clone())),
        failure
    );
    assert_eq!(shown_state(available.clone(), None), available);
    assert_eq!(
        shown_state(TailscaleState::NotConnected, Some(failure)),
        TailscaleState::NotConnected
    );
}

#[test]
fn the_network_host_and_the_serve_target_come_from_the_machine() {
    assert_eq!(
        local_host("MacBook-Pro.local").as_deref(),
        Some("macbook-pro.local")
    );
    assert_eq!(local_host("box.example.com").as_deref(), Some("box.local"));
    assert_eq!(local_host("My Mac").as_deref(), Some("mymac.local"));
    assert_eq!(local_host("---"), None);
    assert_eq!(local_host(""), None);
    assert_eq!(serve_target("127.0.0.1:7600".parse().unwrap()), TARGET);
    assert_eq!(serve_target("0.0.0.0:7600".parse().unwrap()), TARGET);
    assert_eq!(
        serve_target("[::1]:7600".parse().unwrap()),
        "http://[::1]:7600"
    );
}

#[test]
fn only_a_local_or_development_hub_may_change_its_reach() {
    use consort_proto::release::InstallKind::*;
    assert!(super::may_change(Local, false));
    assert!(super::may_change(Manual, true));
    for kind in [Service, Docker, Manual] {
        assert!(!super::may_change(kind, false), "{kind:?}");
    }
}

async fn test_hub(
    can_change: bool,
    fake: &Arc<FakeTailscale>,
) -> (crate::hub::Hub, tempfile::TempDir) {
    let (hub, dir) = crate::hub::test_hub().await;
    hub.reach.set_program(fake.clone());
    hub.start_reach("127.0.0.1:7600".parse().unwrap(), can_change)
        .await
        .unwrap();
    (hub, dir)
}

#[tokio::test]
async fn a_service_hub_answers_with_its_state_and_changes_nothing() {
    let fake = FakeTailscale::running();
    let (hub, _dir) = test_hub(false, &fake).await;
    let info = hub.set_reach(ReachMethod::Tailscale, true).await.unwrap();
    assert!(!info.can_change);
    assert!(matches!(info.tailscale, TailscaleState::Available { .. }));
    let info = hub.set_reach(ReachMethod::ThisNetwork, true).await.unwrap();
    assert_eq!(info.this_network, NetworkState::Off);
    assert!(fake.changes().is_empty());
    assert_eq!(hub.accepted_origins().await.len(), 1);
}

#[tokio::test]
async fn turning_tailscale_on_and_off_changes_the_origins_and_the_pairing_address() {
    let fake = FakeTailscale::running();
    let (hub, _dir) = test_hub(true, &fake).await;
    let public = hub.settings.public_url.clone();
    assert_eq!(hub.pairing_address().await.unwrap().hub_url, public);

    let info = hub.set_reach(ReachMethod::Tailscale, true).await.unwrap();
    assert!(
        matches!(info.tailscale, TailscaleState::On { .. }),
        "{info:?}"
    );
    assert_eq!(info.pairing.hub_url, format!("https://{DNS}"));
    assert_eq!(info.pairing.tls_sha256, None);
    let origins = hub.accepted_origins().await;
    assert!(origins.contains(&hub.settings.origin));
    assert!(origins.contains(&format!("https://{DNS}")));
    // The public URL does not change, so saved clients keep their address.
    assert_eq!(hub.settings.public_url, public);

    let info = hub.set_reach(ReachMethod::Tailscale, false).await.unwrap();
    assert!(matches!(info.tailscale, TailscaleState::Available { .. }));
    assert_eq!(
        hub.accepted_origins().await,
        vec![hub.settings.origin.clone()]
    );
    assert_eq!(info.pairing.hub_url, public);
}

#[tokio::test]
async fn a_tailscale_entry_that_went_away_keeps_its_origin_until_the_user_decides() {
    let fake = FakeTailscale::running();
    let (hub, _dir) = test_hub(true, &fake).await;
    hub.set_reach(ReachMethod::Tailscale, true).await.unwrap();
    fake.entries.lock().unwrap().clear();

    let info = hub.reach_info().await.unwrap();
    assert_eq!(
        info.tailscale,
        TailscaleState::Removed {
            address: format!("https://{DNS}")
        }
    );
    assert!(
        hub.accepted_origins()
            .await
            .contains(&format!("https://{DNS}"))
    );

    // Turn on again.
    let info = hub.set_reach(ReachMethod::Tailscale, true).await.unwrap();
    assert!(matches!(info.tailscale, TailscaleState::On { .. }));
    fake.entries.lock().unwrap().clear();
    // Or turn off, which forgets the address. There is no entry to remove.
    let info = hub.set_reach(ReachMethod::Tailscale, false).await.unwrap();
    assert!(matches!(info.tailscale, TailscaleState::Available { .. }));
    assert_eq!(hub.accepted_origins().await.len(), 1);
}

#[tokio::test]
async fn https_off_shows_the_link_and_the_hub_turns_on_when_https_is_there() {
    let fake = FakeTailscale::running();
    *fake.https.lock().unwrap() = false;
    *fake.serve_answer.lock().unwrap() = Some(ProgramOutput {
        timed_out: true,
        stdout: "To enable, visit: https://login.tailscale.com/f/serve?node=abc".to_owned(),
        ..ProgramOutput::default()
    });
    let (hub, _dir) = test_hub(true, &fake).await;
    let info = hub.set_reach(ReachMethod::Tailscale, true).await.unwrap();
    let TailscaleState::HttpsOff { action_url, detail } = info.tailscale else {
        panic!("{:?}", info.tailscale)
    };
    assert_eq!(action_url, "https://login.tailscale.com/f/serve?node=abc");
    assert!(detail.is_some());
    assert_eq!(hub.accepted_origins().await.len(), 1);

    // An admin turns on HTTPS. The next check needs no other step.
    *fake.https.lock().unwrap() = true;
    *fake.serve_answer.lock().unwrap() = None;
    let info = hub.reach_info().await.unwrap();
    assert!(
        matches!(info.tailscale, TailscaleState::On { .. }),
        "{info:?}"
    );
    assert_eq!(hub.accepted_origins().await.len(), 2);
}

#[tokio::test]
async fn a_failed_turn_on_shows_the_output_and_try_again_clears_it() {
    let fake = FakeTailscale::running();
    *fake.serve_answer.lock().unwrap() = Some(ProgramOutput {
        stderr: "Access denied: serve config denied".to_owned(),
        ..ProgramOutput::default()
    });
    let (hub, _dir) = test_hub(true, &fake).await;
    let info = hub.set_reach(ReachMethod::Tailscale, true).await.unwrap();
    assert!(matches!(
        info.tailscale,
        TailscaleState::NeedsPermission { .. } | TailscaleState::Failed { .. }
    ));
    // A status check does not run Serve again.
    let before = fake.changes().len();
    hub.reach_info().await.unwrap();
    assert_eq!(fake.changes().len(), before);

    *fake.serve_answer.lock().unwrap() = None;
    let info = hub.set_reach(ReachMethod::Tailscale, true).await.unwrap();
    assert!(matches!(info.tailscale, TailscaleState::On { .. }));
}

#[tokio::test]
async fn the_cli_changes_the_same_store_and_the_hub_reads_it_with_no_restart() {
    let fake = FakeTailscale::running();
    let (hub, dir) = test_hub(false, &fake).await;

    let lines = cli::tailscale_with(dir.path(), fake.as_ref())
        .await
        .unwrap();
    assert!(lines[0].contains(DNS), "{lines:?}");
    assert!(
        hub.accepted_origins()
            .await
            .contains(&format!("https://{DNS}"))
    );
    let lines = cli::status_with(dir.path(), fake.as_ref()).await.unwrap();
    assert!(lines[0].contains("Tailscale is on"), "{lines:?}");
    assert!(matches!(
        hub.reach_info().await.unwrap().tailscale,
        TailscaleState::On { .. }
    ));

    let lines = cli::off_with(dir.path(), None, fake.as_ref())
        .await
        .unwrap();
    assert!(lines[0].contains("no longer works"), "{lines:?}");
    assert_eq!(hub.accepted_origins().await.len(), 1);
    assert!(fake.entries.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_cli_needs_a_hub_that_started_and_says_what_stopped_it() {
    let fake = FakeTailscale::running();
    let dir = tempfile::tempdir().unwrap();
    let err = cli::tailscale_with(dir.path(), fake.as_ref())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Start it"), "{err}");

    let conn = crate::store::open_and_migrate(dir.path()).unwrap();
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)",
        [TARGET_KEY, TARGET],
    )
    .unwrap();
    *fake.backend.lock().unwrap() = "Stopped".to_owned();
    let err = cli::tailscale_with(dir.path(), fake.as_ref())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not connected"), "{err}");
    assert!(reach_store::rows(&conn).unwrap().is_empty());
}
