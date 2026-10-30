//! Single source of truth for ingestion policies, limits, and conventions.

mod classify;
mod exclude;
mod exclusions;
mod limits;

pub(crate) use classify::*;
pub use exclude::ExcludePatterns;
pub(crate) use exclusions::*;
pub(crate) use limits::*;
