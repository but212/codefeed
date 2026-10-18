#![warn(missing_docs)]

//! Fast, memory-efficient repository ingestion and prompt generation for LLMs.
//!
//! `codefeed` converts local source directories or remote GitHub repositories into
//! clean, LLM-optimized prompt digests in plain text or XML format.
//!
//! Built on the Functional Core, Imperative Shell (FCIS) pattern:
//! - Pure logic in `core`, handling policy limits, candidate filtering, ASCII tree rendering,
//!   and formatting without side effects.
//! - Effects in `shell`, executing file walking, HTTP tarball streaming, and atomic writes.
//!
//! # Primary Entry Point
//!
//! Ingestion is orchestrated via [`ingest`]:
//!
//! ```no_run
//! use std::path::PathBuf;
//! use codefeed::{ingest, DigestFormat, OutputTarget, RunConfig, Source};
//!
//! let config = RunConfig::new(
//!     Source::Local(PathBuf::from(".")),
//!     OutputTarget::Stdout,
//!     DigestFormat::Text,
//! );
//!
//! let report = ingest(&config).expect("ingestion succeeded");
//! println!("Processed {} files (~{} tokens)", report.included_files, report.estimated_tokens);
//! ```

pub(crate) mod core;
pub(crate) mod shell;

pub use core::cli::{CliAction, help_text, parse};
pub use core::error::{CandidateLimitExceededError, IngestError};
pub use core::model::{DigestFormat, IngestReport, OutputTarget, RunConfig, Source};
pub use shell::ingest;
