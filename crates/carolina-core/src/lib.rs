//! CarolinaDB core types.
//!
//! This crate has no file or network I/O. It owns:
//!
//! * the nominal identity/generation/epoch taxonomy of SPEC-011 §2 ([`ids`]);
//! * composite identities `RequestKey`, `TxnId`, `IdcBinding`, `PlanRef`,
//!   `OriginId`, `AuthorityBinding`, `ProtocolRecordRef` ([`ids`]);
//! * the restricted canonical JSON encoding of SPEC-003 §10 / SPEC-012 §8 ([`canon`]);
//! * domain-separated SHA-256 hashing ([`hash`]);
//! * checked fixed-scale decimals of SPEC-003 §4 ([`decimal`]);
//! * the order-preserving physical key codec of SPEC-002 §11 ([`keycodec`]);
//! * stable error codes ([`error`]) and versioned resource limits ([`limits`]).
//!
//! Equal integer payloads of two different epoch types are never interchangeable:
//! every scalar identity is a distinct newtype and there is no `From<u64>` between them.

pub mod canon;
pub mod decimal;
pub mod error;
pub mod hash;
pub mod ids;
pub mod keycodec;
pub mod limits;
pub mod rng;

pub use canon::{CanonValue, Canonical};
pub use decimal::Decimal;
pub use error::{CoreError, ErrorCode};
pub use hash::{domain_hash, Hash256};
pub use ids::*;
pub use limits::Limits;
