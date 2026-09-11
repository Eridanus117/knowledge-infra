#![forbid(unsafe_code)]

//! Source-plane boundaries for Git-backed Markdown knowledge.

pub mod adopt;
pub mod amend;
pub mod author;
pub mod capture;
pub mod check;
pub mod doctor;
pub mod frozen;
pub mod git;
pub mod human_index;
pub mod ledger;
pub mod links;
pub mod mermaid;
pub mod relocate;
pub mod source;

pub use check::{CheckReport, CoreError, check_source};
pub use human_index::{HumanIndexPlan, apply_human_index, plan_human_index};

pub use source::{
    DomainNode, NoteLocator, SnapshotNote, SourceContext, SourceSnapshot, discover_source,
};
