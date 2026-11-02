//! A local SOCKS5 proxy that reaches only public addresses.
//!
//! The preview browser sends all of its traffic here
//! (`--proxy-server=socks5://127.0.0.1:<port>`). The proxy resolves each
//! name itself, checks every address, and refuses a target when one of its
//! addresses is not public: loopback, private, link-local, CGNAT,
//! multicast, broadcast, unspecified, documentation, an address of this
//! machine, or an IPv6 form that carries one of those. So a page cannot
//! reach the hub machine or its network, and a name that resolves to a
//! private address (DNS rebinding) does not help.
//!
//! The proxy speaks RFC 1928 with no authentication and only CONNECT. A
//! refusal gets the reply 0x02 ("connection not allowed by ruleset").

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};

/// The SOCKS version.
const SOCKS5: u8 = 5;
/// The method "no authentication required".
const NO_AUTH: u8 = 0x00;
/// The method reply "no acceptable methods".
const NO_METHOD: u8 = 0xFF;
/// The command CONNECT.
const CONNECT: u8 = 0x01;

/// The reply "succeeded".
const REPLY_OK: u8 = 0x00;
/// The reply "connection not allowed by ruleset".
pub(crate) const REPLY_REFUSED: u8 = 0x02;
/// The reply "host unreachable".
const REPLY_UNREACHABLE: u8 = 0x04;
/// The reply "connection refused".
const REPLY_CONNECTION_REFUSED: u8 = 0x05;
/// The reply "command not supported".
const REPLY_BAD_COMMAND: u8 = 0x07;
/// The reply "address type not supported".
const REPLY_BAD_ADDRESS: u8 = 0x08;

/// How long a client has to send its greeting and its request.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one connection attempt to a target may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Decides whether the proxy may connect to an address.
type Policy = fn(IpAddr) -> bool;

/// A running proxy. It stops, with all of its connections, when this value
/// drops.
#[derive(Debug)]
pub(crate) struct Proxy {
    port: u16,
    task: JoinHandle<()>,
}

impl Proxy {
    /// Starts a proxy on `127.0.0.1` at a free port. It connects only to
    /// public addresses.
    pub(crate) async fn start() -> std::io::Result<Self> {
        Self::start_with(allowed).await
    }

    /// Starts a proxy with another policy. Tests use it to reach a local
    /// server through the proxy.
    async fn start_with(policy: Policy) -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        let task = tokio::spawn(accept_loop(listener, policy));
        Ok(Self { port, task })
    }

    /// The port on `127.0.0.1`.
    pub(crate) fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        // The connection tasks live in a `JoinSet` inside this task, so they
        // stop with it.
        self.task.abort();
    }
}

async fn accept_loop(listener: TcpListener, policy: Policy) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((client, _)) => {
                    connections.spawn(async move {
                        if let Err(err) = serve(client, policy).await {
                            tracing::trace!(error = %err, "preview proxy connection ended");
                        }
                    });
                }
                Err(err) => {
                    tracing::debug!(error = %err, "preview proxy could not accept a connection");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            },
            // Reap finished connections, so the set does not grow.
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

/// The target of a CONNECT request.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Ip(IpAddr),
    Name(String),
}

async fn serve(mut client: TcpStream, policy: Policy) -> std::io::Result<()> {
    let request = tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(&mut client))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "handshake timed out"))??;
    let Some((target, port)) = request else {
        return Ok(());
    };
    if port == 0 {
        return reply(&mut client, REPLY_REFUSED).await;
    }
    let addresses = match resolve(&target, port).await {
        Ok(addresses) if !addresses.is_empty() => addresses,
        _ => return reply(&mut client, REPLY_UNREACHABLE).await,
    };
    // Refuse when one address is not allowed, so a name with a public and a
    // private address cannot reach the private one later.
    if addresses.iter().any(|a| !policy(a.ip())) {
        return reply(&mut client, REPLY_REFUSED).await;
    }
    let mut upstream = None;
    for address in &addresses {
        if let Ok(Ok(stream)) =
            tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address)).await
        {
            upstream = Some(stream);
            break;
        }
    }
    let Some(mut upstream) = upstream else {
        return reply(&mut client, REPLY_CONNECTION_REFUSED).await;
    };
    reply(&mut client, REPLY_OK).await?;
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

