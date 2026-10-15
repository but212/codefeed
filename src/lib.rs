//! codefeed library core and shell modules.

pub(crate) mod core;
pub(crate) mod shell;

pub use core::cli::{CliAction, help_text, parse_cli};
pub use core::error::IngestError;
pub use core::model::{DigestFormat, IngestReport, OutputTarget, RunConfig, Source};
pub use shell::orchestrate::ingest;

#[doc(hidden)]
pub mod __bench {
    pub use crate::core::decide::{CandidateBudget, Item, ItemPath, Probe, Selector, text_str};
    pub use crate::core::digest::{
        BudgetTrie, escape_header, estimate_tokens, render_to_writer, unescape_header,
        write_escaped, xml_escape,
    };
    pub use crate::core::model::{
        CollectionResult, DigestFormat, FileRecord, FileStorage, OutputTarget,
    };
    pub use crate::shell::local::LocalStream;
    pub use crate::shell::orchestrate::{SourceStream, collect_stream};
}
