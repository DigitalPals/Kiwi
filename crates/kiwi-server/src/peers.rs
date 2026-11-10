//! Who is on the other end of a loopback connection (architecture.md "Security"). On a
//! Mac (or with `--trust-local`), Kiwi trusts anything on loopback: the computer is its
//! owner's alone. Elsewhere other
//! users and services may share the computer, so only tailscaled (`tailscale serve`, which
//! runs as root) may forward a tailnet address with `X-Forwarded-For`, and nothing else on
//! the computer counts as the owner. The kernel's socket tables (`/proc/net/tcp` and
//! `/proc/net/tcp6`) name the user of each connection's client socket.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
};

use axum::{extract::connect_info::Connected, serve::IncomingStream};
use tokio::{net::TcpListener, sync::OnceCell};

use crate::{Kiwi, config::LocalTrust, devices::forwarded_client};

/// One TCP connection to Kiwi (axum's connect info for every request on it).
#[derive(Debug, Clone)]
pub struct Connection {
    /// The client's address.
    pub remote: SocketAddr,
    /// Kiwi's end of the connection.
    pub local: Option<SocketAddr>,
    /// The uid of the client's socket, looked up once per connection: it cannot change
    /// while the connection is open.
    uid: Arc<OnceCell<u32>>,
}

impl Connection {
    pub fn new(remote: SocketAddr, local: Option<SocketAddr>) -> Self {
        Self {
            remote,
            local,
            uid: Arc::default(),
        }
    }

    /// The uid of the process that made the connection, from `peers`.
    async fn uid(&self, peers: Arc<dyn PeerUids>) -> Result<u32, String> {
        let local = self
            .local
            .ok_or("Kiwi's end of the connection is unknown")?;
        let remote = self.remote;
        let uid = self
            .uid
            .get_or_try_init(|| async move {
                let found = tokio::task::spawn_blocking(move || peers.uid(remote, local))
                    .await
                    .map_err(|e| e.to_string())??;
                found.ok_or_else(|| {
                    format!("the kernel lists no open socket {remote} connected to {local}")
                })
            })
            .await?;
        Ok(*uid)
    }
}

impl Connected<IncomingStream<'_, TcpListener>> for Connection {
    fn connect_info(stream: IncomingStream<'_, TcpListener>) -> Self {
        Self::new(*stream.remote_addr(), stream.io().local_addr().ok())
    }
}

/// Finds the user of a loopback connection's client socket (injectable for tests).
pub trait PeerUids: Send + Sync + 'static {
    /// The uid of the open socket at `client` connected to `server` (Kiwi's end);
    /// `Ok(None)` when no such socket is listed.
    fn uid(&self, client: SocketAddr, server: SocketAddr) -> Result<Option<u32>, String>;
}

/// The kernel's socket tables of Kiwi's network namespace.
pub struct ProcNet;

/// `/proc/net/tcp` and `/proc/net/tcp6` (through `/proc/self`, which works with
/// systemd's `ProcSubset=pid` too).
const TABLES: [&str; 2] = ["/proc/self/net/tcp", "/proc/self/net/tcp6"];

impl PeerUids for ProcNet {
    fn uid(&self, client: SocketAddr, server: SocketAddr) -> Result<Option<u32>, String> {
        let mut uids = Vec::new();
        for path in TABLES {
            match std::fs::read_to_string(path) {
                Ok(text) => uids.extend(socket_uids(&text, client, server)),
                // No tcp6 without IPv6.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && path.ends_with('6') => {}
                Err(e) => return Err(format!("{path}: {e}")),
            }
        }
        one_uid(uids)
    }
}

/// The single uid of the matching sockets; an error when they disagree (which a TCP
/// connection's unique addresses rule out).
fn one_uid(uids: Vec<u32>) -> Result<Option<u32>, String> {
    match uids.split_first() {
        None => Ok(None),
        Some((uid, rest)) if rest.iter().all(|u| u == uid) => Ok(Some(*uid)),
        Some(_) => Err(format!(
            "conflicting sockets for one connection (uids {uids:?})"
        )),
    }
}

