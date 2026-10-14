//! codefeed library core and shell modules.

pub(crate) mod core;
pub(crate) mod shell;

pub use core::cli::{CliAction, help_text, parse_cli};
pub use core::model::{OutputTarget, RunConfig};
pub use shell::orchestrate::ingest;

#[doc(hidden)]
pub mod __bench {
    pub use crate::core::decide::{CandidateBudget, Item, ItemPath, Probe, Selector, text_content};
    pub use crate::core::digest::{
        BudgetTrie, escape_header, estimate_tokens, render_digest_to_writer, write_xml_escaped,
        xml_escape,
    };
    pub use crate::core::model::{
        CollectionResult, DigestFormat, FileRecord, FileStorage, OutputTarget,
    };
    pub use crate::shell::local::LocalStream;
    pub use crate::shell::orchestrate::{SourceStream, collect_stream};
}
