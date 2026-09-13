//! `ASTR` framing over TCP (SPEC-012 §9–§10) with the `DEV_LOCAL` security profile.
//!
//! Every connection starts with a Hello/HelloAck negotiation on stream 0; the negotiated role of
//! the connection bounds what it may send: `Consensus` frames are accepted only from `Node`
//! peers, `Admin` frames only from `Admin` endpoints, `Invoke`/`ResolveRequest` from `Client`
//! or `Admin`. A frame violating that is a protocol failure that closes the connection.
//! These roles are declarations, not authenticated identities. All sockets must use loopback;
//! the plaintext profile is only suitable for trusted processes on the same host.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use carolina_consensus::Envelope;
use carolina_core::canon::Canonical;
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::ids::ClusterId;
use carolina_core::limits::Limits;
use carolina_wire::envelope::{encode_frame, FrameReader, MessageKind};
use carolina_wire::negotiation::{EndpointRole, HelloAckV1, HelloV1, LocalCapabilities};

pub const SECURITY_PROFILE: &str = "DEV_LOCAL";

pub(crate) fn require_loopback(addr: SocketAddr) -> CoreResult<()> {
    if !addr.ip().is_loopback() {
        return Err(CoreError::new(
            ErrorCode::InvalidManifest,
            format!("DEV_LOCAL requires a loopback address: {addr}"),
        ));
    }
    Ok(())
}

fn require_local_stream(stream: &TcpStream) -> CoreResult<()> {
    require_loopback(stream.local_addr()?)?;
    require_loopback(stream.peer_addr()?)
}

pub type ConnId = u64;

/// Something the core loop must handle.
#[derive(Debug)]
pub enum Event {
    Peer(Envelope),
    Client {
        conn: ConnId,
        role: EndpointRole,
        kind: MessageKind,
        stream_id: u64,
        payload: Vec<u8>,
    },
    Disconnected(ConnId),
    Tick,
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct Outbound {
    pub kind: MessageKind,
    pub stream_id: u64,
    pub payload: Vec<u8>,
}

/// One direction of a negotiated connection: sequence numbers start at 0 after negotiation.
pub struct Writer {
    stream: TcpStream,
    seq: u64,
    limits: Limits,
}

impl Writer {
    pub fn new(stream: TcpStream) -> Writer {
        Writer {
            stream,
            seq: 0,
            limits: Limits::v1(),
        }
    }
    pub fn send(&mut self, kind: MessageKind, stream_id: u64, payload: &[u8]) -> CoreResult<()> {
        let bytes = encode_frame(kind, stream_id, self.seq, payload, &self.limits)?;
        self.seq += 1;
        self.stream
            .write_all(&bytes)
            .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()))?;
        self.stream
            .flush()
            .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()))
    }
}

pub fn capabilities(cluster: ClusterId, role: EndpointRole) -> LocalCapabilities {
    LocalCapabilities::v1(cluster, role, &Limits::v1(), SECURITY_PROFILE)
}

fn read_frame(
    stream: &mut TcpStream,
    reader: &mut FrameReader,
    limits: &Limits,
) -> CoreResult<Option<carolina_wire::envelope::Frame>> {
    let mut buf = [0u8; 8192];
    loop {
        let n = stream
            .read(&mut buf)
            .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()))?;
        if n == 0 {
            return Ok(None);
        }
        let mut frames = reader.feed(&buf[..n], limits)?;
        if !frames.is_empty() {
            // one frame per call; the remainder waits in the thread-local queue
            let first = frames.remove(0);
            PENDING.with(|p| p.borrow_mut().extend(frames));
            return Ok(Some(first));
        }
    }
}

thread_local! {
    static PENDING: std::cell::RefCell<Vec<carolina_wire::envelope::Frame>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn next_frame(
    stream: &mut TcpStream,
    reader: &mut FrameReader,
    limits: &Limits,
) -> CoreResult<Option<carolina_wire::envelope::Frame>> {
    if let Some(f) = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        if p.is_empty() {
            None
        } else {
            Some(p.remove(0))
        }
    }) {
        return Ok(Some(f));
    }
    read_frame(stream, reader, limits)
}

