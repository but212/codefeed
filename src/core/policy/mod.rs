//! Single source of truth for ingestion policies, limits, and conventions.

mod exclude;
mod exclusions;
mod limits;

pub use exclude::ExcludePatterns;
pub(crate) use exclude::push_match_segment;
pub(crate) use exclusions::*;
pub(crate) use limits::*;
