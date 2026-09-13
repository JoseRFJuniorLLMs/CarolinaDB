//! CarolinaDB request identity, client protocol, wire encoding and compatibility (SPEC-012).

pub mod envelope;
pub mod negotiation;
pub mod records;
pub mod registry;
pub mod snapshot;

pub use records::*;

#[cfg(test)]
mod tests {
    use super::envelope::*;
    use super::negotiation::*;
    use super::records::*;
    use super::registry::*;
    use carolina_core::canon::{CanonValue, Canonical};
    use carolina_core::error::ErrorCode;
    use carolina_core::hash::Hash256;
    use carolina_core::ids::*;
    use carolina_core::limits::Limits;

    fn key() -> RequestKey {
        RequestKey {
            tenant_id: TenantId::derive("t"),
            request_namespace: RequestNamespace::derive("inventory"),
            stable_request_id: StableRequestId::derive("r1"),
        }
    }

    fn content(args: CanonValue) -> RequestContentV1 {
        RequestContentV1 {
            request_key: key(),
            operation: OperationRef {
                operation_id: OperationId(1),
                version: 1,
            },
            operation_hash: OperationHash(Hash256::ZERO),
            schema_hash: SchemaHash(Hash256::ZERO),
            contract_hash: ContractHash(Hash256::ZERO),
            arguments: args,
            read_contract: ReadContractV1::operation_default(Visibility::Serial),
            initial_session: None,
        }
    }

    fn receipt(bytes: Vec<u8>) -> FinalReceiptV1 {
        let ar = AcceptedResultV1::new(
            Outcome::Committed,
            "canonical-value:1",
            Hash256::ZERO,
            bytes,
            CanonValue::obj().build(),
            None,
        );
        FinalReceiptV1 {
            receipt_version: 1,
            cluster_id: ClusterId::derive("c"),
            request_key: key(),
            request_hash: content(CanonValue::Null).request_hash(),
            txn_id: TxnId::new(
                RequestHomeId::derive("h"),
                RequestHomeEpoch(1),
                RequestAllocationSeq(1),
            ),
            operation: OperationRef {
                operation_id: OperationId(1),
                version: 1,
            },
            operation_hash: OperationHash(Hash256::ZERO),
            schema_hash: SchemaHash(Hash256::ZERO),
            contract_hash: ContractHash(Hash256::ZERO),
            plan: PlanRef::default(),
            idc_bindings: vec![IdcBinding::default()],
            origin_ids: vec![],
            outcome: ar.outcome,
            result_codec: ar.result_codec.clone(),
            result_type_hash: ar.result_type_hash,
            exact_result_bytes: ar.exact_result_bytes.clone(),
            result_digest: ar.result_digest,
            commitments: ar.commitments.clone(),
            observation_token: None,
            durability_policy: PolicyRef {
                name: "local".into(),
                policy_hash: Hash256::ZERO,
            },
            durability_evidence: vec![],
            decision_ref: ProtocolRecordRef {
                record_kind: "local_decision".into(),
                record_version: 1,
                record_key: vec![1],
                payload_hash: Hash256::ZERO,
            },
            completion_ref: None,
        }
    }

    /// CP-02: changed arguments change the request hash; identical content gives identical hash.
    #[test]
    fn request_hash_binds_content() {
        let a = content(CanonValue::str("x")).request_hash();
        let b = content(CanonValue::str("x")).request_hash();
        let c = content(CanonValue::str("y")).request_hash();
        assert_eq!(a, b);
        assert_ne!(a, c);
        let mut inv = InvokeV1 {
            content: content(CanonValue::str("x")),
            request_hash: a,
            attempt_id: AttemptId::derive("a1"),
            deadline_budget_ms: 1000,
            route_hint: None,
            accepted_result_codecs: vec!["canonical-value:1".into()],
        };
        assert!(inv.verify_hash().is_ok());
        inv.request_hash = c;
        assert_eq!(
            inv.verify_hash().unwrap_err().code,
            ErrorCode::RequestIdentityMismatch
        );
        // attempt id is not part of identity
        let inv2 = InvokeV1 {
            attempt_id: AttemptId::derive("a2"),
            ..inv.clone()
        };
        assert_eq!(inv2.content.request_hash(), inv.content.request_hash());
        let bytes = inv2.encode();
        assert_eq!(InvokeV1::decode(&bytes, &Limits::v1()).unwrap(), inv2);
    }