/// Client-side negotiation: send Hello, expect HelloAck, verify.
pub fn negotiate_client(
    stream: &mut TcpStream,
    caps: &LocalCapabilities,
    nonce: &[u8],
) -> CoreResult<(Writer, FrameReader, HelloAckV1)> {
    require_local_stream(stream)?;
    let limits = Limits::v1();
    let hello = caps.hello(nonce.to_vec());
    let mut writer = Writer::new(
        stream
            .try_clone()
            .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()))?,
    );
    writer.send(MessageKind::Hello, 0, &hello.encode())?;
    let mut reader = FrameReader::new();
    let frame = next_frame(stream, &mut reader, &limits)?
        .ok_or_else(|| CoreError::new(ErrorCode::Io, "connection closed during negotiation"))?;
    if frame.header.kind != MessageKind::HelloAck {
        return Err(CoreError::new(
            ErrorCode::ProtocolError,
            "expected HelloAck",
        ));
    }
    let ack = HelloAckV1::decode(&frame.payload, &limits)?;
    caps.verify_ack(&hello, &ack)?;
    Ok((writer, reader, ack))
}

/// Server-side negotiation: expect Hello, answer HelloAck. Returns the peer's declared role.
pub fn negotiate_server(
    stream: &mut TcpStream,
    caps: &LocalCapabilities,
    nonce: &[u8],
) -> CoreResult<(Writer, FrameReader, EndpointRole)> {
    require_local_stream(stream)?;
    let limits = Limits::v1();
    let mut reader = FrameReader::new();
    let frame = next_frame(stream, &mut reader, &limits)?
        .ok_or_else(|| CoreError::new(ErrorCode::Io, "connection closed during negotiation"))?;
    if frame.header.kind != MessageKind::Hello {
        return Err(CoreError::new(ErrorCode::ProtocolError, "expected Hello"));
    }
    let hello = HelloV1::decode(&frame.payload, &limits)?;
    let ack = caps.answer(&hello, nonce.to_vec())?;
    let mut writer = Writer::new(
        stream
            .try_clone()
            .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()))?,
    );
    writer.send(MessageKind::HelloAck, 0, &ack.encode())?;
    Ok((writer, reader, hello.role))
}

/// Connection registry shared between the acceptor threads and the core loop.
#[derive(Default)]
pub struct Connections {
    writers: Mutex<HashMap<ConnId, Sender<Outbound>>>,
}

impl Connections {
    #[cfg(test)]
    pub(crate) fn test_receiver(&self, conn: ConnId) -> Receiver<Outbound> {
        let (tx, rx) = channel();
        self.register(conn, tx);
        rx
    }

    pub fn reply(&self, conn: ConnId, out: Outbound) {
        if let Some(tx) = self.writers.lock().unwrap().get(&conn) {
            let _ = tx.send(out);
        }
    }
    fn register(&self, conn: ConnId, tx: Sender<Outbound>) {
        self.writers.lock().unwrap().insert(conn, tx);
    }
    fn remove(&self, conn: ConnId) {
        self.writers.lock().unwrap().remove(&conn);
    }
}

/// Frame kinds a negotiated endpoint role may send (SPEC-013 is not implemented: the role is a
/// declaration, so this gate is a structural constraint, not authentication).
fn role_allows(role: EndpointRole, kind: MessageKind) -> bool {
    match kind {
        MessageKind::Consensus => role == EndpointRole::Node,
        MessageKind::Admin => role == EndpointRole::Admin,
        MessageKind::Invoke | MessageKind::ResolveRequest => {
            matches!(role, EndpointRole::Client | EndpointRole::Admin)
        }
        _ => false,
    }
}

