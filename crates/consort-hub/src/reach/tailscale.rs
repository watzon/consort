//! What the hub reads from Tailscale and asks of it. Only the entry of this
//! hub ever changes. The hub never runs `tailscale serve reset`, and it never
//! turns on Funnel.

use std::collections::{BTreeMap, BTreeSet};

use consort_proto::client_api::TailscaleState;
use serde_json::Value;

use super::program::{ProgramError, ProgramOutput, TailscaleProgram};
use crate::store::reach::ReachRow;

/// The ports that Consort uses for Serve, in order.
const SERVE_PORTS: [u16; 2] = [443, 8443];
/// The page where an admin turns on HTTPS, when Tailscale gives no link.
const ADMIN_DNS_URL: &str = "https://login.tailscale.com/admin/dns";
/// The command that gives a Linux user the right to change Tailscale.
const OPERATOR_COMMAND: &str = "sudo tailscale set --operator=$USER";
/// The most characters of program output in an answer.
const DETAIL_LIMIT: usize = 4_000;

/// This machine on the tailnet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Tailnet {
    /// The first label of the DNS name.
    pub name: String,
    /// The DNS name, with no dot at the end.
    pub dns: String,
    /// The tailnet can give this machine a certificate.
    pub https: bool,
}

/// What `tailscale status --json` said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TailnetRead {
    NotInstalled,
    NotConnected,
    Failed(String),
    Ready(Tailnet),
}

fn shorten(text: &str) -> String {
    let text = text.trim();
    match text.char_indices().nth(DETAIL_LIMIT) {
        Some((end, _)) => format!("{}...", text.get(..end).unwrap_or(text)),
        None => text.to_owned(),
    }
}

fn run_failed(err: &ProgramError) -> String {
    err.to_string()
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|arg| (*arg).to_owned()).collect()
}

