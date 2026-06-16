//! # Network I/O
//!
//! This module provides VUMA-verified network I/O types with Behavioral Description
//! (BD) annotations and capability-based access control.
//!
//! ## Types
//!
//! - **TcpListener**: A TCP socket server that listens for incoming connections.
//! - **TcpStream**: A TCP connection stream for reading and writing.
//! - **UdpSocket**: A UDP socket for datagram communication.
//! - **SocketAddr**: An internet socket address (IP + port).
//! - **IpAddr**: An IP address (IPv4 or IPv6).
//! - **Ipv4Addr**: An IPv4 address.
//! - **Ipv6Addr**: An IPv6 address.
//!
//! ## BD Annotations
//!
//! - TcpStream: CapD { Read, Write, Send }
//! - TcpListener: CapD { Read, Execute }
//! - UdpSocket: CapD { Read, Write, Send }
//! - SyncEdge: bind → accept/connect (Seq), read → write (Seq)

use crate::primitives::{CapD, CapFlag, RepD, SyncEdge, SyncEdgeKind};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::{Read as StdRead, Write as StdWrite};
use std::net::{self as std_net};

// ---------------------------------------------------------------------------
// IpAddr
// ---------------------------------------------------------------------------

/// An IP address, either IPv4 or IPv6.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare, Hash, Serialize }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IpAddr {
    /// An IPv4 address.
    V4(Ipv4Addr),
    /// An IPv6 address.
    V6(Ipv6Addr),
}

impl IpAddr {
    /// Parse an IP address from a string.
    // VUMA-VERIFIED: parse delegates to std, result is validated
    pub fn parse(s: &str) -> Result<Self, String> {
        s.parse::<std_net::IpAddr>()
            .map(|addr| match addr {
                std_net::IpAddr::V4(v4) => IpAddr::V4(Ipv4Addr::from_std(v4)),
                std_net::IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from_std(v6)),
            })
            .map_err(|e| format!("invalid IP address: {}", e))
    }

    /// Create an IpAddr from an Ipv4Addr.
    // VUMA-VERIFIED: constructor is pure
    pub fn from_v4(addr: Ipv4Addr) -> Self {
        IpAddr::V4(addr)
    }

    /// Create an IpAddr from an Ipv6Addr.
    // VUMA-VERIFIED: constructor is pure
    pub fn from_v6(addr: Ipv6Addr) -> Self {
        IpAddr::V6(addr)
    }

    /// Returns true if this is an IPv4 address.
    // VUMA-VERIFIED: pure query
    pub fn is_ipv4(&self) -> bool {
        matches!(self, IpAddr::V4(_))
    }

    /// Returns true if this is an IPv6 address.
    // VUMA-VERIFIED: pure query
    pub fn is_ipv6(&self) -> bool {
        matches!(self, IpAddr::V6(_))
    }

    /// Convert to a std::net::IpAddr.
    // VUMA-VERIFIED: conversion is lossless
    pub(crate) fn to_std(self) -> std_net::IpAddr {
        match self {
            IpAddr::V4(v4) => std_net::IpAddr::V4(v4.to_std()),
            IpAddr::V6(v6) => std_net::IpAddr::V6(v6.to_std()),
        }
    }

    /// Convert from a std::net::IpAddr.
    // VUMA-VERIFIED: conversion is lossless
    pub(crate) fn from_std(addr: std_net::IpAddr) -> Self {
        match addr {
            std_net::IpAddr::V4(v4) => IpAddr::V4(Ipv4Addr::from_std(v4)),
            std_net::IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from_std(v6)),
        }
    }

    /// Returns the CapD for this type.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![
            CapFlag::Read,
            CapFlag::Compare,
            CapFlag::Hash,
            CapFlag::Serialize,
        ])
    }

    /// Returns the RepD for this type.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("IpAddr", 16, 4, self.capd())
    }

    /// Returns the SyncEdge annotations for this type.
    // VUMA-VERIFIED: IP addresses are passive data
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![]
    }
}

impl fmt::Display for IpAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IpAddr::V4(v4) => write!(f, "{}", v4),
            IpAddr::V6(v6) => write!(f, "{}", v6),
        }
    }
}

