#![warn(missing_docs)]

//! Ingest local directories or remote GitHub repositories into a single LLM-friendly text digest.
//!
//! # Overview
//!
//! `rustingest` traverses a source tree, excludes noise files (lockfiles, `.git`, binary extensions),
//! enforces size limits (1 MiB per file, 20 MiB per digest), and outputs an LLM-ready text digest
//! either to a file atomically or directly to standard output.
//!
//! # Quickstart
//!
//! ```no_run
//! use std::path::PathBuf;
//! use rustingest::{parse_source, ingest, Options, OutputTarget};
//!
//! # fn main() -> Result<(), rustingest::IngestError> {
//! let source = parse_source("./my-project")?;
//! let options = Options {
//!     output: OutputTarget::File(PathBuf::from("digest.txt")),
//!     reference: None,
//! };
//! let report = ingest(&source, &options)?;
//! println!("Included {} files, skipped {}", report.included_files, report.skipped_files);
//! # Ok(())
//! # }
//! ```

mod core;
mod shell;

use std::ffi::{OsStr, OsString};

pub use crate::core::error::IngestError;
pub use crate::core::model::{IngestReport, Options, OutputTarget, Source};

/// Parses a source string into a [`Source::Local`] directory or [`Source::GitHub`] repository.
///
/// # Examples
///
/// ```
/// use rustingest::{parse_source, Source};
///
/// let local = parse_source("./src").unwrap();
/// assert!(matches!(local, Source::Local(_)));
///
/// let github = parse_source("https://github.com/rust-lang/cargo").unwrap();
/// assert_eq!(
///     github,
///     Source::GitHub {
///         owner: "rust-lang".into(),
///         repo: "cargo".into(),
///     }
/// );
/// ```
///
/// # Errors
///
/// Returns [`IngestError::UnsupportedSource`] if the URL scheme is not HTTPS or the host is not `github.com`.
/// Returns [`IngestError::InvalidSource`] if the path format or owner/repository characters are invalid.
pub fn parse_source(value: &str) -> Result<Source, IngestError> {
    core::source::parse_source(value)
}

pub(crate) fn parse_source_os(value: &OsStr) -> Result<Source, IngestError> {
    core::source::parse_source_os(value)
}

/// Ingests candidate files from `source` and emits the rendered digest to `options.output`.
///
/// Local directories are walked without following symlinks and filtered using `.gitignore`.
/// GitHub repositories are fetched via remote API trees and downloaded concurrently in bounded batches.
///
/// # Examples
///
/// ```no_run
/// use rustingest::{parse_source, ingest, Options};
///
/// # fn main() -> Result<(), rustingest::IngestError> {
/// let source = parse_source("./")?;
/// let options = Options::default();
/// let report = ingest(&source, &options)?;
/// println!("Included {} files", report.included_files);
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns [`IngestError::InvalidSource`] if `--ref` is provided for a local source or is empty.
/// Returns [`IngestError::OutputTooLarge`] if the rendered digest exceeds the 20 MiB limit.
/// Returns [`IngestError::Io`] or [`IngestError::Http`] if filesystem or remote network operations fail.
pub fn ingest(source: &Source, options: &Options) -> Result<IngestReport, IngestError> {
    if options
        .reference
        .as_ref()
        .is_some_and(|reference| reference.trim().is_empty())
    {
        return Err(IngestError::InvalidSource("--ref cannot be empty".into()));
    }
    if matches!(source, Source::Local(_)) && options.reference.is_some() {
        return Err(IngestError::InvalidSource(
            "--ref is only valid for GitHub sources".into(),
        ));
    }

    let collection = match source {
        Source::Local(root) => shell::local::collect_local(root, &options.output)?,
        Source::GitHub { owner, repo } => {
            shell::github::collect_github(owner, repo, options.reference.as_deref())?
        }
    };

    let (label, revision, files, skipped) = match collection {
        core::model::CollectionResult::Local {
            label,
            files,
            skipped,
        } => (label, None, files, skipped),
        core::model::CollectionResult::GitHub {
            label,
            revision,
            files,
            skipped,
        } => (label, Some(revision), files, skipped),
    };

    shell::output::emit(&options.output, |writer| {
        core::digest::render_digest_to_writer(writer, &label, revision.as_deref(), &files, skipped)
    })?;

    Ok(IngestReport {
        output: options.output.clone(),
        included_files: files.len(),
        skipped_files: skipped,
    })
}

/// Returns the standard CLI usage help string.
///
/// # Examples
///
/// ```
/// let help = rustingest::help_text();
/// assert!(help.starts_with("Usage: rustingest"));
/// ```
pub fn help_text() -> &'static str {
    shell::cli::help_text()
}

/// Checks whether CLI arguments contain a help request (`--help` or `-h`).
///
/// Skips values following options that consume arguments (`--output`, `--ref`).
///
/// # Examples
///
/// ```
/// use std::ffi::OsString;
/// use rustingest::requests_help;
///
/// let args = vec![OsString::from("rustingest"), OsString::from("--help")];
/// assert!(requests_help(&args));
///
/// let args = vec![OsString::from("rustingest"), OsString::from("--output"), OsString::from("--help")];
/// assert!(!requests_help(&args));
/// ```
pub fn requests_help(args: &[OsString]) -> bool {
    shell::cli::requests_help(args)
}

/// Runs the command-line interface entry point with the provided argument sequence.
///
/// The first argument is treated as the executable name and skipped.
///
/// # Errors
///
/// Returns [`IngestError::InvalidSource`] if required arguments are missing or invalid options are encountered.
pub fn run_cli<I, T>(args: I) -> Result<IngestReport, IngestError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    shell::cli::run_cli(args)
}
