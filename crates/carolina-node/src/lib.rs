//! CarolinaDB node (SPEC-014 §2/§3, MVP-3): three isolated processes and data directories, a
//! fixed three-voter consensus authority (`carolina-consensus`), the control-plane catalog
//! (`carolina-catalog`) and a single-IDC C5 ordered executor over the local durable slice
//! (`carolina-runtime` + `carolina-storage`).
//!
//! Ordered execution (SPEC-008 §8, single IDC): the leader admits a client `Invoke` by proposing
//! an `Admit` command; every voter applies the committed log in order and executes the same
//! deterministic engine step against the same replicated pre-state; the leader then proposes the
//! `Decision` carrying the exact receipt digest. The client reply is sent only when that decision
//! is committed, so a leader or client failure yields one final outcome resolvable by
//! `RequestKey`. Followers compare their own receipt digest with the decision and fail closed on
//! divergence (SPEC-008 §8 "follower checks").
//!
//! Security profile: `DEV_LOCAL` only (plaintext `ASTR` frames restricted to host loopback).
//! Endpoint roles are unauthenticated declarations; use only with trusted local processes.
//! The node never advertises `ENCRYPTED_HOST_V1`; SPEC-013 mTLS is not implemented and no
//! security qualification is claimed.

pub mod campaign;
pub mod client;
pub mod config;
pub mod core;
pub mod protocol;
pub mod transport;

pub use client::Client;
pub use config::NodeConfig;
pub use core::run_node;