/// A row of `/proc/net/tcp` or `/proc/net/tcp6`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Socket {
    pub local: SocketAddr,
    pub remote: SocketAddr,
    /// `01` established, `06` time-wait, `0A` listening, ...
    pub state: u8,
    pub uid: u32,
    /// 0 for sockets no process holds any more (time-wait, closed but not yet finished);
    /// the kernel reports them as uid 0.
    pub inode: u64,
}

/// The rows of a socket table; the header and rows that don't parse are skipped.
pub fn parse_table(text: &str) -> impl Iterator<Item = Socket> + '_ {
    text.lines().filter_map(parse_row)
}

/// `  sl  local_address rem_address   st tx_queue:rx_queue tr:tm->when retrnsmt   uid  timeout inode ...`
fn parse_row(line: &str) -> Option<Socket> {
    let mut fields = line.split_whitespace();
    let slot = fields.next()?.strip_suffix(':')?;
    slot.parse::<u64>().ok()?;
    let local = parse_address(fields.next()?)?;
    let remote = parse_address(fields.next()?)?;
    let state = hex_field(fields.next()?).and_then(|s| u8::from_str_radix(s, 16).ok())?;
    // tx_queue:rx_queue, tr:tm->when, retrnsmt
    let mut fields = fields.skip(3);
    let uid = fields.next()?.parse().ok()?;
    let _timeout = fields.next()?;
    let inode = fields.next()?.parse().ok()?;
    Some(Socket {
        local,
        remote,
        state,
        uid,
        inode,
    })
}

fn hex_field(text: &str) -> Option<&str> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_hexdigit())).then_some(text)
}

