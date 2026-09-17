//! Collection backends: local filesystem and public GitHub.

pub(crate) mod github;
pub(crate) mod github_client;
pub(crate) mod github_ignore;
pub(crate) mod github_tree;
pub(crate) mod github_url;
pub(crate) mod ignore;
pub(crate) mod local;
pub(crate) mod local_io;
