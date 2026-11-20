#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(unreachable_pub)]
#![warn(clippy::unwrap_used)]
#![warn(clippy::missing_errors_doc)]
#![warn(clippy::missing_panics_doc)]

//! Converts local directories or GitHub repositories into LLM prompt digests (text or XML).
//!
//! Built on Functional Core, Imperative Shell (FCIS):
//! - `core`: pure policy limits, filtering, ASCII tree rendering, and formatting.
//! - `shell`: file walking, HTTP tarball streaming, and atomic output writes.
//!
//! # Primary Entry Point
//!
//! Orchestrated via [`ingest`]:
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

pub use core::error::{DomainError, ErrorDetail, LimitExceededError};
pub use core::github::GithubApiError;
pub use core::model::{DigestFormat, IngestReport, OutputTarget, RunConfig};
pub use core::policy::ExcludePatterns;
pub use core::source::{GitRef, GithubName, Source};
pub use shell::{IngestError, ingest};

#[doc(hidden)]
pub mod cli {
    pub use crate::shell::cli::run_cli;
}
