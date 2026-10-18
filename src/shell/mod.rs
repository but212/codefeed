//! Imperative shell: process, filesystem, network, and orchestration.
//!
//! Shell modules perform all effects and feed observations to the functional
//! core in `crate::core`.

use crate::{
    IngestError, IngestReport, OutputTarget, RunConfig, Source,
    core::cli::{CliAction, help_text, parse},
};

mod github;
mod local;
mod output;

/// Executes the end-to-end repository ingestion pipeline.
///
/// Dispatches collection across local filesystem or GitHub remote sources, renders
/// the digest into the target format (plain text or XML), and atomically emits the
/// result to standard output or a destination file.
///
/// # Errors
///
/// Returns [`IngestError`] if input sources are invalid, candidate budgets or output size
/// limits are exceeded, filesystem operations fail, or network requests error.
pub fn ingest(config: &RunConfig) -> Result<IngestReport, IngestError> {
    let resolved_output = output::resolve_output_target(&config.output)?;

    let collection = match &config.source {
        Source::Local(path) => local::collect(path, resolved_output.as_deref())?,
        Source::Github {
            owner,
            repo,
            reference,
        } => github::collect(owner, repo, reference.as_deref())?,
    };

    let estimated_tokens = output::emit(resolved_output.as_deref(), &collection, config.format)?;

    Ok(IngestReport {
        output: config.output.clone(),
        included_files: collection.files.len(),
        skipped_files: collection.skipped,
        estimated_tokens,
    })
}

/// Executes the command-line interface entry point.
///
/// Parses arguments, renders help text or performs ingestion, formats user-facing
/// status messages, and returns the appropriate process exit code (`0` on success, `1` on error).
pub fn run_cli<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString>,
{
    let action = match parse(args) {
        Ok(action) => action,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };

    match action {
        CliAction::Help => {
            write_stdout_line(help_text());
            0
        }
        CliAction::Run(config) => match ingest(&config) {
            Ok(report) => {
                let included = report.included_files;
                let skipped = report.skipped_files;
                let tokens = report.estimated_tokens;
                match &report.output {
                    OutputTarget::File(path) => {
                        let path_display = path.display();
                        write_stdout_line(&format!(
                            "Wrote {path_display} ({included} files, {skipped} skipped, ~{tokens} tokens)"
                        ));
                    }
                    OutputTarget::Stdout => {
                        eprintln!(
                            "Wrote to stdout ({included} files, {skipped} skipped, ~{tokens} tokens)"
                        );
                    }
                }
                0
            }
            Err(error) => {
                eprintln!("error: {error}");
                1
            }
        },
    }
}

fn write_stdout_line_to<W: std::io::Write>(out: &mut W, line: &str) -> std::io::Result<()> {
    match writeln!(out, "{line}") {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(e),
    }
}

fn write_stdout_line(line: &str) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = write_stdout_line_to(&mut lock, line);
}

/// Returns the elapsed duration since the Unix epoch in nanoseconds.
fn current_epoch_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
#[derive(Debug)]
struct TempDirGuard(std::path::PathBuf);

#[cfg(test)]
impl TempDirGuard {
    fn new(label: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let nanos = current_epoch_nanos();
        let path = std::env::temp_dir().join(format!("local_ingest_{label}_{nanos}"));
        std::fs::create_dir_all(&path)?;
        Ok(Self(std::fs::canonicalize(&path)?))
    }
}

#[cfg(test)]
impl std::ops::Deref for TempDirGuard {
    type Target = std::path::Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
impl AsRef<std::path::Path> for TempDirGuard {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}

#[cfg(test)]
impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
fn temp_dir(label: &str) -> Result<TempDirGuard, Box<dyn std::error::Error>> {
    TempDirGuard::new(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_dir_guard_cleans_up_on_drop_and_panic() -> Result<(), Box<dyn std::error::Error>> {
        // 1. Drop cleanup
        let drop_path = {
            let guard = temp_dir("test_guard_drop")?;
            assert!(guard.is_dir());
            guard.to_path_buf()
        };
        assert!(
            !drop_path.exists(),
            "directory should be removed on guard drop"
        );

        // 2. Panic cleanup
        let mut panic_path = std::path::PathBuf::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let guard = temp_dir("test_guard_panic").unwrap();
            panic_path = guard.to_path_buf();
            assert!(guard.is_dir());
            panic!("intentional panic inside test");
        }));
        assert!(result.is_err());
        assert!(
            !panic_path.exists(),
            "directory should be removed even on panic"
        );
        Ok(())
    }

    #[test]
    fn write_stdout_line_ignores_broken_pipe() {
        struct BrokenPipeWriter;
        impl std::io::Write for BrokenPipeWriter {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "broken pipe",
                ))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "broken pipe",
                ))
            }
        }
        let mut writer = BrokenPipeWriter;
        assert!(write_stdout_line_to(&mut writer, "hello").is_ok());
    }
}
