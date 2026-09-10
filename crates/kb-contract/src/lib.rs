#![forbid(unsafe_code)]

//! Shared value and error boundaries for knowledge-source contracts.

pub mod error;

pub use error::ContractError;

/// Schema identifier carried by source-contract diagnostics and artifacts.
pub const SOURCE_CONTRACT_SCHEMA: &str = "source-contract-v2";