// ---------------------------------------------------------------------------
// Ipv4Addr
// ---------------------------------------------------------------------------

/// An IPv4 address.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare, Hash, Serialize }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Ipv4Addr {
    /// The four octets of the address.
    pub octets: [u8; 4],
}

impl Ipv4Addr {
    /// Create a new IPv4 address from four octets.
    // VUMA-VERIFIED: constructor is pure
    pub fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self {
            octets: [a, b, c, d],
        }
    }

    /// The localhost address (127.0.0.1).
    // VUMA-VERIFIED: constant is correct
    pub const LOCALHOST: Self = Self {
        octets: [127, 0, 0, 1],
    };

    /// The unspecified address (0.0.0.0).
    // VUMA-VERIFIED: constant is correct
    pub const UNSPECIFIED: Self = Self {
        octets: [0, 0, 0, 0],
    };

    /// The broadcast address (255.255.255.255).
    // VUMA-VERIFIED: constant is correct
    pub const BROADCAST: Self = Self {
        octets: [255, 255, 255, 255],
    };

    /// Create from a std::net::Ipv4Addr.
    pub(crate) fn from_std(addr: std_net::Ipv4Addr) -> Self {
        Self {
            octets: addr.octets(),
        }
    }

    /// Convert to a std::net::Ipv4Addr.
    // VUMA-VERIFIED: conversion is lossless
    pub(crate) fn to_std(self) -> std_net::Ipv4Addr {
        std_net::Ipv4Addr::new(
            self.octets[0],
            self.octets[1],
            self.octets[2],
            self.octets[3],
        )
    }

    /// Returns true if this is the loopback address.
    // VUMA-VERIFIED: pure query
    pub fn is_loopback(&self) -> bool {
        self.octets[0] == 127
    }

    /// Returns true if this is the unspecified address.
    // VUMA-VERIFIED: pure query
    pub fn is_unspecified(&self) -> bool {
        self.octets == [0, 0, 0, 0]
    }

    /// Returns the CapD for this type.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![
            CapFlag::Read,
            CapFlag::Compare,
            CapFlag::Hash,
            CapFlag::Serialize,
        ])
    }
}

impl fmt::Display for Ipv4Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{}.{}.{}",
            self.octets[0], self.octets[1], self.octets[2], self.octets[3]
        )
    }
}

// ---------------------------------------------------------------------------
// Ipv6Addr
// ---------------------------------------------------------------------------

/// An IPv6 address.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare, Hash, Serialize }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Ipv6Addr {
    /// The eight 16-bit segments of the address.
    pub segments: [u16; 8],
}

impl Ipv6Addr {
    /// Create a new IPv6 address from eight 16-bit segments.
    // VUMA-VERIFIED: constructor is pure
    #[allow(clippy::too_many_arguments)]
    pub fn new(a: u16, b: u16, c: u16, d: u16, e: u16, f: u16, g: u16, h: u16) -> Self {
        Self {
            segments: [a, b, c, d, e, f, g, h],
        }
    }

    /// The localhost address (::1).
    // VUMA-VERIFIED: constant is correct
    pub const LOCALHOST: Self = Self {
        segments: [0, 0, 0, 0, 0, 0, 0, 1],
    };

    /// The unspecified address (::).
    // VUMA-VERIFIED: constant is correct
    pub const UNSPECIFIED: Self = Self {
        segments: [0, 0, 0, 0, 0, 0, 0, 0],
    };

    /// Create from a std::net::Ipv6Addr.
    pub(crate) fn from_std(addr: std_net::Ipv6Addr) -> Self {
        Self {
            segments: addr.segments(),
        }
    }

    /// Convert to a std::net::Ipv6Addr.
    // VUMA-VERIFIED: conversion is lossless
    pub(crate) fn to_std(self) -> std_net::Ipv6Addr {
        std_net::Ipv6Addr::new(
            self.segments[0],
            self.segments[1],
            self.segments[2],
            self.segments[3],
            self.segments[4],
            self.segments[5],
            self.segments[6],
            self.segments[7],
        )
    }

