#![no_main]

use carolina_core::canon::Canonical;
use carolina_core::limits::Limits;
use carolina_node::protocol::{AdminReply, AdminRequest, NodeCommand, NodeStatus};
use carolina_wire::envelope::FrameReader;
use carolina_wire::negotiation::{HelloAckV1, HelloV1};
use carolina_wire::records::{
    AcceptedResultV1, ClientReplyV1, FinalReceiptV1, InvokeV1, RequestBindingV1, ResolveReplyV1,
    ResolveRequestV1,
};
use carolina_wire::snapshot::{CodecManifest, SnapshotChunkV1, SnapshotManifestV1};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits::tiny();
    let _ = HelloV1::decode(data, &limits);
    let _ = HelloAckV1::decode(data, &limits);
    let _ = InvokeV1::decode(data, &limits);
    let _ = ResolveRequestV1::decode(data, &limits);
    let _ = ResolveReplyV1::decode(data, &limits);
    let _ = RequestBindingV1::decode(data, &limits);
    let _ = AcceptedResultV1::decode(data, &limits);
    let _ = FinalReceiptV1::decode(data, &limits);
    let _ = ClientReplyV1::decode(data, &limits);
    let _ = SnapshotManifestV1::decode(data, &limits);
    let _ = SnapshotChunkV1::decode(data, &limits);
    let _ = CodecManifest::decode(data, &limits);
    let _ = NodeCommand::decode(data, &limits);
    let _ = AdminRequest::decode(data, &limits);
    let _ = AdminReply::decode(data, &limits);
    let _ = NodeStatus::decode(data, &limits);
    let mut frames = FrameReader::new();
    let _ = frames.feed(data, &limits);
});
