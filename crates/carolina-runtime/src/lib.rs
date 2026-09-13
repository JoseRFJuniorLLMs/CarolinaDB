//! CarolinaDB runtime — the local vertical slice of SPEC-014 §3 (MVP-2) and §4.
//!
//! ```text
//! declared operation -> typed IR -> closed obligations -> qualified plan       (carolina-lang/compiler)
//! -> authenticated RequestKey -> home durable mapping and plan assignment      (home.rs)
//! -> ordered invariant/effect/result evaluation                                (carolina-lang interp)
//! -> atomic CompiledBatch -> local decision durability -> persisted receipt    (engine.rs + storage)
//! -> lost response -> ResolveRequest -> identical receipt after restart        (engine.rs)
//! ```
//!
//! Scope limits (SPEC-014 §3, MVP-2 row): one process, one data directory, one RequestHome with an
//! explicitly local grant, C0/C5 plans whose atomicity is one local batch. Nothing here claims
//! replication, multi-IDC atomicity or distributed durability; those are later stages.

pub mod catalog;
pub mod engine;
pub mod home;
pub mod state;

pub use catalog::LocalCatalog;
pub use engine::{EngineOptions, LocalEngine};
pub use home::LocalRequestHome;