/// Accept connections forever; each connection gets a reader thread (events → core) and a writer
/// thread (core → socket).
pub fn serve(
    listener: TcpListener,
    caps: LocalCapabilities,
    conns: Arc<Connections>,
    events: Sender<Event>,
    seed: u64,
) {
    if listener
        .local_addr()
        .map_or(true, |addr| require_loopback(addr).is_err())
    {
        return;
    }
    let mut next_id: ConnId = 1;
    for incoming in listener.incoming() {
        let mut stream = match incoming {
            Ok(s) => s,
            Err(_) => continue,
        };
        if require_local_stream(&stream).is_err() {
            continue;
        }
        let conn = next_id;
        next_id += 1;
        let caps = caps.clone();
        let conns = conns.clone();
        let events = events.clone();
        thread::spawn(move || {
            let nonce = carolina_core::hash::sha256(
                &[seed.to_le_bytes().as_slice(), &conn.to_le_bytes()].concat(),
            )
            .0[..16]
                .to_vec();
            let (writer, mut reader, role) = match negotiate_server(&mut stream, &caps, &nonce) {
                Ok(x) => x,
                Err(_) => return,
            };
            let (tx, rx) = channel::<Outbound>();
            conns.register(conn, tx);
            let mut writer = writer;
            thread::spawn(move || {
                for out in rx {
                    if writer.send(out.kind, out.stream_id, &out.payload).is_err() {
                        break;
                    }
                }
            });
            let limits = Limits::v1();
            while let Ok(Some(frame)) = next_frame(&mut stream, &mut reader, &limits) {
                let kind = frame.header.kind;
                if !role_allows(role, kind) {
                    break; // protocol violation: close
                }
                let ev = if kind == MessageKind::Consensus {
                    match Envelope::decode(&frame.payload, &limits) {
                        Ok(env) => Event::Peer(env),
                        Err(_) => break,
                    }
                } else {
                    Event::Client {
                        conn,
                        role,
                        kind,
                        stream_id: frame.header.stream_id,
                        payload: frame.payload,
                    }
                };
                if events.send(ev).is_err() {
                    break;
                }
            }
            conns.remove(conn);
            let _ = events.send(Event::Disconnected(conn));
            let _ = stream.shutdown(std::net::Shutdown::Both);
        });
    }
}

/// Outgoing peer link: reconnects forever; envelopes are dropped while disconnected (Raft
/// retransmits from durable state).
pub fn peer_link(addr: SocketAddr, caps: LocalCapabilities, rx: Receiver<Envelope>, seed: u64) {
    if require_loopback(addr).is_err() {
        return;
    }
    let mut attempt = 0u64;
    loop {
        attempt += 1;
        let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
            Ok(s) => s,
            Err(_) => {
                // drop queued envelopes while unreachable
                while rx.try_recv().is_ok() {}
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let _ = stream.set_nodelay(true);
        let nonce = carolina_core::hash::sha256(
            &[seed.to_le_bytes().as_slice(), &attempt.to_le_bytes()].concat(),
        )
        .0[..16]
            .to_vec();
        let (mut writer, _reader, _ack) = match negotiate_client(&mut stream, &caps, &nonce) {
            Ok(x) => x,
            Err(_) => {
                thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        loop {
            match rx.recv() {
                Ok(env) => {
                    if writer
                        .send(MessageKind::Consensus, 1, &env.encode())
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-008 §18 / SPEC-013 (unimplemented): frame kinds are bound to the negotiated endpoint
    /// role, so a client-role connection can never carry consensus or admin traffic. The role is a
    /// declaration under `DEV_LOCAL`; this gate is structural, not authentication.
    #[test]
    fn frame_kinds_are_bound_to_the_endpoint_role() {
        use EndpointRole::{Admin, Client, Node};
        for (role, kind, allowed) in [
            (Client, MessageKind::Invoke, true),
            (Client, MessageKind::ResolveRequest, true),
            (Client, MessageKind::Consensus, false),
            (Client, MessageKind::Admin, false),
            (Node, MessageKind::Consensus, true),
            (Node, MessageKind::Invoke, false),
            (Node, MessageKind::Admin, false),
            (Admin, MessageKind::Admin, true),
            (Admin, MessageKind::Invoke, true),
            (Admin, MessageKind::Consensus, false),
        ] {
            assert_eq!(
                role_allows(role, kind),
                allowed,
                "role {role:?} kind {kind:?}"
            );
        }
        // reply kinds are never accepted as inbound traffic from any role
        for role in [Client, Node, Admin] {
            for kind in [
                MessageKind::AdminReply,
                MessageKind::Reply,
                MessageKind::Hello,
                MessageKind::HelloAck,
            ] {
                assert!(!role_allows(role, kind), "role {role:?} kind {kind:?}");
            }
        }
    }

    /// `DEV_LOCAL` is loopback-only on both ends (SECURITY.md).
    #[test]
    fn require_loopback_refuses_public_addresses() {
        for addr in ["127.0.0.1:1", "[::1]:1"] {
            require_loopback(addr.parse().unwrap()).unwrap();
        }
        for addr in ["10.0.0.1:1", "0.0.0.0:1", "[2001:db8::1]:1"] {
            let err = require_loopback(addr.parse().unwrap()).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidManifest, "{addr}");
        }
    }
}
