//! Single source of truth for ingestion policies, limits, and conventions.

mod classify;
mod exclusions;
mod limits;

pub(crate) use classify::*;
pub(crate) use exclusions::*;
pub(crate) use limits::*;
