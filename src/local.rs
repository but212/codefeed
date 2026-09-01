use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::Path;

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

use ignore::WalkBuilder;

use crate::digest::{DigestBudget, FileRecord};
use crate::output::output_identity;
use crate::{text_content, CandidateBudget, CollectionResult, IngestError, MAX_FILE_SIZE};

pub(crate) enum LocalRead {
    Text(String),
    Skip,
}

pub(crate) fn read_local_file(root: &Path, path: &Path) -> Result<LocalRead, IngestError> {
    let file = match open_local_file(root, path) {
        Ok(file) => file,
        Err(error) if is_symlink_open_error(&error) => return Ok(LocalRead::Skip),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_SIZE {
        return Ok(LocalRead::Skip);
    }

    let mut bytes = Vec::new();
    file.take(MAX_FILE_SIZE.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_SIZE {
        return Ok(LocalRead::Skip);
    }
    Ok(match text_content(bytes) {
        Some(content) => LocalRead::Text(content),
        None => LocalRead::Skip,
    })
}

#[cfg(unix)]
fn open_local_file(root: &Path, path: &Path) -> io::Result<fs::File> {
    let relative = path
        .strip_prefix(root)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let mut directory = open_directory(root)?;
    let mut components = relative.components().peekable();
    let Some(first) = components.next() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot open the repository root as a file",
        ));
    };
    let mut component = first;
    loop {
        let name = CString::new(component.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
        let is_file = components.peek().is_none();
        let flags = if is_file {
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK
        } else {
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW
        };
        let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if descriptor == -1 {
            return Err(io::Error::last_os_error());
        }
        let opened = unsafe { OwnedFd::from_raw_fd(descriptor) };
        if is_file {
            return Ok(fs::File::from(opened));
        }
        directory = opened;
        component = components.next().expect("peek confirmed another component");
    }
}

#[cfg(unix)]
fn open_directory(path: &Path) -> io::Result<OwnedFd> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW);
    Ok(options.open(path)?.into())
}

#[cfg(not(unix))]
fn open_local_file(_: &Path, path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    options.open(path)
}

#[cfg(unix)]
fn is_symlink_open_error(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ELOOP)
}

#[cfg(not(unix))]
fn is_symlink_open_error(_: &io::Error) -> bool {
    false
}

pub(crate) fn collect_local(root: &Path, output: &Path) -> Result<CollectionResult, IngestError> {
    let root = fs::canonicalize(root)
        .map_err(|error| IngestError::InvalidSource(format!("{} ({error})", root.display())))?;
    if !root.is_dir() {
        return Err(IngestError::InvalidSource(format!(
            "{} is not a directory",
            root.display()
        )));
    }
    let output = output_identity(output)?;
    let label = root.display().to_string();
    let mut files = Vec::new();
    let mut budget = DigestBudget::new(&label, None)?;
    let mut candidates = CandidateBudget::default();

    let walker = WalkBuilder::new(&root)
        .hidden(true)
        .ignore(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .parents(false)
        .follow_links(false)
        .build();

    for entry in walker {
        let entry = entry.map_err(|error| IngestError::Io(io::Error::other(error)))?;
        let path = entry.path();
        if path == root || path == output || !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        candidates.add()?;
        let relative = path
            .strip_prefix(&root)
            .map_err(|error| IngestError::InvalidMetadata(error.to_string()))?;
        let Some(parts) = relative
            .components()
            .map(|part| part.as_os_str().to_str())
            .collect::<Option<Vec<_>>>()
        else {
            budget.skip()?;
            continue;
        };
        let relative = parts.join("/");
        let content = match read_local_file(&root, path)? {
            LocalRead::Text(content) => content,
            LocalRead::Skip => {
                budget.skip()?;
                continue;
            }
        };
        let candidate = FileRecord {
            path: relative,
            content,
        };
        budget.add(&candidate)?;
        files.push(candidate);
    }

    Ok(CollectionResult::Local {
        label,
        files,
        skipped: budget.skipped(),
    })
}