    /// CP-06/CP-12: receipts roundtrip byte-identically; digests verified on decode.
    #[test]
    fn receipt_roundtrip_and_digest_check() {
        let r = receipt(vec![1, 2, 3]);
        let bytes = r.encode();
        let back = FinalReceiptV1::decode(&bytes, &Limits::v1()).unwrap();
        assert_eq!(back, r);
        assert_eq!(back.encode(), bytes);
        assert_eq!(back.receipt_digest(), r.receipt_digest());
        // tamper with result bytes: digest mismatch is detected
        let mut o = r.to_canon().as_object().unwrap().clone();
        o.insert("exact_result_bytes".into(), CanonValue::bytes(&[9]));
        assert_eq!(
            FinalReceiptV1::from_canon(&CanonValue::Object(o))
                .unwrap_err()
                .code,
            ErrorCode::Corruption
        );
        // unknown field rejected
        let mut o = r.to_canon().as_object().unwrap().clone();
        o.insert("zz".into(), CanonValue::Null);
        assert!(FinalReceiptV1::from_canon(&CanonValue::Object(o)).is_err());
        let wrapped = CanonicalRecord::wrap("final_receipt", &r).unwrap();
        let rec = CanonicalRecord::decode_checked(&wrapped.encode(), &Limits::v1()).unwrap();
        assert_eq!(rec.body::<FinalReceiptV1>().unwrap(), r);
        assert!(CanonicalRecord::wrap("bogus", &r).is_err());
    }

    /// Binding invariants: TxnId equals home||epoch||seq; terminal needs receipt; states only advance.
    #[test]
    fn binding_verification() {
        let home = RequestHomeId::derive("h");
        let b = RequestBindingV1 {
            request_key: key(),
            request_hash: content(CanonValue::Null).request_hash(),
            txn_id: TxnId::new(home, RequestHomeEpoch(1), RequestAllocationSeq(1)),
            allocation_home: home,
            allocation_epoch: RequestHomeEpoch(1),
            allocation_seq: RequestAllocationSeq(1),
            record_revision: RecordRevision(1),
            state: BindingState::Bound,
            admitted_plan: None,
            decision_authority: None,
            terminal_receipt: None,
            tombstone: None,
        };
        assert!(b.verify().is_ok());
        assert_eq!(
            RequestBindingV1::decode(&b.encode(), &Limits::v1()).unwrap(),
            b
        );
        let bad = RequestBindingV1 {
            txn_id: TxnId::new(home, RequestHomeEpoch(2), RequestAllocationSeq(1)),
            ..b.clone()
        };
        assert!(bad.verify().is_err());
        let bad = RequestBindingV1 {
            state: BindingState::Terminal,
            ..b.clone()
        };
        assert!(bad.verify().is_err());
        assert!(BindingState::Bound.can_advance_to(BindingState::Terminal));
        assert!(!BindingState::Terminal.can_advance_to(BindingState::Bound));
        assert!(!BindingState::Terminal.can_advance_to(BindingState::Admitted));
    }