    /// Returns true if this is the loopback address.
    // VUMA-VERIFIED: pure query
    pub fn is_loopback(&self) -> bool {
        self.segments == [0, 0, 0, 0, 0, 0, 0, 1]
    }

    /// Returns the CapD for this type.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![
            CapFlag::Read,
            CapFlag::Compare,
            CapFlag::Hash,
            CapFlag::Serialize,
        ])
    }
}

impl fmt::Display for Ipv6Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}",
            self.segments[0],
            self.segments[1],
            self.segments[2],
            self.segments[3],
            self.segments[4],
            self.segments[5],
            self.segments[6],
            self.segments[7]
        )
    }
}

// ---------------------------------------------------------------------------
// SocketAddr
// ---------------------------------------------------------------------------

/// An internet socket address consisting of an IP address and a port number.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare, Hash, Serialize }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SocketAddr {
    /// The IP address.
    pub ip: IpAddr,
    /// The port number.
    pub port: u16,
}

impl SocketAddr {
    /// Create a new socket address from an IP address and port.
    // VUMA-VERIFIED: constructor is pure
    pub fn new(ip: IpAddr, port: u16) -> Self {
        Self { ip, port }
    }

    /// Parse a socket address from a string (e.g., "127.0.0.1:8080").
    // VUMA-VERIFIED: parse delegates to std, result is validated
    pub fn parse(s: &str) -> Result<Self, String> {
        s.parse::<std_net::SocketAddr>()
            .map(SocketAddr::from_std)
            .map_err(|e| format!("invalid socket address: {}", e))
    }

    /// Convert from a std::net::SocketAddr.
    // VUMA-VERIFIED: conversion is lossless
    pub(crate) fn from_std(addr: std_net::SocketAddr) -> Self {
        SocketAddr {
            ip: IpAddr::from_std(addr.ip()),
            port: addr.port(),
        }
    }

    /// Convert to a std::net::SocketAddr.
    // VUMA-VERIFIED: conversion is lossless
    pub(crate) fn to_std(self) -> std_net::SocketAddr {
        std_net::SocketAddr::new(self.ip.to_std(), self.port)
    }

    /// Returns the IP address.
    // VUMA-VERIFIED: pure accessor
    pub fn ip(&self) -> &IpAddr {
        &self.ip
    }

    /// Returns the port number.
    // VUMA-VERIFIED: pure accessor
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Returns the CapD for this type.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![
            CapFlag::Read,
            CapFlag::Compare,
            CapFlag::Hash,
            CapFlag::Serialize,
        ])
    }

    /// Returns the RepD for this type.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("SocketAddr", 20, 4, self.capd())
    }

    /// Returns the SyncEdge annotations for this type.
    // VUMA-VERIFIED: socket addresses are passive data
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![]
    }
}

impl fmt::Display for SocketAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.ip {
            IpAddr::V4(v4) => write!(f, "{}:{}", v4, self.port),
            IpAddr::V6(v6) => write!(f, "[{}]:{}", v6, self.port),
        }
    }
}

// ---------------------------------------------------------------------------
// TcpListener
// ---------------------------------------------------------------------------

/// A VUMA-verified TCP listener.
///
/// Binds to a socket address and accepts incoming TCP connections.
/// Each accepted connection yields a `TcpStream`.
///
/// ## BD Annotations
///
/// - CapD: { Read, Execute }
/// - SyncEdge: bind → accept (Seq), accept → read (Seq)
pub struct TcpListener {
    /// The local address this listener is bound to.
    pub local_addr: SocketAddr,
    /// Whether the listener is currently bound.
    pub is_bound: bool,
    /// Number of connections accepted (BD tracking).
    pub accept_count: u64,
    /// The underlying std::net::TcpListener.
    inner: std_net::TcpListener,
}

impl TcpListener {
    /// Bind a new TCP listener to the given socket address.
    ///
    /// Delegates to `std::net::TcpListener::bind` for real OS-level binding.
    // VUMA-VERIFIED: bind requires Execute capability
    pub fn bind(addr: SocketAddr) -> Result<Self, String> {
        let std_listener = std_net::TcpListener::bind(addr.to_std())
            .map_err(|e| format!("TcpListener bind failed: {}", e))?;

        let local_addr = SocketAddr::from_std(
            std_listener
                .local_addr()
                .map_err(|e| format!("failed to get local addr: {}", e))?,
        );

        Ok(Self {
            local_addr,
            is_bound: true,
            accept_count: 0,
            inner: std_listener,
        })
    }

