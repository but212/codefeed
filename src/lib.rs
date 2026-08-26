mod cli;
mod digest;
mod github;
mod local;
mod output;
mod source;

#[cfg(test)]
#[path = "../tests/unit/mod.rs"]
mod tests;

use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::path::PathBuf;

pub(crate) const MAX_FILE_SIZE: u64 = 1024 * 1024;
pub(crate) const MAX_OUTPUT_SIZE: u64 = 20 * 1024 * 1024;
pub(crate) const MAX_FILE_COUNT: usize = 10_000;
pub(crate) const MAX_JSON_SIZE: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_REMOTE_IGNORE_FILES: usize = 256;
pub(crate) const MAX_REMOTE_IGNORE_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const REMOTE_ROOT: &str = "/__local_ingest_repo";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Local(PathBuf),
    GitHub { owner: String, repo: String },
}

#[derive(Debug, Clone)]
pub struct Options {
    pub output: PathBuf,
    pub reference: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            output: PathBuf::from("digest.txt"),
            reference: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestReport {
    pub output: PathBuf,
    pub included_files: usize,
    pub skipped_files: usize,
}

#[derive(Debug)]
pub enum IngestError {
    InvalidSource(String),
    UnsupportedSource(String),
    Io(io::Error),
    Http(String),
    Api(String),
    InvalidMetadata(String),
    OutputTooLarge,
}

impl fmt::Display for IngestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSource(message) => write!(f, "invalid source: {message}"),
            Self::UnsupportedSource(message) => write!(f, "unsupported source: {message}"),
            Self::Io(error) => write!(f, "filesystem error: {error}"),
            Self::Http(message) => write!(f, "GitHub request failed: {message}"),
            Self::Api(message) => write!(f, "GitHub API error: {message}"),
            Self::InvalidMetadata(message) => write!(f, "invalid repository metadata: {message}"),
            Self::OutputTooLarge => write!(f, "digest exceeds the 20 MiB output limit"),
        }
    }
}

impl std::error::Error for IngestError {}

impl From<io::Error> for IngestError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn parse_source(value: &str) -> Result<Source, IngestError> {
    source::parse_source(value)
}

pub(crate) fn parse_source_os(value: &OsStr) -> Result<Source, IngestError> {
    source::parse_source_os(value)
}

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

    let (label, revision, mut files, skipped) = match source {
        Source::Local(root) => local::collect_local(root, &options.output)?,
        Source::GitHub { owner, repo } => github::collect_github(owner, repo, options)?,
    };

    files.sort_by(|left, right| left.path.cmp(&right.path));
    let digest = digest::render_digest(&label, revision.as_deref(), &files, skipped)?;
    output::write_atomic(&options.output, digest.as_bytes())?;

    Ok(IngestReport {
        output: options.output.clone(),
        included_files: files.len(),
        skipped_files: skipped,
    })
}

pub fn help_text() -> &'static str {
    cli::help_text()
}

pub fn run_cli<I, T>(args: I) -> Result<IngestReport, IngestError>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString>,
{
    cli::run_cli(args)
}

pub(crate) fn text_content(bytes: Vec<u8>) -> Option<String> {
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}