/// Reads the greeting and the request. Returns `None` when the proxy has
/// already answered with a refusal or closed the connection.
async fn handshake(client: &mut TcpStream) -> std::io::Result<Option<(Target, u16)>> {
    let mut head = [0u8; 2];
    client.read_exact(&mut head).await?;
    let mut methods = vec![0u8; usize::from(head[1])];
    client.read_exact(&mut methods).await?;
    if head[0] != SOCKS5 || !methods.contains(&NO_AUTH) {
        client.write_all(&[SOCKS5, NO_METHOD]).await?;
        return Ok(None);
    }
    client.write_all(&[SOCKS5, NO_AUTH]).await?;

    let mut request = [0u8; 4];
    client.read_exact(&mut request).await?;
    let [version, command, _reserved, kind] = request;
    if version != SOCKS5 {
        return Ok(None);
    }
    let target = match kind {
        0x01 => {
            let mut octets = [0u8; 4];
            client.read_exact(&mut octets).await?;
            Target::Ip(IpAddr::V4(Ipv4Addr::from(octets)))
        }
        0x03 => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).await?;
            let mut name = vec![0u8; usize::from(len[0])];
            client.read_exact(&mut name).await?;
            match String::from_utf8(name) {
                Ok(name) => Target::Name(name),
                Err(_) => {
                    reply(client, REPLY_BAD_ADDRESS).await?;
                    return Ok(None);
                }
            }
        }
        0x04 => {
            let mut octets = [0u8; 16];
            client.read_exact(&mut octets).await?;
            Target::Ip(IpAddr::V6(Ipv6Addr::from(octets)))
        }
        _ => {
            reply(client, REPLY_BAD_ADDRESS).await?;
            return Ok(None);
        }
    };
    let mut port = [0u8; 2];
    client.read_exact(&mut port).await?;
    if command != CONNECT {
        reply(client, REPLY_BAD_COMMAND).await?;
        return Ok(None);
    }
    Ok(Some((target, u16::from_be_bytes(port))))
}

async fn resolve(target: &Target, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    match target {
        Target::Ip(ip) => Ok(vec![SocketAddr::new(*ip, port)]),
        Target::Name(name) => {
            // A name in brackets or an address in text form resolves to
            // itself, and the policy checks it like any other address.
            let name = name.trim_start_matches('[').trim_end_matches(']');
            Ok(tokio::net::lookup_host((name, port)).await?.collect())
        }
    }
}

async fn reply(client: &mut TcpStream, code: u8) -> std::io::Result<()> {
    client
        .write_all(&[SOCKS5, code, 0, 0x01, 0, 0, 0, 0, 0, 0])
        .await?;
    client.flush().await
}

/// The policy of the proxy: a public address that this machine does not
/// hold.
fn allowed(ip: IpAddr) -> bool {
    is_public(ip) && !is_own_address(ip)
}

/// Whether this machine holds the address. A socket can bind only to an
/// address of one of its own interfaces, so a bind that works proves it.
/// This finds a public address of the hub machine, where a service may
/// listen on all interfaces.
fn is_own_address(ip: IpAddr) -> bool {
    std::net::UdpSocket::bind(SocketAddr::new(ip, 0)).is_ok()
}

/// Whether an address is public: not loopback, private, link-local, CGNAT,
/// multicast, broadcast, unspecified, documentation or another special
/// block, and not an IPv6 form that carries such an IPv4 address.
pub(crate) fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