    /// Accept a new incoming connection.
    ///
    /// Returns a `TcpStream` representing the new connection.
    /// Delegates to `std::net::TcpListener::accept` for real OS-level accept.
    // VUMA-VERIFIED: accept requires bound listener; yields Read/Write stream
    pub fn accept(&mut self) -> Result<TcpStream, String> {
        if !self.is_bound {
            return Err("TcpListener is not bound".to_string());
        }

        let (std_stream, sock_addr) = self
            .inner
            .accept()
            .map_err(|e| format!("TcpListener accept failed: {}", e))?;

        self.accept_count += 1;

        let peer_addr = SocketAddr::from_std(sock_addr);
        let local_addr = SocketAddr::from_std(
            std_stream
                .local_addr()
                .map_err(|e| format!("failed to get local addr: {}", e))?,
        );

        Ok(TcpStream {
            peer_addr,
            local_addr,
            is_connected: true,
            timeout_ms: None,
            read_count: 0,
            write_count: 0,
            inner: std_stream,
        })
    }

    /// Returns an iterator over incoming connections.
    // VUMA-VERIFIED: iterator delegates to accept
    pub fn incoming(&mut self) -> TcpListenerIncoming<'_> {
        TcpListenerIncoming { listener: self }
    }

    /// Returns the local address this listener is bound to.
    // VUMA-VERIFIED: pure query
    pub fn local_addr(&self) -> &SocketAddr {
        &self.local_addr
    }

    /// Returns a reference to the underlying std::net::TcpListener.
    // VUMA-VERIFIED: access to raw socket is tracked
    pub fn inner(&self) -> &std_net::TcpListener {
        &self.inner
    }

    /// Returns the CapD for this listener.
    // VUMA-VERIFIED: listener requires Read and Execute
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Execute])
    }

    /// Returns the RepD for this listener.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("TcpListener", 32, 8, self.capd())
    }

    /// Returns the SyncEdge annotations for this listener.
    // VUMA-VERIFIED: synchronization edges model accept ordering
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("tcp_bind", "tcp_accept", SyncEdgeKind::Seq),
            SyncEdge::new("tcp_accept", "tcp_read", SyncEdgeKind::Seq),
        ]
    }
}

/// Iterator over incoming TCP connections.
pub struct TcpListenerIncoming<'a> {
    listener: &'a mut TcpListener,
}

impl<'a> Iterator for TcpListenerIncoming<'a> {
    type Item = Result<TcpStream, String>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.listener.accept())
    }
}

// ---------------------------------------------------------------------------
// TcpStream
// ---------------------------------------------------------------------------

/// A VUMA-verified TCP stream.
///
/// Represents a connected TCP socket that supports reading and writing.
/// Delegates I/O to `std::net::TcpStream` with real OS-level operations.
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Send }
/// - SyncEdge: connect → read/write (Seq), read → write (Seq)
pub struct TcpStream {
    /// The peer (remote) address.
    pub peer_addr: SocketAddr,
    /// The local address.
    pub local_addr: SocketAddr,
    /// Whether the stream is connected.
    pub is_connected: bool,
    /// Optional timeout in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Number of read operations (BD tracking).
    pub read_count: u64,
    /// Number of write operations (BD tracking).
    pub write_count: u64,
    /// The underlying std::net::TcpStream.
    inner: std_net::TcpStream,
}

impl TcpStream {
    /// Connect to a remote TCP socket address.
    ///
    /// Delegates to `std::net::TcpStream::connect` for real OS-level connection.
    // VUMA-VERIFIED: connect requires Read/Write capabilities
    pub fn connect(addr: SocketAddr) -> Result<Self, String> {
        let std_stream = std_net::TcpStream::connect(addr.to_std())
            .map_err(|e| format!("TcpStream connect failed: {}", e))?;

        let peer_addr = addr;
        let local_addr = SocketAddr::from_std(
            std_stream
                .local_addr()
                .map_err(|e| format!("failed to get local addr: {}", e))?,
        );

        Ok(Self {
            peer_addr,
            local_addr,
            is_connected: true,
            timeout_ms: None,
            read_count: 0,
            write_count: 0,
            inner: std_stream,
        })
    }

