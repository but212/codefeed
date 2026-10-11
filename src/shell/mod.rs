//! Imperative shell: process, filesystem, network, and orchestration.
//!
//! Shell modules perform all effects and feed observations to the functional
//! core in `crate::core`.

pub(crate) mod cli;
pub(crate) mod github;
pub(crate) mod github_client;
pub(crate) mod local;
pub(crate) mod local_io;
pub(crate) mod orchestrate;
pub(crate) mod output;
pub(crate) mod output_exclusion;

#[cfg(test)]
pub(crate) fn temp_dir(label: &str) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    Ok(std::env::temp_dir().join(format!("local_ingest_{label}_{nanos}")))
}
