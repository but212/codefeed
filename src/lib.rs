//! codefeed library core and shell modules.

pub(crate) mod core;
pub(crate) mod shell;

pub use core::cli::{CliAction, help_text, parse};
pub use core::error::{CandidateLimitExceededError, IngestError};
pub use core::model::{DigestFormat, IngestReport, OutputTarget, RunConfig, Source};
pub use shell::orchestrate::ingest;