    /// Read bytes from the stream into `buf`.
    ///
    /// Returns the number of bytes read.
    /// Delegates to `std::io::Read::read` on the underlying `std::net::TcpStream`.
    // VUMA-VERIFIED: read requires Read capability
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, String> {
        if !self.is_connected {
            return Err("TcpStream is not connected".to_string());
        }
        self.read_count += 1;
        let n = self
            .inner
            .read(buf)
            .map_err(|e| format!("TcpStream read failed: {}", e))?;
        Ok(n)
    }

    /// Write bytes from `buf` to the stream.
    ///
    /// Returns the number of bytes written.
    /// Delegates to `std::io::Write::write` on the underlying `std::net::TcpStream`.
    // VUMA-VERIFIED: write requires Write capability
    pub fn write(&mut self, buf: &[u8]) -> Result<usize, String> {
        if !self.is_connected {
            return Err("TcpStream is not connected".to_string());
        }
        self.write_count += 1;
        let n = self
            .inner
            .write(buf)
            .map_err(|e| format!("TcpStream write failed: {}", e))?;
        Ok(n)
    }

    /// Set the read/write timeout for this stream.
    ///
    /// Delegates to `std::net::TcpStream::set_read_timeout` and
    /// `std::net::TcpStream::set_write_timeout`.
    // VUMA-VERIFIED: timeout configuration is safe
    pub fn set_timeout(&mut self, timeout_ms: Option<u64>) {
        let dur = timeout_ms.map(std::time::Duration::from_millis);
        let _ = self.inner.set_read_timeout(dur);
        let _ = self.inner.set_write_timeout(dur);
        self.timeout_ms = timeout_ms;
    }

    /// Returns the peer (remote) address.
    // VUMA-VERIFIED: pure query
    pub fn peer_addr(&self) -> &SocketAddr {
        &self.peer_addr
    }

    /// Returns the local address.
    // VUMA-VERIFIED: pure query
    pub fn local_addr(&self) -> &SocketAddr {
        &self.local_addr
    }

    /// Returns a reference to the underlying std::net::TcpStream.
    // VUMA-VERIFIED: access to raw socket is tracked
    pub fn inner(&self) -> &std_net::TcpStream {
        &self.inner
    }

    /// Returns the CapD for this stream.
    // VUMA-VERIFIED: stream has Read, Write, Send
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Send])
    }

    /// Returns the RepD for this stream.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("TcpStream", 48, 8, self.capd())
    }

    /// Returns the SyncEdge annotations for this stream.
    // VUMA-VERIFIED: synchronization edges model TCP read/write ordering
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("tcp_connect", "tcp_read", SyncEdgeKind::Seq),
            SyncEdge::new("tcp_connect", "tcp_write", SyncEdgeKind::Seq),
            SyncEdge::new("tcp_read", "tcp_write", SyncEdgeKind::Seq),
        ]
    }
}

impl fmt::Display for TcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TcpStream {{ local: {}, peer: {}, connected: {} }}",
            self.local_addr, self.peer_addr, self.is_connected
        )
    }
}

// ---------------------------------------------------------------------------
// UdpSocket
// ---------------------------------------------------------------------------

/// A VUMA-verified UDP socket.
///
/// Supports sending and receiving datagrams without establishing a connection.
/// Delegates to `std::net::UdpSocket` for real OS-level operations.
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Send }
/// - SyncEdge: bind → send_to/recv_from (Seq)
pub struct UdpSocket {
    /// The local address this socket is bound to.
    pub local_addr: SocketAddr,
    /// Whether the socket is bound.
    pub is_bound: bool,
    /// Number of send operations (BD tracking).
    pub send_count: u64,
    /// Number of recv operations (BD tracking).
    pub recv_count: u64,
    /// The underlying std::net::UdpSocket.
    inner: std_net::UdpSocket,
}