/// The IPv4 blocks that are not public.
const V4_BLOCKED: [(u32, u8); 15] = [
    (0x0000_0000, 8),  // 0.0.0.0/8: this network, unspecified
    (0x0A00_0000, 8),  // 10.0.0.0/8: private
    (0x6440_0000, 10), // 100.64.0.0/10: CGNAT
    (0x7F00_0000, 8),  // 127.0.0.0/8: loopback
    (0xA9FE_0000, 16), // 169.254.0.0/16: link-local
    (0xAC10_0000, 12), // 172.16.0.0/12: private
    (0xC000_0000, 24), // 192.0.0.0/24: protocol assignments
    (0xC000_0200, 24), // 192.0.2.0/24: documentation
    (0xC058_6300, 24), // 192.88.99.0/24: 6to4 relay
    (0xC0A8_0000, 16), // 192.168.0.0/16: private
    (0xC612_0000, 15), // 198.18.0.0/15: benchmarking
    (0xC633_6400, 24), // 198.51.100.0/24: documentation
    (0xCB00_7100, 24), // 203.0.113.0/24: documentation
    (0xE000_0000, 4),  // 224.0.0.0/4: multicast
    (0xF000_0000, 4),  // 240.0.0.0/4: reserved and broadcast
];

/// The IPv6 blocks that are not public. The blocks that carry an IPv4
/// address (mapped, NAT64, 6to4) are checked by that address instead.
const V6_BLOCKED: [(u128, u8); 11] = [
    (0, 96), // ::/96: unspecified, loopback, IPv4-compatible
    (0x0064_ff9b_0001_0000_0000_0000_0000_0000, 48), // 64:ff9b:1::/48: local NAT64
    (0x0100_0000_0000_0000_0000_0000_0000_0000, 64), // 100::/64: discard
    (0x2001_0000_0000_0000_0000_0000_0000_0000, 23), // 2001::/23: protocol assignments, Teredo
    (0x2001_0db8_0000_0000_0000_0000_0000_0000, 32), // 2001:db8::/32: documentation
    (0x3fff_0000_0000_0000_0000_0000_0000_0000, 20), // 3fff::/20: documentation
    (0x5f00_0000_0000_0000_0000_0000_0000_0000, 16), // 5f00::/16: segment routing
    (0xfc00_0000_0000_0000_0000_0000_0000_0000, 7), // fc00::/7: unique local
    (0xfe80_0000_0000_0000_0000_0000_0000_0000, 10), // fe80::/10: link-local
    (0xfec0_0000_0000_0000_0000_0000_0000_0000, 10), // fec0::/10: site-local
    (0xff00_0000_0000_0000_0000_0000_0000_0000, 8), // ff00::/8: multicast
];

/// Whether an IPv4 address is in a block.
fn in_v4(value: u32, network: u32, prefix: u8) -> bool {
    let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
    value & mask == network & mask
}

/// Whether an IPv6 address is in a block.
fn in_v6(value: u128, network: u128, prefix: u8) -> bool {
    let mask = u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0);
    value & mask == network & mask
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let value = u32::from(ip);
    !V4_BLOCKED
        .iter()
        .any(|&(network, prefix)| in_v4(value, network, prefix))
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = embedded_v4(ip) {
        return is_public_v4(v4);
    }
    let value = u128::from(ip);
    !V6_BLOCKED
        .iter()
        .any(|&(network, prefix)| in_v6(value, network, prefix))
}

