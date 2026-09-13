//! Synchronous client for tests and the CLI: one negotiated connection, one request at a time.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use carolina_catalog::CatalogCommand;
use carolina_core::canon::Canonical;
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::ids::ClusterId;
use carolina_core::limits::Limits;
use carolina_lang::types::Value;
use carolina_wire::envelope::{FrameReader, MessageKind};
use carolina_wire::negotiation::EndpointRole;
use carolina_wire::records::{ClientReplyV1, InvokeV1, ResolveReplyV1, ResolveRequestV1};

use crate::protocol::{AdminReply, AdminRequest, NodeStatus};
use crate::transport::{capabilities, negotiate_client, require_loopback, Writer};

pub struct Client {
    stream: TcpStream,
    writer: Writer,
    reader: FrameReader,
    next_stream: u64,
}

impl Client {
    pub fn connect(
        addr: SocketAddr,
        cluster: ClusterId,
        role: EndpointRole,
        timeout: Duration,
    ) -> CoreResult<Client> {
        require_loopback(addr)?;
        let mut stream = TcpStream::connect_timeout(&addr, timeout)
            .map_err(|e| CoreError::new(ErrorCode::Unavailable, format!("connect {addr}: {e}")))?;
        stream.set_read_timeout(Some(timeout)).ok();
        stream.set_nodelay(true).ok();
        let caps = capabilities(cluster, role);
        let nonce = carolina_core::hash::sha256(format!("{addr}").as_bytes()).0[..16].to_vec();
        let (writer, reader, _ack) = negotiate_client(&mut stream, &caps, &nonce)?;
        Ok(Client {
            stream,
            writer,
            reader,
            next_stream: 1,
        })
    }

    fn call(
        &mut self,
        kind: MessageKind,
        payload: &[u8],
        expect: MessageKind,
    ) -> CoreResult<Vec<u8>> {
        let sid = self.next_stream;
        self.next_stream += 1;
        self.writer.send(kind, sid, payload)?;
        let limits = Limits::v1();
        let mut buf = [0u8; 8192];
        loop {
            let n = std::io::Read::read(&mut self.stream, &mut buf)
                .map_err(|e| CoreError::new(ErrorCode::Unavailable, format!("read: {e}")))?;
            if n == 0 {
                return Err(CoreError::new(ErrorCode::Unavailable, "connection closed"));
            }
            let frames = self.reader.feed(&buf[..n], &limits)?;
            for f in frames {
                if f.header.stream_id == sid {
                    if f.header.kind != expect {
                        return Err(CoreError::new(
                            ErrorCode::ProtocolError,
                            format!("unexpected reply kind {:?}", f.header.kind),
                        ));
                    }
                    return Ok(f.payload);
                }
            }
        }
    }

    pub fn invoke(&mut self, inv: &InvokeV1) -> CoreResult<ClientReplyV1> {
        let bytes = self.call(MessageKind::Invoke, &inv.encode(), MessageKind::Reply)?;
        ClientReplyV1::decode(&bytes, &Limits::v1())
    }

    pub fn resolve(&mut self, req: &ResolveRequestV1) -> CoreResult<ResolveReplyV1> {
        let bytes = self.call(
            MessageKind::ResolveRequest,
            &req.encode(),
            MessageKind::ResolveReply,
        )?;
        ResolveReplyV1::decode(&bytes, &Limits::v1())
    }

    pub fn admin(&mut self, req: &AdminRequest) -> CoreResult<AdminReply> {
        let bytes = self.call(MessageKind::Admin, &req.encode(), MessageKind::AdminReply)?;
        AdminReply::decode(&bytes, &Limits::v1())
    }

    pub fn status(&mut self) -> CoreResult<NodeStatus> {
        match self.admin(&AdminRequest::Status)? {
            AdminReply::Status(s) => Ok(s),
            other => Err(CoreError::new(
                ErrorCode::ProtocolError,
                format!("{other:?}"),
            )),
        }
    }

    pub fn catalog(&mut self, cmd: &CatalogCommand) -> CoreResult<AdminReply> {
        self.admin(&AdminRequest::Catalog(cmd.clone()))
    }

    pub fn seed(
        &mut self,
        label: &str,
        record: &str,
        rows: Vec<(Value, Vec<(String, Value)>)>,
    ) -> CoreResult<AdminReply> {
        self.admin(&AdminRequest::Seed {
            label: label.into(),
            record: record.into(),
            rows,
        })
    }
}
