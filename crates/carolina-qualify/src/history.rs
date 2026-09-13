//! Observable history (SPEC-010 §4): events with logical steps, canonical JSON lines, trace digest.
//!
//! Client-visible replies are recorded with their argument/result digests; durable evidence
//! (`Decision`, `Resolve`) carries the references the system itself returned. Wall-clock time is
//! never recorded: the local runner has an exact total order of steps.

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, sha256, Hash256};

use crate::w1::W1Op;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryEvent {
    /// A client attempt of request `req` (stable id) with content hash `request_hash`.
    Invoke {
        attempt: u64,
        req: String,
        request_key: Vec<u8>,
        request_hash: Hash256,
        op: W1Op,
    },
    AdmissionRefusal {
        attempt: u64,
        req: String,
        code: String,
    },
    FinalReply {
        attempt: u64,
        req: String,
        outcome: String,
        txn_id: Vec<u8>,
        receipt_digest: Hash256,
        result_digest: Hash256,
    },
    UnknownReply {
        attempt: u64,
        req: String,
    },
    /// A reply that names an expired identity/result (`ResultExpired`, `IdentityExpired`).
    ExpiredReply {
        attempt: u64,
        req: String,
        kind: String,
    },
    Decision {
        req: String,
        txn_id: Vec<u8>,
        outcome: String,
        decision_ref: Hash256,
    },
    Crash {
        fault_point: String,
        nth: u32,
    },
    Restart {
        home_epoch: u64,
    },
    Checkpoint,
    Resolve {
        req: String,
        kind: String,
        txn_id: Vec<u8>,
        receipt_digest: Option<Hash256>,
        result_digest: Option<Hash256>,
    },
    ResultEvicted {
        req: String,
    },
    NamespaceRetired {
        namespace: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub step: u64,
    pub event: HistoryEvent,
}

fn bytes_field(v: &CanonValue, name: &str) -> CoreResult<Vec<u8>> {
    v.field(name)?.as_bytes()
}

fn str_field(v: &CanonValue, name: &str) -> CoreResult<String> {
    Ok(v.field(name)?.as_str()?.to_string())
}

fn opt_hash(v: &CanonValue, name: &str) -> CoreResult<Option<Hash256>> {
    match v.field(name)? {
        CanonValue::Null => Ok(None),
        x => Ok(Some(Hash256::from_canon(x)?)),
    }
}

impl Canonical for Event {
    fn to_canon(&self) -> CanonValue {
        let b = CanonValue::obj().fu64("step", self.step);
        match &self.event {
            HistoryEvent::Invoke {
                attempt,
                req,
                request_key,
                request_hash,
                op,
            } => b
                .fu64("attempt", *attempt)
                .fstr("kind", "Invoke")
                .fc("op", op)
                .fstr("req", req)
                .fc("request_hash", request_hash)
                .fbytes("request_key", request_key)
                .build(),
            HistoryEvent::AdmissionRefusal { attempt, req, code } => b
                .fu64("attempt", *attempt)
                .fstr("code", code)
                .fstr("kind", "AdmissionRefusal")
                .fstr("req", req)
                .build(),
            HistoryEvent::FinalReply {
                attempt,
                req,
                outcome,
                txn_id,
                receipt_digest,
                result_digest,
            } => b
                .fu64("attempt", *attempt)
                .fstr("kind", "FinalReply")
                .fstr("outcome", outcome)
                .fc("receipt_digest", receipt_digest)
                .fstr("req", req)
                .fc("result_digest", result_digest)
                .fbytes("txn_id", txn_id)
                .build(),
            HistoryEvent::UnknownReply { attempt, req } => b
                .fu64("attempt", *attempt)
                .fstr("kind", "UnknownReply")
                .fstr("req", req)
                .build(),
            HistoryEvent::ExpiredReply { attempt, req, kind } => b
                .fu64("attempt", *attempt)
                .fstr("expired", kind)
                .fstr("kind", "ExpiredReply")
                .fstr("req", req)
                .build(),
            HistoryEvent::Decision {
                req,
                txn_id,
                outcome,
                decision_ref,
            } => b
                .fc("decision_ref", decision_ref)
                .fstr("kind", "Decision")
                .fstr("outcome", outcome)
                .fstr("req", req)
                .fbytes("txn_id", txn_id)
                .build(),
            HistoryEvent::Crash { fault_point, nth } => b
                .fstr("fault_point", fault_point)
                .fstr("kind", "Crash")
                .fu32("nth", *nth)
                .build(),
            HistoryEvent::Restart { home_epoch } => b
                .fu64("home_epoch", *home_epoch)
                .fstr("kind", "Restart")
                .build(),
            HistoryEvent::Checkpoint => b.fstr("kind", "Checkpoint").build(),
            HistoryEvent::Resolve {
                req,
                kind,
                txn_id,
                receipt_digest,
                result_digest,
            } => b
                .fstr("kind", "Resolve")
                .fopt("receipt_digest", receipt_digest)
                .fstr("req", req)
                .fstr("resolution", kind)
                .fopt("result_digest", result_digest)
                .fbytes("txn_id", txn_id)
                .build(),
            HistoryEvent::ResultEvicted { req } => {
                b.fstr("kind", "ResultEvicted").fstr("req", req).build()
            }
            HistoryEvent::NamespaceRetired { namespace } => b
                .fstr("kind", "NamespaceRetired")
                .fbytes("namespace", namespace)
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        let step = v.field("step")?.as_u64()?;
        let kind = str_field(v, "kind")?;
        let attempt = |v: &CanonValue| v.field("attempt")?.as_u64();
        let event = match kind.as_str() {
            "Invoke" => HistoryEvent::Invoke {
                attempt: attempt(v)?,
                req: str_field(v, "req")?,
                request_key: bytes_field(v, "request_key")?,
                request_hash: Hash256::from_canon(v.field("request_hash")?)?,
                op: W1Op::from_canon(v.field("op")?)?,
            },
            "AdmissionRefusal" => HistoryEvent::AdmissionRefusal {
                attempt: attempt(v)?,
                req: str_field(v, "req")?,
                code: str_field(v, "code")?,
            },
            "FinalReply" => HistoryEvent::FinalReply {
                attempt: attempt(v)?,
                req: str_field(v, "req")?,
                outcome: str_field(v, "outcome")?,
                txn_id: bytes_field(v, "txn_id")?,
                receipt_digest: Hash256::from_canon(v.field("receipt_digest")?)?,
                result_digest: Hash256::from_canon(v.field("result_digest")?)?,
            },
            "UnknownReply" => HistoryEvent::UnknownReply {
                attempt: attempt(v)?,
                req: str_field(v, "req")?,
            },
            "ExpiredReply" => HistoryEvent::ExpiredReply {
                attempt: attempt(v)?,
                req: str_field(v, "req")?,
                kind: str_field(v, "expired")?,
            },
            "Decision" => HistoryEvent::Decision {
                req: str_field(v, "req")?,
                txn_id: bytes_field(v, "txn_id")?,
                outcome: str_field(v, "outcome")?,
                decision_ref: Hash256::from_canon(v.field("decision_ref")?)?,
            },
            "Crash" => HistoryEvent::Crash {
                fault_point: str_field(v, "fault_point")?,
                nth: v.field("nth")?.as_u64()? as u32,
            },
            "Restart" => HistoryEvent::Restart {
                home_epoch: v.field("home_epoch")?.as_u64()?,
            },
            "Checkpoint" => HistoryEvent::Checkpoint,
            "Resolve" => HistoryEvent::Resolve {
                req: str_field(v, "req")?,
                kind: str_field(v, "resolution")?,
                txn_id: bytes_field(v, "txn_id")?,
                receipt_digest: opt_hash(v, "receipt_digest")?,
                result_digest: opt_hash(v, "result_digest")?,
            },
            "ResultEvicted" => HistoryEvent::ResultEvicted {
                req: str_field(v, "req")?,
            },
            "NamespaceRetired" => HistoryEvent::NamespaceRetired {
                namespace: bytes_field(v, "namespace")?,
            },
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("history event kind {k}"),
                ))
            }
        };
        Ok(Event { step, event })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    pub events: Vec<Event>,
}

impl History {
    pub fn push(&mut self, event: HistoryEvent) -> u64 {
        let step = self.events.len() as u64 + 1;
        self.events.push(Event { step, event });
        step
    }
    /// One canonical event per line.
    pub fn to_jsonl(&self) -> String {
        let mut s = String::new();
        for e in &self.events {
            s.push_str(&String::from_utf8_lossy(&e.encode()));
            s.push('\n');
        }
        s
    }
    pub fn from_jsonl(text: &str) -> CoreResult<History> {
        let limits = carolina_core::limits::Limits::v1();
        let mut events = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            events.push(Event::decode(line.as_bytes(), &limits)?);
        }
        Ok(History { events })
    }
    /// `SHA-256("astra.trace.v1" || 0x00 || jsonl)`: identical seed/schedule ⇒ identical digest.
    pub fn trace_digest(&self) -> Hash256 {
        domain_hash("astra.trace.v1", self.to_jsonl().as_bytes())
    }
    pub fn len(&self) -> usize {
        self.events.len()
    }
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// Digest of exact result bytes as recorded in replies (`SHA-256(bytes)`).
pub fn result_digest_of(bytes: &[u8]) -> Hash256 {
    sha256(bytes)
}
