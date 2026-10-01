use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::error::IngestError;
use crate::core::model::OutputTarget;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn output_identity(output: &Path) -> Result<PathBuf, IngestError> {
    let absolute = if output.is_absolute() {
        output.to_path_buf()
    } else {
        env::current_dir()?.join(output)
    };
    let file_name = absolute.file_name().unwrap_or_default().to_os_string();
    let mut parent = absolute
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut unresolved = Vec::new();
    let canonical_parent = loop {
        match fs::canonicalize(&parent) {
            Ok(parent) => break parent,
            Err(_) => {
                let Some(component) = parent.file_name() else {
                    break parent;
                };
                unresolved.push(component.to_os_string());
                parent = parent
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf();
            }
        }
    };

    let mut resolved = canonical_parent;
    for component in unresolved.iter().rev() {
        resolved.push(component);
    }
    resolved.push(file_name);
    Ok(normalize_path(&resolved))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

/// Delivers the rendered digest to its target. A file target is written
/// atomically; the stdout target writes the buffer to the stream directly,
/// bypassing temporary-file creation and filesystem sync.
///
/// A broken pipe on the stdout target is a normal early close by the consumer
/// (`rustingest . --output - | head -1`), not an I/O failure: the consumer took
/// what it asked for, and stdout emission leaves no partial state behind.
pub(crate) fn emit(target: &OutputTarget, contents: &str) -> Result<(), IngestError> {
    match target {
        OutputTarget::File(path) => write_atomic(path, contents.as_bytes()),
        OutputTarget::Stdout => write_stdout(contents.as_bytes()),
    }
}

fn write_stdout(contents: &[u8]) -> Result<(), IngestError> {
    let mut stdout = io::stdout().lock();
    match stdout.write_all(contents).and_then(|()| stdout.flush()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn write_atomic(output: &Path, contents: &[u8]) -> Result<(), IngestError> {
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = output
        .file_name()
        .ok_or_else(|| IngestError::InvalidMetadata("output path has no filename".into()))?
        .to_string_lossy();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let (temporary, mut file) = loop {
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".{file_name}.{timestamp}.{counter}.tmp"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(IngestError::Io(error)),
        }
    };
    let result = (|| {
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&temporary, output)?;
        Ok::<(), io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(IngestError::Io)
}
