//! codefeed library core and shell modules.

pub(crate) mod core;
pub(crate) mod shell;

pub use core::model::OutputTarget;
pub use shell::cli::{CliAction, help_text, parse_cli};
pub use shell::orchestrate::ingest;

#[doc(hidden)]
pub mod __bench {
    pub use crate::core::decide::{Item, ItemPath, Probe, Selector};
    pub use crate::core::digest::{render_digest_to_writer, BudgetTrie, TokenEstimatingWriter};
    pub use crate::core::model::{CollectionResult, DigestFormat, FileRecord};
}
