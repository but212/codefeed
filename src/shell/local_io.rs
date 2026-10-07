//! Safe local file acquisition: never follows symlinks, never blocks on
//! special files.

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

use crate::core::decide::Probe;
use crate::core::error::IngestError;
use crate::core::policy::MAX_FILE_SIZE;

pub(crate) fn read_local_file(root: &Path, path: &Path) -> Result<Probe, IngestError> {
    let file = match open_local_file(root, path) {
        Ok(file) => file,
        Err(error) if is_symlink_open_error(&error) => return Ok(Probe::Missing),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Ok(Probe::Missing);
    }
    if metadata.len() > MAX_FILE_SIZE {
        return Ok(Probe::TooLarge);
    }

    let mut bytes = Vec::new();
    file.take(MAX_FILE_SIZE.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_SIZE {
        return Ok(Probe::TooLarge);
    }
    Ok(Probe::Bytes(bytes))
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

#[cfg(test)]
mod tests {
    use super::super::temp_dir;
    use super::*;

    #[cfg(unix)]
    #[test]
    fn does_not_follow_local_symlink() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let root = temp_dir("symlink")?;
        fs::create_dir_all(&root)?;
        let target = root.join("outside.txt");
        let link = root.join("link.txt");
        fs::write(&target, "outside")?;
        symlink(&target, &link)?;

        assert!(matches!(read_local_file(&root, &link)?, Probe::Missing));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn does_not_block_when_candidate_becomes_fifo() -> Result<(), Box<dyn std::error::Error>> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        let root = temp_dir("fifo")?;
        fs::create_dir_all(&root)?;
        let path = root.join("candidate");
        let c_path = CString::new(path.as_os_str().as_bytes())?;
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        let (sender, receiver) = mpsc::channel();
        let thread_path = path.clone();
        let thread_root = root.clone();
        let worker = thread::spawn(move || {
            let result = read_local_file(&thread_root, &thread_path);
            sender.send(result).expect("result receiver should exist");
        });
        let result = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("FIFO candidate must not block local ingestion")?;

        assert!(matches!(result, Probe::Missing));
        worker.join().expect("file reader thread should finish");
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn bounds_local_file_reads() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("bounded_read")?;
        fs::create_dir_all(&root)?;
        let path = root.join("large.txt");
        fs::write(&path, vec![b'x'; MAX_FILE_SIZE as usize + 1])?;

        assert!(matches!(read_local_file(&root, &path)?, Probe::TooLarge));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn includes_file_at_exact_size_limit() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("exact_size")?;
        fs::create_dir_all(&root)?;
        let path = root.join("exact.txt");
        fs::write(&path, vec![b'x'; MAX_FILE_SIZE as usize])?;

        assert!(matches!(read_local_file(&root, &path)?, Probe::Bytes(_)));
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
