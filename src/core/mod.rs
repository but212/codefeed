//! Functional core: pure domain logic with no I/O, clock, network, or entropy.
//!
//! Core modules never depend on `shell`; the imperative shell depends on the
//! core in one direction.

pub(crate) mod decide;
pub(crate) mod digest;
pub(crate) mod error;
pub(crate) mod github_tree;
pub(crate) mod github_url;
pub(crate) mod ignore;
pub(crate) mod limits;
pub(crate) mod model;
pub(crate) mod source;
