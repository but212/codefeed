//! Single source of truth for ingestion policies, limits, and conventions.

mod exclude;
mod exclusions;
mod limits;

pub use exclude::ExcludePatterns;
pub(crate) use exclude::push_match_segment;
pub(crate) use exclusions::*;
pub(crate) use limits::*;

/// Environment variables holding the GitHub token, in lookup order. The first non-empty value wins.
pub(crate) const TOKEN_ENV_VARS: [&str; 2] = ["GITHUB_TOKEN", "GH_TOKEN"];
