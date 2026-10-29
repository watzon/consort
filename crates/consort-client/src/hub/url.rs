//! The hub URL: what the user types, and the address the client keeps.

use std::net::{Ipv4Addr, Ipv6Addr};

use consort_proto::auth::CLIENT_WS_PATH;
use url::{Host, Url};

use crate::ClientError;

/// Plain `http` is for places where an eavesdropper is unlikely: this
/// machine, a private network, the Tailscale and carrier-grade NAT range,
/// and `.local` names. Every other host needs `https`.
fn plain_http_allowed(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost") || name.ends_with(".local")
        }
        Host::Ipv4(ip) => ipv4_is_private(*ip),
        Host::Ipv6(ip) => ipv6_is_private(*ip),
    }
}

fn ipv4_is_private(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        // 100.64.0.0/10: carrier-grade NAT, and the Tailscale range.
        || (a == 100 && (b & 0b1100_0000) == 0b0100_0000)
}

fn ipv6_is_private(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return ipv4_is_private(v4);
    }
    let first = ip.segments()[0];
    ip.is_loopback()
        // fc00::/7: unique local addresses.
        || (first & 0xfe00) == 0xfc00
        // fe80::/10: link-local addresses.
        || (first & 0xffc0) == 0xfe80
}

/// The hub URL as the user typed it, reduced to its origin: the scheme, the
/// host and a port that is not the default. A path, a query, a fragment and
/// a `user:password@` part are dropped, so a pasted secret is never stored.
///
/// A missing scheme is `http` where `http` is allowed (see
/// [`plain_http_allowed`]) and `https` anywhere else. An explicit `http://`
/// for any other host is an error that says to use `https`.
pub fn normalize_hub_url(input: &str) -> Result<String, ClientError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ClientError::InvalidUrl("it is empty".into()));
    }
    let explicit = trimmed.contains("://");
    let mut url = Url::parse(&if explicit {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    })
    .map_err(|e| ClientError::InvalidUrl(e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ClientError::InvalidUrl(format!(
            "{} is not an http or https address",
            url.scheme()
        )));
    }
    let Some(host) = url.host() else {
        return Err(ClientError::InvalidUrl(
            "it has no host name or address".into(),
        ));
    };
    let allowed = plain_http_allowed(&host);
    let host = host.to_string();
    if !explicit && allowed {
        url.set_scheme("http")
            .map_err(|()| ClientError::InvalidUrl(trimmed.to_owned()))?;
    }
    if url.scheme() == "http" && !allowed {
        return Err(ClientError::InvalidUrl(format!(
            "http is only for this machine, a private network, Tailscale or a .local name, and {host} is none of these. Use https://{host}"
        )));
    }
    Ok(url.origin().ascii_serialization())
}

/// The WebSocket address of the client link for a hub URL.
pub(super) fn ws_url(hub_url: &str) -> Result<String, ClientError> {
    let mut url = Url::parse(hub_url).map_err(|e| ClientError::InvalidUrl(e.to_string()))?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme)
        .map_err(|()| ClientError::InvalidUrl(hub_url.to_owned()))?;
    url.set_path(CLIENT_WS_PATH);
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(input: &str) -> String {
        normalize_hub_url(input).unwrap_or_else(|e| panic!("{input}: {e}"))
    }

    #[test]
    fn hub_urls_get_a_scheme() {
        assert_eq!(ok("127.0.0.1:7600"), "http://127.0.0.1:7600");
        assert_eq!(ok("localhost:7600"), "http://localhost:7600");
        assert_eq!(ok("hub.example.com/"), "https://hub.example.com");
        assert_eq!(ok("https://hub.example.com"), "https://hub.example.com");
        assert!(normalize_hub_url("").is_err());
        assert!(normalize_hub_url("   ").is_err());
        assert!(normalize_hub_url("ftp://x").is_err());
    }

    #[test]
    fn http_is_for_this_machine_a_private_network_tailscale_and_local_names() {
        for allowed in [
            "http://localhost:7600",
            "http://127.0.0.1:7600",
            "http://127.4.5.6",
            "http://[::1]:7600",
            "http://10.1.2.3:7600",
            "http://172.16.0.1",
            "http://172.31.255.254",
            "http://192.168.188.138:7600",
            "http://169.254.10.20",
            "http://100.64.0.1",
            "http://100.101.102.103:7600",
            "http://100.127.255.254",
            "http://[fd12:3456:789a::1]:7600",
            "http://[fc00::1]",
            "http://[fe80::1]",
            "http://[::ffff:192.168.1.5]",
            "http://hub.local:7600",
            "http://Mac-Studio.LOCAL",
            "http://hub.local.:7600",
        ] {
            assert!(normalize_hub_url(allowed).is_ok(), "{allowed} must work");
        }
    }

    #[test]
    fn http_to_any_other_host_needs_https() {
        for refused in [
            "http://hub.example.com",
            "http://8.8.8.8",
            "http://172.32.0.1",
            "http://172.15.0.1",
            "http://100.63.255.255",
            "http://100.128.0.1",
            "http://11.0.0.1",
            "http://[2001:db8::1]",
            "http://[fe00::1]",
            "http://localhost.example.com",
            "http://notlocal",
            "http://hub.local.example.com",
        ] {
            let err = normalize_hub_url(refused).unwrap_err().to_string();
            assert!(err.contains("https://"), "{refused}: {err}");
        }
    }

    #[test]
    fn https_works_for_every_host() {
        assert_eq!(ok("https://8.8.8.8:8443"), "https://8.8.8.8:8443");
        assert_eq!(ok("https://[2001:db8::1]"), "https://[2001:db8::1]");
        assert_eq!(ok("https://localhost"), "https://localhost");
    }

    #[test]
    fn a_bracketed_ipv6_address_gets_the_http_scheme_when_it_is_private() {
        assert_eq!(ok("[::1]:7600"), "http://[::1]:7600");
        assert_eq!(ok("[fd00::2]:7600"), "http://[fd00::2]:7600");
        assert_eq!(ok("[2001:db8::1]:7600"), "https://[2001:db8::1]:7600");
    }

    #[test]
    fn only_the_origin_is_kept() {
        assert_eq!(
            ok("https://user:secret@hub.example.com/x?y=1#frag"),
            "https://hub.example.com"
        );
        assert_eq!(
            ok("http://me:pw@192.168.1.5:7600/v1/client?token=abc"),
            "http://192.168.1.5:7600"
        );
        assert_eq!(
            ok("HTTPS://Hub.Example.COM:443/"),
            "https://hub.example.com"
        );
        assert_eq!(ok("http://127.0.0.1:80"), "http://127.0.0.1");
        let kept = ok("user:pw@hub.example.com/path?q=1");
        assert!(!kept.contains("pw") && !kept.contains("path") && !kept.contains('?'));
    }

    #[test]
    fn websocket_urls_follow_the_hub_scheme() {
        assert_eq!(
            ws_url("http://127.0.0.1:7600").unwrap(),
            "ws://127.0.0.1:7600/v1/client"
        );
        assert_eq!(
            ws_url("https://hub.example.com").unwrap(),
            "wss://hub.example.com/v1/client"
        );
    }
}