/// The IPv4 address in an IPv4-mapped (`::ffff:a.b.c.d`), NAT64
/// (`64:ff9b::a.b.c.d`) or 6to4 (`2002:aabb:ccdd::`) address. A translator
/// sends traffic for these to the IPv4 address, so the proxy checks that
/// address.
fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return Some(v4);
    }
    let value = u128::from(ip);
    if in_v6(value, 0x0064_ff9b_0000_0000_0000_0000_0000_0000, 96) {
        // The low 32 bits; the cast keeps exactly those.
        return Some(Ipv4Addr::from((value & 0xFFFF_FFFF) as u32));
    }
    if in_v6(value, 0x2002_0000_0000_0000_0000_0000_0000_0000, 16) {
        // Bits 16 to 47 from the top hold the IPv4 address.
        return Some(Ipv4Addr::from(((value >> 80) & 0xFFFF_FFFF) as u32));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> IpAddr {
        IpAddr::V4(s.parse().unwrap())
    }

    fn v6(s: &str) -> IpAddr {
        IpAddr::V6(s.parse().unwrap())
    }

    #[test]
    fn visual_proxy_refuses_special_ipv4_blocks() {
        for address in [
            "0.0.0.0",
            "0.1.2.3",
            "10.0.0.1",
            "10.255.255.255",
            "100.64.0.1",
            "100.127.255.254",
            "127.0.0.1",
            "127.1.2.3",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.8",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.7",
            "203.0.113.9",
            "224.0.0.1",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
        ] {
            assert!(!is_public(v4(address)), "{address} must not be public");
        }
    }

    #[test]
    fn visual_proxy_allows_public_ipv4() {
        for address in [
            "1.1.1.1",
            "8.8.8.8",
            "9.255.255.255",
            "11.0.0.1",
            "100.63.255.255",
            "100.128.0.1",
            "126.255.255.255",
            "128.0.0.1",
            "169.253.255.255",
            "172.15.255.255",
            "172.32.0.1",
            "192.0.1.1",
            "192.167.255.255",
            "192.169.0.1",
            "198.17.255.255",
            "198.20.0.1",
            "223.255.255.255",
        ] {
            assert!(is_public(v4(address)), "{address} must be public");
        }
    }

    #[test]
    fn visual_proxy_refuses_special_ipv6_blocks() {
        for address in [
            "::",
            "::1",
            "::7f00:1",
            "::a00:1",
            "64:ff9b:1::1",
            "100::1",
            "2001::1",
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "2001:1ff::1",
            "2001:db8::1",
            "3fff::1",
            "5f00::1",
            "fc00::1",
            "fd12:3456:789a::1",
            "fe80::1",
            "febf::1",
            "fec0::1",
            "ff02::1",
            "ff05::1:3",
        ] {
            assert!(!is_public(v6(address)), "{address} must not be public");
        }
    }

    #[test]
    fn visual_proxy_allows_public_ipv6() {
        for address in [
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
            "2a00:1450:4001::1",
            "2001:200::1",
            "2600::1",
        ] {
            assert!(is_public(v6(address)), "{address} must be public");
        }
    }

    #[test]
    fn visual_proxy_checks_the_ipv4_inside_ipv6_forms() {
        // IPv4-mapped.
        assert!(!is_public(v6("::ffff:127.0.0.1")));
        assert!(!is_public(v6("::ffff:10.1.2.3")));
        assert!(!is_public(v6("::ffff:169.254.169.254")));
        assert!(!is_public(v6("::ffff:100.64.0.1")));
        assert!(is_public(v6("::ffff:1.1.1.1")));
        // NAT64.
        assert!(!is_public(v6("64:ff9b::7f00:1")));
        assert!(!is_public(v6("64:ff9b::a00:1")));
        assert!(!is_public(v6("64:ff9b::c0a8:101")));
        assert!(is_public(v6("64:ff9b::808:808")));
        // 6to4.
        assert!(!is_public(v6("2002:7f00:1::1")));
        assert!(!is_public(v6("2002:c0a8:101::1")));
        assert!(is_public(v6("2002:808:808::1")));
    }

    #[test]
    fn visual_proxy_knows_its_own_addresses() {
        assert!(is_own_address(v4("127.0.0.1")));
        // A documentation address is on no interface.
        assert!(!is_own_address(v4("192.0.2.1")));
        assert!(!allowed(v4("127.0.0.1")));
    }

    /// Sends a greeting and a CONNECT request, and returns the reply code.
    async fn connect_through(port: u16, target: &[u8]) -> (u8, TcpStream) {
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        socket.write_all(&[5, 1, 0]).await.unwrap();
        let mut method = [0u8; 2];
        socket.read_exact(&mut method).await.unwrap();
        assert_eq!(method, [5, 0]);
        let mut request = vec![5, 1, 0];
        request.extend_from_slice(target);
        socket.write_all(&request).await.unwrap();
        let mut reply = [0u8; 10];
        socket.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[0], 5);
        (reply[1], socket)
    }

    fn ipv4_target(ip: Ipv4Addr, port: u16) -> Vec<u8> {
        let mut target = vec![1];
        target.extend_from_slice(&ip.octets());
        target.extend_from_slice(&port.to_be_bytes());
        target
    }

    fn ipv6_target(ip: Ipv6Addr, port: u16) -> Vec<u8> {
        let mut target = vec![4];
        target.extend_from_slice(&ip.octets());
        target.extend_from_slice(&port.to_be_bytes());
        target
    }

    fn name_target(name: &str, port: u16) -> Vec<u8> {
        let mut target = vec![3, u8::try_from(name.len()).unwrap()];
        target.extend_from_slice(name.as_bytes());
        target.extend_from_slice(&port.to_be_bytes());
        target
    }

    #[tokio::test]
    async fn visual_proxy_refuses_loopback_by_address_and_by_name() {
        // A real listener, so a refusal is the proxy's choice and not a
        // closed port.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let local = listener.local_addr().unwrap().port();
        let proxy = Proxy::start().await.unwrap();
        for target in [
            ipv4_target(Ipv4Addr::LOCALHOST, local),
            name_target("localhost", local),
            name_target("127.0.0.1", local),
            name_target("[::1]", local),
            ipv6_target(Ipv6Addr::LOCALHOST, local),
            ipv6_target("::ffff:127.0.0.1".parse().unwrap(), local),
            ipv4_target(Ipv4Addr::new(10, 0, 0, 1), 80),
            ipv4_target(Ipv4Addr::new(169, 254, 169, 254), 80),
        ] {
            let (code, _socket) = connect_through(proxy.port(), &target).await;
            assert_eq!(code, REPLY_REFUSED, "target {target:?}");
        }
        // The listener got no connection.
        let accepted = tokio::time::timeout(Duration::from_millis(100), listener.accept()).await;
        assert!(accepted.is_err(), "the proxy reached the local listener");
    }

    #[tokio::test]
    async fn visual_proxy_refuses_other_commands_and_port_zero() {
        let proxy = Proxy::start().await.unwrap();
        let (code, _socket) =
            connect_through(proxy.port(), &ipv4_target(Ipv4Addr::new(1, 1, 1, 1), 0)).await;
        assert_eq!(code, REPLY_REFUSED);

        // BIND (0x02) is not supported.
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, proxy.port()))
            .await
            .unwrap();
        socket.write_all(&[5, 1, 0]).await.unwrap();
        let mut method = [0u8; 2];
        socket.read_exact(&mut method).await.unwrap();
        let mut request = vec![5, 2, 0];
        request.extend_from_slice(&ipv4_target(Ipv4Addr::new(1, 1, 1, 1), 80));
        socket.write_all(&request).await.unwrap();
        let mut reply = [0u8; 10];
        socket.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[1], REPLY_BAD_COMMAND);
    }

    #[tokio::test]
    async fn visual_proxy_refuses_a_client_that_wants_authentication() {
        let proxy = Proxy::start().await.unwrap();
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, proxy.port()))
            .await
            .unwrap();
        // Only username and password (0x02).
        socket.write_all(&[5, 1, 2]).await.unwrap();
        let mut method = [0u8; 2];
        socket.read_exact(&mut method).await.unwrap();
        assert_eq!(method, [5, NO_METHOD]);
    }

    #[tokio::test]
    async fn visual_proxy_copies_both_ways_when_the_policy_allows() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let local = listener.local_addr().unwrap().port();
        let echo = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 5];
            stream.read_exact(&mut buf).await.unwrap();
            stream.write_all(&buf).await.unwrap();
        });
        let proxy = Proxy::start_with(|_| true).await.unwrap();
        let (code, mut socket) =
            connect_through(proxy.port(), &name_target("localhost", local)).await;
        assert_eq!(code, REPLY_OK);
        socket.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        socket.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
        echo.await.unwrap();
    }

    #[tokio::test]
    async fn visual_proxy_closes_its_connections_when_it_drops() {
        let proxy = Proxy::start().await.unwrap();
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, proxy.port()))
            .await
            .unwrap();
        socket.write_all(&[5, 1, 0]).await.unwrap();
        let mut method = [0u8; 2];
        socket.read_exact(&mut method).await.unwrap();
        drop(proxy);
        let mut rest = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut rest))
            .await
            .expect("the connection stayed open after the proxy stopped");
        assert!(read.is_err() || rest.is_empty());
    }
}