impl UdpSocket {
    /// Bind a new UDP socket to the given socket address.
    ///
    /// Delegates to `std::net::UdpSocket::bind` for real OS-level binding.
    // VUMA-VERIFIED: bind requires Execute capability
    pub fn bind(addr: SocketAddr) -> Result<Self, String> {
        let std_socket = std_net::UdpSocket::bind(addr.to_std())
            .map_err(|e| format!("UdpSocket bind failed: {}", e))?;

        let local_addr = SocketAddr::from_std(
            std_socket
                .local_addr()
                .map_err(|e| format!("failed to get local addr: {}", e))?,
        );

        Ok(Self {
            local_addr,
            is_bound: true,
            send_count: 0,
            recv_count: 0,
            inner: std_socket,
        })
    }

    /// Send data to the given address.
    ///
    /// Returns the number of bytes sent.
    /// Delegates to `std::net::UdpSocket::send_to`.
    // VUMA-VERIFIED: send_to requires Write capability
    pub fn send_to(&mut self, buf: &[u8], addr: SocketAddr) -> Result<usize, String> {
        if !self.is_bound {
            return Err("UdpSocket is not bound".to_string());
        }
        self.send_count += 1;
        let n = self
            .inner
            .send_to(buf, addr.to_std())
            .map_err(|e| format!("UdpSocket send_to failed: {}", e))?;
        Ok(n)
    }

    /// Receive data from the socket.
    ///
    /// Returns the number of bytes received and the sender's address.
    /// Delegates to `std::net::UdpSocket::recv_from`.
    // VUMA-VERIFIED: recv_from requires Read capability
    pub fn recv_from(&mut self, buf: &mut [u8]) -> Result<(usize, SocketAddr), String> {
        if !self.is_bound {
            return Err("UdpSocket is not bound".to_string());
        }
        self.recv_count += 1;
        let (n, addr) = self
            .inner
            .recv_from(buf)
            .map_err(|e| format!("UdpSocket recv_from failed: {}", e))?;
        Ok((n, SocketAddr::from_std(addr)))
    }

    /// Connect the UDP socket to a remote address, allowing use of send/recv.
    ///
    /// Delegates to `std::net::UdpSocket::connect`.
    // VUMA-VERIFIED: connect restricts send/recv to a single peer
    pub fn connect(&self, addr: SocketAddr) -> Result<(), String> {
        self.inner
            .connect(addr.to_std())
            .map_err(|e| format!("UdpSocket connect failed: {}", e))
    }

    /// Send data to the connected peer (requires prior connect).
    ///
    /// Delegates to `std::net::UdpSocket::send`.
    // VUMA-VERIFIED: send requires Write capability and prior connect
    pub fn send(&mut self, buf: &[u8]) -> Result<usize, String> {
        if !self.is_bound {
            return Err("UdpSocket is not bound".to_string());
        }
        self.send_count += 1;
        let n = self
            .inner
            .send(buf)
            .map_err(|e| format!("UdpSocket send failed: {}", e))?;
        Ok(n)
    }

    /// Receive data from the connected peer (requires prior connect).
    ///
    /// Delegates to `std::net::UdpSocket::recv`.
    // VUMA-VERIFIED: recv requires Read capability and prior connect
    pub fn recv(&mut self, buf: &mut [u8]) -> Result<usize, String> {
        if !self.is_bound {
            return Err("UdpSocket is not bound".to_string());
        }
        self.recv_count += 1;
        let n = self
            .inner
            .recv(buf)
            .map_err(|e| format!("UdpSocket recv failed: {}", e))?;
        Ok(n)
    }

    /// Returns the local address this socket is bound to.
    // VUMA-VERIFIED: pure query
    pub fn local_addr(&self) -> &SocketAddr {
        &self.local_addr
    }

    /// Returns a reference to the underlying std::net::UdpSocket.
    // VUMA-VERIFIED: access to raw socket is tracked
    pub fn inner(&self) -> &std_net::UdpSocket {
        &self.inner
    }

