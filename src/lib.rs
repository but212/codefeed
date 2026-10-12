//! codefeed library core and shell modules.

pub(crate) mod core;
pub(crate) mod shell;

pub use core::model::OutputTarget;
pub use shell::cli::{CliAction, help_text, parse_cli};
pub use shell::orchestrate::ingest;

#[doc(hidden)]
pub mod __bench {
    pub use crate::core::decide::{text_content, Item, ItemPath, Probe, Selector};
    pub use crate::core::digest::{
        escaped_metrics, render_digest_to_writer, write_xml_escaped, xml_escape, BudgetTrie,
        TokenEstimatingWriter,
    };
    pub use crate::core::model::{CollectionResult, DigestFormat, FileRecord, OutputTarget};
    pub use crate::shell::local::collect_local;
}