/// Reads the output of `tailscale status --json`.
pub(super) fn parse_status(stdout: &str) -> TailnetRead {
    let Ok(status) = serde_json::from_str::<Value>(stdout) else {
        return TailnetRead::Failed(shorten(stdout));
    };
    if status.get("BackendState").and_then(Value::as_str) != Some("Running") {
        return TailnetRead::NotConnected;
    }
    let me = status.get("Self");
    let dns = me
        .and_then(|me| me.get("DNSName"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_owned();
    let capability = me
        .and_then(|me| me.get("Capabilities"))
        .and_then(Value::as_array)
        .is_some_and(|caps| caps.iter().any(|cap| cap.as_str() == Some("https")))
        || me
            .and_then(|me| me.get("CapMap"))
            .and_then(Value::as_object)
            .is_some_and(|caps| caps.contains_key("https"));
    let magic_dns = status
        .get("CurrentTailnet")
        .and_then(|tailnet| tailnet.get("MagicDNSEnabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let name = dns.split('.').next().unwrap_or_default().to_owned();
    TailnetRead::Ready(Tailnet {
        name,
        https: capability && magic_dns && !dns.is_empty(),
        dns,
    })
}

async fn read_tailnet(program: &dyn TailscaleProgram) -> TailnetRead {
    match program.run(&args(&["status", "--json"])).await {
        Err(ProgramError::NotFound) => TailnetRead::NotInstalled,
        Err(err) => TailnetRead::Failed(run_failed(&err)),
        // A stopped Tailscale still answers with JSON, on some systems with a
        // failed status too.
        Ok(out) => match parse_status(&out.stdout) {
            TailnetRead::Failed(_) if !out.success => {
                if out.text().to_ascii_lowercase().contains("not running") {
                    TailnetRead::NotConnected
                } else {
                    TailnetRead::Failed(shorten(&out.text()))
                }
            }
            read => read,
        },
    }
}

/// What `tailscale serve status --json` said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ServeConfig {
    /// Every port that has an entry, of any kind.
    pub ports: BTreeSet<u16>,
    /// `(<host>, <port>)` to the URL that the root path forwards to.
    pub proxies: BTreeMap<(String, u16), String>,
}

impl ServeConfig {
    /// The port of the entry of this hub on this machine.
    fn own_port(&self, dns: &str, target: &str) -> Option<u16> {
        self.proxies
            .iter()
            .find(|((host, _), proxy)| host == dns && same_target(proxy, target))
            .map(|((_, port), _)| *port)
    }

    fn holds(&self, dns: &str, port: u16, target: &str) -> bool {
        self.proxies
            .get(&(dns.to_owned(), port))
            .is_some_and(|proxy| same_target(proxy, target))
    }

    fn first_free_port(&self) -> Option<u16> {
        SERVE_PORTS
            .into_iter()
            .find(|port| !self.ports.contains(port))
    }
}

fn same_target(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

fn add_config(config: &Value, into: &mut ServeConfig) {
    if let Some(tcp) = config.get("TCP").and_then(Value::as_object) {
        into.ports
            .extend(tcp.keys().filter_map(|port| port.parse::<u16>().ok()));
    }
    if let Some(web) = config.get("Web").and_then(Value::as_object) {
        for (host_port, entry) in web {
            let Some((host, port)) = host_port.rsplit_once(':') else {
                continue;
            };
            let Ok(port) = port.parse::<u16>() else {
                continue;
            };
            into.ports.insert(port);
            let proxy = entry
                .get("Handlers")
                .and_then(|handlers| handlers.get("/"))
                .and_then(|handler| handler.get("Proxy"))
                .and_then(Value::as_str);
            if let Some(proxy) = proxy {
                into.proxies
                    .insert((host.to_owned(), port), proxy.to_owned());
            }
        }
    }
    // A foreground `tailscale serve` holds its ports while it runs.
    if let Some(foreground) = config.get("Foreground").and_then(Value::as_object) {
        for session in foreground.values() {
            add_config(session, into);
        }
    }
}

/// Reads the output of `tailscale serve status --json`. Empty output means no
/// entry.
pub(super) fn parse_serve(stdout: &str) -> Result<ServeConfig, String> {
    let mut config = ServeConfig::default();
    if stdout.trim().is_empty() {
        return Ok(config);
    }
    let value: Value = serde_json::from_str(stdout).map_err(|_| shorten(stdout))?;
    add_config(&value, &mut config);
    Ok(config)
}

async fn read_serve(program: &dyn TailscaleProgram) -> Result<ServeConfig, String> {
    match program.run(&args(&["serve", "status", "--json"])).await {
        Err(err) => Err(run_failed(&err)),
        Ok(out) if !out.success => Err(shorten(&out.text())),
        Ok(out) => parse_serve(&out.stdout),
    }
}

/// The address on a Serve port.
pub(super) fn address(dns: &str, port: u16) -> String {
    if port == 443 {
        format!("https://{dns}")
    } else {
        format!("https://{dns}:{port}")
    }
}

/// The first `https://` word of a text, with no punctuation at its end.
fn first_https_url(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|word| word.starts_with("https://"))
        .map(|word| {
            word.trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '/')
                .to_owned()
        })
}

/// Turns a failed change into a state. The text of the program is not a stable
/// contract, so anything that does not match shows that text as it is.
pub(super) fn classify_failure(out: &ProgramOutput, linux: bool) -> TailscaleState {
    let text = out.text();
    let lower = text.to_ascii_lowercase();
    let denied = lower.contains("--operator")
        || lower.contains("access denied")
        || lower.contains("permission denied");
    if denied {
        return if linux {
            TailscaleState::NeedsPermission {
                command: OPERATOR_COMMAND.to_owned(),
                detail: shorten(&text),
            }
        } else {
            TailscaleState::Failed {
                detail: shorten(&text),
            }
        };
    }
    if let Some(action_url) = first_https_url(&text) {
        return TailscaleState::HttpsOff {
            action_url,
            detail: Some(shorten(&text)),
        };
    }
    TailscaleState::Failed {
        detail: if out.timed_out && text.is_empty() {
            "Tailscale did not answer in time.".to_owned()
        } else {
            shorten(&text)
        },
    }
}

/// The state with what the store says. `saved` is the row of Tailscale.
pub(super) async fn read_state(
    program: &dyn TailscaleProgram,
    saved: Option<&ReachRow>,
    target: &str,
) -> TailscaleState {
    let tailnet = match read_tailnet(program).await {
        TailnetRead::NotInstalled => {
            return match saved {
                Some(row) => TailscaleState::Removed {
                    address: row.address.clone(),
                },
                None => TailscaleState::NotInstalled,
            };
        }
        TailnetRead::NotConnected => return TailscaleState::NotConnected,
        TailnetRead::Failed(detail) => return TailscaleState::Failed { detail },
        TailnetRead::Ready(tailnet) => tailnet,
    };
    let serve = read_serve(program).await;
    if let Some(row) = saved {
        return match serve {
            // Without an answer the hub knows nothing new, and a hardened
            // service may not read the entries. Only a clear answer shows
            // that the entry is gone.
            Err(_) => TailscaleState::On {
                address: row.address.clone(),
                port_in_use: row.port != 443,
            },
            Ok(serve) => {
                let forwards_here = row.target.as_deref() == Some(target)
                    && serve.holds(&tailnet.dns, row.port, target);
                if forwards_here && row.address == address(&tailnet.dns, row.port) {
                    TailscaleState::On {
                        address: row.address.clone(),
                        port_in_use: row.port != 443,
                    }
                } else {
                    TailscaleState::Removed {
                        address: row.address.clone(),
                    }
                }
            }
        };
    }
    if !tailnet.https {
        return TailscaleState::HttpsOff {
            action_url: ADMIN_DNS_URL.to_owned(),
            detail: None,
        };
    }
    let serve = match serve {
        Ok(serve) => serve,
        Err(detail) => return TailscaleState::Failed { detail },
    };
    match serve
        .own_port(&tailnet.dns, target)
        .or_else(|| serve.first_free_port())
    {
        Some(port) => TailscaleState::Available {
            address: address(&tailnet.dns, port),
            name: tailnet.name,
            port_in_use: port != 443,
        },
        None => TailscaleState::NoFreePort { name: tailnet.name },
    }
}

/// A change to Tailscale that worked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TurnedOn {
    pub address: String,
    pub port: u16,
}

/// Adds the entry of this hub on the first free port, or finds the one that is
/// there. `previous` is an entry that an earlier run of this hub made for
/// another address of the hub. It goes first, so that it does not stay behind.
pub(super) async fn turn_on(
    program: &dyn TailscaleProgram,
    previous: Option<&ReachRow>,
    target: &str,
) -> Result<TurnedOn, TailscaleState> {
    let tailnet = match read_tailnet(program).await {
        TailnetRead::NotInstalled => return Err(TailscaleState::NotInstalled),
        TailnetRead::NotConnected => return Err(TailscaleState::NotConnected),
        TailnetRead::Failed(detail) => return Err(TailscaleState::Failed { detail }),
        TailnetRead::Ready(tailnet) => tailnet,
    };
    let mut serve = read_serve(program)
        .await
        .map_err(|detail| TailscaleState::Failed { detail })?;
    if let Some(port) = serve.own_port(&tailnet.dns, target) {
        return Ok(TurnedOn {
            address: address(&tailnet.dns, port),
            port,
        });
    }
    if let Some(row) = previous
        && let Some(old_target) = row.target.as_deref()
        && serve.holds(&tailnet.dns, row.port, old_target)
    {
        let off = off_args(row.port, old_target);
        let out = program
            .run(&off)
            .await
            .map_err(|err| TailscaleState::Failed {
                detail: run_failed(&err),
            })?;
        if !out.success {
            return Err(classify_failure(&out, cfg!(target_os = "linux")));
        }
        serve = read_serve(program)
            .await
            .map_err(|detail| TailscaleState::Failed { detail })?;
    }
    let Some(port) = serve.first_free_port() else {
        return Err(TailscaleState::NoFreePort { name: tailnet.name });
    };
    let on = vec![
        "serve".to_owned(),
        "--bg".to_owned(),
        "--yes".to_owned(),
        format!("--https={port}"),
        target.to_owned(),
    ];
    let out = program
        .run(&on)
        .await
        .map_err(|err| TailscaleState::Failed {
            detail: run_failed(&err),
        })?;
    if !out.success {
        return Err(classify_failure(&out, cfg!(target_os = "linux")));
    }
    // Believe the entries, not the exit status.
    let serve = read_serve(program)
        .await
        .map_err(|detail| TailscaleState::Failed { detail })?;
    if !serve.holds(&tailnet.dns, port, target) {
        return Err(TailscaleState::Failed {
            detail: shorten(&out.text()),
        });
    }
    Ok(TurnedOn {
        address: address(&tailnet.dns, port),
        port,
    })
}

fn off_args(port: u16, target: &str) -> Vec<String> {
    vec![
        "serve".to_owned(),
        "--yes".to_owned(),
        format!("--https={port}"),
        target.to_owned(),
        "off".to_owned(),
    ]
}

/// Removes the entry of this hub, and only when it still is the entry of this
/// hub. An entry that someone changed stays.
pub(super) async fn turn_off(
    program: &dyn TailscaleProgram,
    row: &ReachRow,
) -> Result<(), TailscaleState> {
    let Some(target) = row.target.as_deref() else {
        return Ok(());
    };
    let tailnet = match read_tailnet(program).await {
        // With no Tailscale there is no entry to remove.
        TailnetRead::NotInstalled | TailnetRead::NotConnected => return Ok(()),
        TailnetRead::Failed(detail) => return Err(TailscaleState::Failed { detail }),
        TailnetRead::Ready(tailnet) => tailnet,
    };
    let serve = read_serve(program)
        .await
        .map_err(|detail| TailscaleState::Failed { detail })?;
    if !serve.holds(&tailnet.dns, row.port, target) {
        return Ok(());
    }
    let out = program
        .run(&off_args(row.port, target))
        .await
        .map_err(|err| TailscaleState::Failed {
            detail: run_failed(&err),
        })?;
    if out.success {
        Ok(())
    } else {
        Err(classify_failure(&out, cfg!(target_os = "linux")))
    }
}
