//! CarolinaDB restricted DSL, typed IR, deterministic normalization, canonical
//! artifacts and reference interpreter (SPEC-003).
//!
//! Pipeline: [`parser`] → [`ast`] → [`lower`] (resolve, type-check, lower) → [`ir`]
//! (canonical encoding and hashes) → [`interp`] (slow sequential reference semantics).

pub mod ast;
pub mod counterexample;
pub mod fixtures;
pub mod interp;
pub mod ir;
pub mod lexer;
pub mod lower;
pub mod parser;
pub mod types;

#[cfg(test)]
mod spec003_tests;