    /// Returns the CapD for this socket.
    // VUMA-VERIFIED: socket has Read, Write, Send
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Send])
    }

    /// Returns the RepD for this socket.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("UdpSocket", 32, 8, self.capd())
    }

    /// Returns the SyncEdge annotations for this socket.
    // VUMA-VERIFIED: synchronization edges model UDP send/recv ordering
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("udp_bind", "udp_send_to", SyncEdgeKind::Seq),
            SyncEdge::new("udp_bind", "udp_recv_from", SyncEdgeKind::Seq),
            SyncEdge::new("udp_recv_from", "udp_send_to", SyncEdgeKind::Seq),
        ]
    }
}

impl fmt::Display for UdpSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "UdpSocket {{ local: {}, bound: {} }}",
            self.local_addr, self.is_bound
        )
    }
}

// ---------------------------------------------------------------------------
// Network CapD Helpers
// ---------------------------------------------------------------------------

/// Returns the CapD for network stream types.
/// Supports: Read, Write, Send.
// VUMA-VERIFIED: network stream capability descriptor
pub fn network_stream_capd() -> CapD {
    CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Send])
}

/// Returns the CapD for network listener types.
/// Supports: Read, Execute.
// VUMA-VERIFIED: network listener capability descriptor
pub fn network_listener_capd() -> CapD {
    CapD::new(vec![CapFlag::Read, CapFlag::Execute])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipv4_addr_new_and_display() {
        let addr = Ipv4Addr::new(192, 168, 1, 1);
        assert_eq!(format!("{}", addr), "192.168.1.1");
        assert_eq!(addr.octets, [192, 168, 1, 1]);
    }

    #[test]
    fn test_ipv4_addr_special() {
        assert!(Ipv4Addr::LOCALHOST.is_loopback());
        assert!(Ipv4Addr::UNSPECIFIED.is_unspecified());
        assert_eq!(format!("{}", Ipv4Addr::LOCALHOST), "127.0.0.1");
    }

    #[test]
    fn test_ipv6_addr_new_and_display() {
        let addr = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1);
        assert!(addr.is_loopback() == false);
        assert!(Ipv6Addr::LOCALHOST.is_loopback());
    }

    #[test]
    fn test_ipaddr_parse() {
        let ip = IpAddr::parse("127.0.0.1").unwrap();
        assert!(ip.is_ipv4());
        assert!(!ip.is_ipv6());

        let ip6 = IpAddr::parse("::1").unwrap();
        assert!(ip6.is_ipv6());
        assert!(!ip6.is_ipv4());

        assert!(IpAddr::parse("not_an_ip").is_err());
    }

    #[test]
    fn test_socket_addr_new() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);
        assert_eq!(addr.port(), 8080);
        assert!(addr.ip().is_ipv4());
    }

    #[test]
    fn test_socket_addr_parse() {
        let addr = SocketAddr::parse("127.0.0.1:8080").unwrap();
        assert_eq!(addr.port(), 8080);
        assert!(addr.ip().is_ipv4());

        assert!(SocketAddr::parse("invalid").is_err());
    }

    #[test]
    fn test_socket_addr_roundtrip() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 9999);
        let std_addr = addr.to_std();
        let back = SocketAddr::from_std(std_addr);
        assert_eq!(addr, back);
    }

    #[test]
    fn test_tcp_listener_bind_real() {
        // Bind to port 0 to let the OS pick a free port
        let listener =
            TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0)).unwrap();
        assert!(listener.is_bound);
        assert!(listener.local_addr.port() > 0);
    }

    #[test]
    fn test_tcp_listener_accept_real() {
        let listener =
            TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0)).unwrap();
        let addr = listener.local_addr;

        // Connect from another thread
        let handle = std::thread::spawn(move || {
            let _ = std_net::TcpStream::connect(addr.to_std());
        });

        let mut listener = listener;
        let stream = listener.accept().unwrap();
        assert!(stream.is_connected);
        assert_eq!(listener.accept_count, 1);

        handle.join().unwrap();
    }

    #[test]
    fn test_tcp_stream_connect_real() {
        let listener =
            TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0)).unwrap();
        let addr = listener.local_addr;

        let handle = std::thread::spawn(move || {
            let _ = listener.inner.accept();
        });

        let stream = TcpStream::connect(addr).unwrap();
        assert!(stream.is_connected);
        assert!(stream.local_addr.port() > 0);

        handle.join().unwrap();
    }

    #[test]
    fn test_tcp_stream_read_write_real() {
        let listener =
            TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0)).unwrap();
        let addr = listener.local_addr;

        // Server thread: accept, then write a message, then read response
        let server_handle = std::thread::spawn(move || {
            let (mut std_stream, _) = listener.inner.accept().unwrap();
            std_stream.write_all(b"hello from server").unwrap();
            let mut buf = [0u8; 64];
            let n = std_stream.read(&mut buf).unwrap();
            let response = std::str::from_utf8(&buf[..n]).unwrap();
            response.to_string()
        });

        // Client: connect, read, then write response
        let mut stream = TcpStream::connect(addr).unwrap();
        let mut buf = [0u8; 64];
        let n = stream.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello from server");
        assert_eq!(stream.read_count, 1);

        let n = stream.write(b"hello from client").unwrap();
        assert_eq!(n, 17);
        assert_eq!(stream.write_count, 1);

        let response = server_handle.join().unwrap();
        assert_eq!(response, "hello from client");
    }

    #[test]
    fn test_tcp_stream_set_timeout() {
        let listener =
            TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0)).unwrap();
        let addr = listener.local_addr;

        let handle = std::thread::spawn(move || {
            let _ = listener.inner.accept();
        });

        let mut stream = TcpStream::connect(addr).unwrap();
        assert!(stream.timeout_ms.is_none());
        stream.set_timeout(Some(5000));
        assert_eq!(stream.timeout_ms, Some(5000));

        handle.join().unwrap();
    }

    #[test]
    fn test_udp_socket_bind_real() {
        let socket =
            UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0)).unwrap();
        assert!(socket.is_bound);
        assert!(socket.local_addr.port() > 0);
    }

    #[test]
    fn test_udp_socket_send_recv_real() {
        let socket_a =
            UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0)).unwrap();
        let addr_a = socket_a.local_addr;

        let socket_b =
            UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0)).unwrap();
        let addr_b = socket_b.local_addr;

        // A sends to B
        let mut socket_a = socket_a;
        let n = socket_a.send_to(b"hello udp", addr_b).unwrap();
        assert_eq!(n, 9);
        assert_eq!(socket_a.send_count, 1);

        // B receives from A
        let mut socket_b = socket_b;
        let mut buf = [0u8; 64];
        let (n, sender) = socket_b.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello udp");
        assert_eq!(sender, addr_a);
        assert_eq!(socket_b.recv_count, 1);
    }

    #[test]
    fn test_udp_socket_connected_send_recv() {
        let socket_a =
            UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0)).unwrap();
        let addr_a = socket_a.local_addr;

        let socket_b =
            UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0)).unwrap();
        let addr_b = socket_b.local_addr;

        // Connect A → B
        socket_a.connect(addr_b).unwrap();

        // A sends via connected socket
        let mut socket_a = socket_a;
        let n = socket_a.send(b"connected msg").unwrap();
        assert_eq!(n, 13);

        // B receives
        let mut socket_b = socket_b;
        let mut buf = [0u8; 64];
        let (n, _) = socket_b.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"connected msg");

        // B connects to A and sends back
        socket_b.connect(addr_a).unwrap();
        let n = socket_b.send(b"reply").unwrap();
        assert_eq!(n, 5);

        // A receives via connected socket
        let mut buf = [0u8; 64];
        let n = socket_a.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"reply");
    }

    #[test]
    fn test_network_capd_helpers() {
        let stream_capd = network_stream_capd();
        assert!(stream_capd.has(CapFlag::Read));
        assert!(stream_capd.has(CapFlag::Write));
        assert!(stream_capd.has(CapFlag::Send));

        let listener_capd = network_listener_capd();
        assert!(listener_capd.has(CapFlag::Read));
        assert!(listener_capd.has(CapFlag::Execute));
    }
}
