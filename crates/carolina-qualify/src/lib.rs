//! CarolinaDB qualification system (SPEC-010).
//!
//! * [`manifest`] — immutable `QualificationManifest` (SPEC-010 §2)
//! * [`verdict`] — `PASS/FAIL/INCONCLUSIVE/NOT_RUN/NOT_APPLICABLE`, gates Q0–Q7/QI (§2, §17)
//! * [`history`] — observable history events and trace digests (§4)
//! * [`w1`] — independent W1 inventory oracle (§4, §11); hand-written, no DSL interpreter import
//! * [`checker`] — history checker rules Q-C01…Q-C14 for the local profile (§5)
//! * [`local`] — deterministic local schedules over the runtime engine with crash injection (§6, §7)
//! * [`minimize`] — schedule minimizer (§15)
//! * [`bundle`] — failure/evidence bundles (§15)
//! * [`runner`] — campaign composition, gate derivation, exit codes (§17, §18)
//!
//! Every verdict is computed from executed checks. A check that did not run is `NOT_RUN`; a
//! disabled capability is `NOT_APPLICABLE` with its justification recorded; a checker budget
//! exhaustion is `INCONCLUSIVE`. Nothing here turns a Markdown acceptance table into evidence.

pub mod bundle;
pub mod checker;
pub mod codec_corpus;
pub mod history;
pub mod local;
pub mod manifest;
pub mod minimize;
pub mod runner;
pub mod verdict;
pub mod w1;

pub use manifest::QualificationManifest;
pub use runner::{run_campaign, CampaignConfig, CampaignReport, ConfigError};
pub use verdict::{CheckResult, Status, Verdict};
