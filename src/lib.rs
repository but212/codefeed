//! codefeed library core and shell modules.

pub(crate) mod core;
pub(crate) mod shell;

pub use core::model::{OutputTarget, RunConfig};
pub use shell::cli::{CliAction, help_text, parse_cli};
pub use shell::orchestrate::ingest;

#[doc(hidden)]
pub mod __bench {
    pub use crate::core::decide::{Item, ItemPath, Probe, Selector, text_content};
    pub use crate::core::digest::{
        BudgetTrie, TokenEstimatingWriter, escaped_metrics, render_digest_to_writer,
        write_xml_escaped, xml_escape,
    };
    pub use crate::core::model::{CollectionResult, DigestFormat, FileRecord, OutputTarget};
    pub use crate::shell::local::collect_local;
}
