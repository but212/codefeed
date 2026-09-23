//! Imperative shell: process, filesystem, network, and orchestration.
//!
//! Shell modules perform all effects and feed observations to the functional
//! core in `crate::core`.

pub(crate) mod cli;
pub(crate) mod github;
pub(crate) mod github_client;
pub(crate) mod github_ignore;
pub(crate) mod local;
pub(crate) mod local_io;
pub(crate) mod output;
