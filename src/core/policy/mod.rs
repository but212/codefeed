//! Single source of truth for ingestion policies, limits, and conventions.

mod auth;
mod exclude;
mod exclusions;
mod limits;

pub(crate) use auth::*;
pub use exclude::ExcludePatterns;
pub(crate) use exclude::push_match_segment;
pub(crate) use exclusions::*;
pub(crate) use limits::*;