    /// CP-09: the header-only golden vector of SPEC-012 §12 and framing negative cases.
    #[test]
    fn envelope_golden_vector_and_negatives() {
        let lim = Limits::v1();
        let frame = encode_frame(MessageKind::Hello, 0, 0, b"{}", &lim).unwrap();
        let expected: Vec<u8> = vec![
            0x41, 0x53, 0x54, 0x52, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x7b, 0x7d,
        ];
        assert_eq!(frame, expected);
        // `{}` must fail Hello semantic validation (missing required fields)
        assert!(HelloV1::decode(b"{}", &lim).is_err());
        let mut reader = FrameReader::new();
        // split delivery
        let frames = reader.feed(&frame[..10], &lim).unwrap();
        assert!(frames.is_empty());
        let frames = reader.feed(&frame[10..], &lim).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].header.kind, MessageKind::Hello);
        // sequence reuse fails the connection
        assert!(reader.feed(&frame, &lim).is_err());
        assert!(
            reader.feed(&frame, &lim).is_err(),
            "poisoned reader dispatches nothing"
        );
        // bad magic / version / flags / oversized
        let mut bad = frame.clone();
        bad[0] = b'X';
        assert!(FrameReader::new().feed(&bad, &lim).is_err());
        let mut bad = frame.clone();
        bad[4] = 2;
        assert_eq!(
            FrameReader::new().feed(&bad, &lim).unwrap_err().code,
            ErrorCode::IncompatiblePeer
        );
        let mut bad = frame.clone();
        bad[10] = 1;
        assert!(FrameReader::new().feed(&bad, &lim).is_err());
        let mut bad = frame.clone();
        bad[12..16].copy_from_slice(&(u32::MAX).to_le_bytes());
        assert_eq!(
            FrameReader::new()
                .feed(&bad, &Limits::tiny())
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimit
        );
        // non-negotiation frame before negotiation
        let inv = encode_frame(MessageKind::Invoke, 7, 0, b"{}", &lim).unwrap();
        assert!(FrameReader::new().feed(&inv, &lim).is_err());
        assert!(encode_frame(MessageKind::Invoke, 0, 0, b"{}", &lim).is_err());
    }

    /// CP-10: negotiation refuses unoffered selections, limit escalation and profile downgrade.
    #[test]
    fn negotiation_downgrade_protection() {
        let lim = Limits::v1();
        let cluster = ClusterId::derive("c");
        let client = LocalCapabilities::v1(cluster, EndpointRole::Client, &lim, "DEV_LOCAL");
        let node = LocalCapabilities::v1(cluster, EndpointRole::Node, &lim, "DEV_LOCAL");
        let hello = client.hello(vec![1, 2, 3]);
        let ack = node.answer(&hello, vec![4, 5, 6]).unwrap();
        assert!(client.verify_ack(&hello, &ack).is_ok());
        assert_eq!(ack.selected_wire_version, "1.0");
        let t1 = transcript_hash(&hello, &ack);
        let mut ack2 = ack.clone();
        ack2.selected_wire_version = "2.0".into();
        assert!(client.verify_ack(&hello, &ack2).is_err());
        assert_ne!(transcript_hash(&hello, &ack2), t1);
        let mut ack3 = ack.clone();
        ack3.limits.max_frame_payload += 1;
        assert!(client.verify_ack(&hello, &ack3).is_err());
        let mut ack4 = ack.clone();
        ack4.selected_capabilities.push("C4_CERTIFIED_V1:1".into());
        assert!(client.verify_ack(&hello, &ack4).is_err());
        // wrong cluster refused
        let other = LocalCapabilities::v1(
            ClusterId::derive("other"),
            EndpointRole::Node,
            &lim,
            "DEV_LOCAL",
        );
        assert_eq!(
            other.answer(&hello, vec![]).unwrap_err().code,
            ErrorCode::IncompatiblePeer
        );
        // a production client refuses a DEV_LOCAL server
        let strict = LocalCapabilities {
            minimum_security_profile: "ENCRYPTED_HOST_V1".into(),
            ..client.clone()
        };
        let h2 = strict.hello(vec![9]);
        assert!(node.answer(&h2, vec![]).is_err());
        // Hello/HelloAck roundtrip through registered records
        let w = CanonicalRecord::wrap("hello", &hello).unwrap();
        assert_eq!(
            CanonicalRecord::decode_checked(&w.encode(), &lim)
                .unwrap()
                .body::<HelloV1>()
                .unwrap(),
            hello
        );
    }

    /// Client replies roundtrip; OutcomeUnknown/Unavailable are the only retry-same-identity replies.
    #[test]
    fn client_reply_roundtrip() {
        let lim = Limits::v1();
        let r = receipt(vec![7]);
        for reply in [
            ClientReplyV1::Committed(r.clone()),
            ClientReplyV1::Rejected(r.clone()),
            ClientReplyV1::Unavailable(RefusalV1 {
                code: "NotReady".into(),
                detail: "".into(),
                possibly_admitted: false,
            }),
            ClientReplyV1::OutcomeUnknown(ResolutionHintV1 {
                request_key: key(),
                request_hash: r.request_hash,
                txn_id: None,
                resolver: None,
            }),
            ClientReplyV1::RequestIdentityMismatch,
            ClientReplyV1::ResultExpired(ResultTombstoneV1 {
                request_key: key(),
                request_hash: r.request_hash,
                txn_id: r.txn_id,
                outcome: Outcome::Committed,
                receipt_digest: r.receipt_digest(),
                decision_ref: r.decision_ref.clone(),
            }),
            ClientReplyV1::IdentityExpired {
                namespace: key().request_namespace,
                retirement_ref: r.decision_ref.clone(),
            },
            ClientReplyV1::ProtocolError {
                code: "UnsupportedCodec".into(),
                detail: "x".into(),
            },
        ] {
            let back = ClientReplyV1::decode(&reply.encode(), &lim).unwrap();
            assert_eq!(back, reply);
            let retry = reply.retry_same_identity();
            assert_eq!(
                retry,
                matches!(
                    reply,
                    ClientReplyV1::Unavailable(_) | ClientReplyV1::OutcomeUnknown(_)
                )
            );
        }
        let rr = ResolveReplyV1::Terminal(Box::new(ClientReplyV1::Committed(r)));
        assert_eq!(ResolveReplyV1::decode(&rr.encode(), &lim).unwrap(), rr);
    }
}