/// `0100007F:1F96` (IPv4) or 32 hex digits and a port (IPv6). The kernel prints each 32-bit
/// word of the address as a number in the host's byte order, and the port in network order.
fn parse_address(text: &str) -> Option<SocketAddr> {
    let (ip, port) = text.split_once(':')?;
    let port = u16::from_str_radix(hex_field(port)?, 16).ok()?;
    let ip = hex_field(ip)?;
    let word = |i: usize| {
        u32::from_str_radix(&ip[i * 8..i * 8 + 8], 16)
            .ok()
            .map(u32::to_ne_bytes)
    };
    let ip = match ip.len() {
        8 => IpAddr::V4(Ipv4Addr::from(word(0)?)),
        32 => {
            let mut bytes = [0u8; 16];
            for i in 0..4 {
                bytes[i * 4..i * 4 + 4].copy_from_slice(&word(i)?);
            }
            IpAddr::V6(Ipv6Addr::from(bytes))
        }
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

/// IPv4-mapped IPv6 addresses (dual-stack sockets) as plain IPv4.
fn canonical(addr: SocketAddr) -> (IpAddr, u16) {
    (addr.ip().to_canonical(), addr.port())
}

/// The uids of the open sockets at `client` connected to `server` in a socket table.
/// Sockets no process holds (inode 0) are left out: the kernel reports them as root's.
pub fn socket_uids(text: &str, client: SocketAddr, server: SocketAddr) -> Vec<u32> {
    let (client, server) = (canonical(client), canonical(server));
    parse_table(text)
        .filter(|s| s.inode != 0 && canonical(s.local) == client && canonical(s.remote) == server)
        .map(|s| s.uid)
        .collect()
}

/// Where a request comes from, as far as Kiwi can tell (architecture.md "Device
/// identification").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Proxied by `tailscale serve` for this tailnet address.
    Tailnet(IpAddr),
    /// Made on the computer itself, which is its owner's alone ([`LocalTrust::Anyone`]).
    Local,
    /// Nothing tells who made it, so it is not the owner: a request on the computer that
    /// doesn't come through tailscaled ([`LocalTrust::ProxyUid`]).
    Unknown,
}

/// The origin of a request on `conn` with `forwarded_for` (`X-Forwarded-For`), under
/// [`LocalTrust`]. Without a way to tell who made a forwarded request, it is not trusted.
pub async fn origin(kiwi: &Kiwi, conn: Option<&Connection>, forwarded_for: Option<&str>) -> Origin {
    let forwarded = forwarded_client(conn.map(|c| c.remote), forwarded_for);
    match kiwi.config.local {
        LocalTrust::Anyone => forwarded.map_or(Origin::Local, Origin::Tailnet),
        LocalTrust::ProxyUid(proxy) => {
            let (Some(conn), Some(ip)) = (conn, forwarded) else {
                return Origin::Unknown;
            };
            match conn.uid(kiwi.peers.clone()).await {
                Ok(uid) if uid == proxy => Origin::Tailnet(ip),
                Ok(uid) => {
                    tracing::warn!(
                        "ignored X-Forwarded-For from {}: it comes from uid {uid}, and only \
                         tailscaled's (uid {proxy}, --proxy-uid) is trusted",
                        conn.remote
                    );
                    Origin::Unknown
                }
                Err(e) => {
                    tracing::warn!(
                        "ignored X-Forwarded-For from {}: could not tell which user made the \
                         connection: {e}",
                        conn.remote
                    );
                    Origin::Unknown
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `/proc/net/tcp` on a little-endian host: Kiwi listening on 127.0.0.1:8742,
    /// tailscaled (root) connected to it from port 51234, a local user (1000) from 51240,
    /// a connection in time-wait, an orphaned one, and the server side of the first.
    const TCP: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:2226 00000000:0000 0A 00000000:00000000 00:00000000 00000000   999        0 31001 1 0000000000000000 100 0 0 10 0
   1: 0100007F:C822 0100007F:2226 01 00000000:00000000 00:00000000 00000000     0        0 31002 1 0000000000000000 20 4 30 10 -1
   2: 0100007F:2226 0100007F:C822 01 00000000:00000000 00:00000000 00000000   999        0 31003 1 0000000000000000 20 4 30 10 -1
   3: 0100007F:C828 0100007F:2226 01 00000000:00000000 00:00000000 00000000  1000        0 31004 1 0000000000000000 20 4 30 10 -1
   4: 0100007F:C82A 0100007F:2226 06 00000000:00000000 03:00001770 00000000     0        0 0 3 0000000000000000
   5: 0100007F:C82C 0100007F:2226 05 00000000:00000000 00:00000000 00000000     0        0 0 2 0000000000000000 20 4 30 10 -1
   6: garbage
";

    /// `/proc/net/tcp6`: an IPv6 client of Kiwi on [::1]:8742, and an IPv4-mapped one
    /// (a dual-stack socket connected to 127.0.0.1).
    const TCP6: &str = "\
  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000001000000:2226 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000   999        0 41001 1 0000000000000000 100 0 0 10 0
   1: 00000000000000000000000001000000:D431 00000000000000000000000001000000:2226 01 00000000:00000000 00:00000000 00000000     0        0 41002 1 0000000000000000 20 4 30 10 -1
   2: 0000000000000000FFFF00000100007F:D432 0000000000000000FFFF00000100007F:2226 01 00000000:00000000 00:00000000 00000000   107        0 41003 1 0000000000000000 20 4 30 10 -1
";

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    #[test]
    #[cfg(target_endian = "little")]
    fn parses_socket_tables() {
        let rows: Vec<Socket> = parse_table(TCP).collect();
        assert_eq!(rows.len(), 6, "the header and the garbage line are skipped");
        assert_eq!(
            rows[0],
            Socket {
                local: addr("127.0.0.1:8742"),
                remote: addr("0.0.0.0:0"),
                state: 0x0A,
                uid: 999,
                inode: 31001,
            }
        );
        assert_eq!(
            (rows[1].local, rows[1].remote, rows[1].state, rows[1].uid),
            (addr("127.0.0.1:51234"), addr("127.0.0.1:8742"), 1, 0)
        );
        let rows: Vec<Socket> = parse_table(TCP6).collect();
        assert_eq!(rows[1].local, addr("[::1]:54321"));
        assert_eq!(rows[1].remote, addr("[::1]:8742"));
        assert_eq!(rows[2].local, addr("[::ffff:127.0.0.1]:54322"));
        assert_eq!(rows[2].uid, 107);
    }

    #[test]
    #[cfg(target_endian = "little")]
    fn finds_the_user_of_a_connection() {
        let kiwi = addr("127.0.0.1:8742");
        assert_eq!(socket_uids(TCP, addr("127.0.0.1:51234"), kiwi), [0]);
        assert_eq!(socket_uids(TCP, addr("127.0.0.1:51240"), kiwi), [1000]);
        // Each end of a connection is its own socket: Kiwi's is Kiwi's.
        assert_eq!(socket_uids(TCP, kiwi, addr("127.0.0.1:51234")), [999]);
        // Another server port, or a port nobody connected from.
        assert!(socket_uids(TCP, addr("127.0.0.1:51234"), addr("127.0.0.1:8743")).is_empty());
        assert!(socket_uids(TCP, addr("127.0.0.1:50000"), kiwi).is_empty());
        // Sockets no process holds any more show as root's: they never count.
        assert!(socket_uids(TCP, addr("127.0.0.1:51242"), kiwi).is_empty());
        assert!(socket_uids(TCP, addr("127.0.0.1:51244"), kiwi).is_empty());
        // IPv6, and IPv4 through a dual-stack socket on either side.
        assert_eq!(
            socket_uids(TCP6, addr("[::1]:54321"), addr("[::1]:8742")),
            [0]
        );
        assert_eq!(socket_uids(TCP6, addr("127.0.0.1:54322"), kiwi), [107]);
        assert_eq!(
            socket_uids(
                TCP,
                addr("[::ffff:127.0.0.1]:51234"),
                addr("[::ffff:127.0.0.1]:8742")
            ),
            [0]
        );
    }

    #[test]
    fn rejects_malformed_rows() {
        for row in [
            "",
            "  sl  local_address rem_address   st",
            "   0: 0100007F:2226",
            "   x: 0100007F:2226 0100007F:C822 01 0:0 0:0 0 0 0 1",
            "   0: 0100007G:2226 0100007F:C822 01 0:0 0:0 0 0 0 1",
            "   0: +100007F:2226 0100007F:C822 01 0:0 0:0 0 0 0 1",
            "   0: 0100007F:+226 0100007F:C822 01 0:0 0:0 0 0 0 1",
            "   0: 0100007F0:2226 0100007F:C822 01 0:0 0:0 0 0 0 1",
            "   0: 0100007F:12345 0100007F:C822 01 0:0 0:0 0 0 0 1",
            "   0: 0100007F:2226 0100007F:C822 01 0:0 0:0 0 -1 0 1",
            "   0: 0100007F:2226 0100007F:C822 01 0:0 0:0 0 0 0",
            "   0: 0100007F:2226 0100007F:C822 01 0:0 0:0 0 0 0 é",
            "   0: 00000000000000000000000001000000é:2226 0100007F:C822 01 0:0 0:0 0 0 0 1",
        ] {
            assert_eq!(parse_row(row), None, "{row:?}");
        }
        assert!(parse_row("   0: 0100007F:2226 0100007F:C822 01 0:0 0:0 0 0 0 1").is_some());
    }

    #[test]
    fn one_uid_per_connection() {
        assert_eq!(one_uid(vec![]), Ok(None));
        assert_eq!(one_uid(vec![0, 0]), Ok(Some(0)));
        assert!(one_uid(vec![0, 1000]).is_err());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn finds_this_process_on_a_real_connection() {
        let uid = unsafe { libc::geteuid() };
        // Kiwi's address, and the address the client connects to.
        let mut cases = vec![("127.0.0.1:0", "127.0.0.1")];
        if std::net::TcpListener::bind("[::1]:0").is_ok() {
            // IPv6, and IPv4 on a dual-stack socket (Kiwi sees IPv4-mapped addresses).
            cases.extend([("[::1]:0", "::1"), ("[::]:0", "127.0.0.1")]);
        }
        for (listen, connect) in cases {
            let listener = std::net::TcpListener::bind(listen).unwrap();
            let port = listener.local_addr().unwrap().port();
            let client =
                std::net::TcpStream::connect((connect.parse::<IpAddr>().unwrap(), port)).unwrap();
            let (accepted, remote) = listener.accept().unwrap();
            let server = accepted.local_addr().unwrap();
            assert_eq!(canonical(remote), canonical(client.local_addr().unwrap()));
            assert_eq!(ProcNet.uid(remote, server), Ok(Some(uid)), "{listen}");
            assert_eq!(ProcNet.uid(server, remote), Ok(Some(uid)), "{listen}");
            let other = SocketAddr::new(remote.ip(), 9);
            assert_eq!(ProcNet.uid(other, server), Ok(None), "{listen}");
        }
    }
}
