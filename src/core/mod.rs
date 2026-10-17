//! Functional core: pure domain logic with no I/O, clock, network, or entropy.
//!
//! Core modules never depend on `shell`; the imperative shell depends on the
//! core in one direction.

pub(crate) mod archive;
pub(crate) mod cli;
pub(crate) mod decide;
pub(crate) mod error;
pub(crate) mod github_url;
pub(crate) mod model;
pub(crate) mod policy;
pub(crate) mod render;
pub(crate) mod source;
pub(crate) mod tree;
