//! Safe local file acquisition: never follows symlinks, never blocks on
//! special files.

use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::Path;

#[cfg(unix)]
use super::sys;
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
    let Some(file) = open_local_file(root, path)? else {
        return Ok(Probe::Missing);
    };
    read_bounded_content(file).map_err(Into::into)
}

fn read_bounded_content(file: fs::File) -> io::Result<Probe> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Ok(Probe::Missing);
    }
    if metadata.len() > MAX_FILE_SIZE {
        return Ok(Probe::TooLarge);
    }

    let mut bytes = Vec::new();
    let mut handle = file.take(MAX_FILE_SIZE.saturating_add(1));
    handle.read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_SIZE {
        return Ok(Probe::TooLarge);
    }
    Ok(Probe::Bytes(bytes))
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

#[cfg(target_os = "linux")]
const RESOLVE_BENEATH: u64 = 0x08;
#[cfg(target_os = "linux")]
const RESOLVE_NO_SYMLINKS: u64 = 0x04;

#[cfg(unix)]
fn open_local_file(root: &Path, path: &Path) -> io::Result<Option<fs::File>> {
    let relative = path
        .strip_prefix(root)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let directory = match open_directory(root) {
        Ok(d) => d,
        Err(err) if is_missing_or_unsupported(&err) => return Ok(None),
        Err(err) => return Err(err),
    };

    #[cfg(target_os = "linux")]
    {
        static OPENAT2_UNAVAILABLE: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);

        if !OPENAT2_UNAVAILABLE.load(std::sync::atomic::Ordering::Relaxed)
            && let Ok(c_rel) = CString::new(relative.as_os_str().as_bytes())
        {
            let how = OpenHow {
                flags: (sys::O_RDONLY | sys::O_CLOEXEC | sys::O_NOFOLLOW | sys::O_NONBLOCK) as u64,
                mode: 0,
                resolve: RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS,
            };
            let fd = unsafe {
                sys::syscall(
                    sys::SYS_openat2,
                    directory.as_raw_fd(),
                    c_rel.as_ptr(),
                    &how as *const OpenHow,
                    std::mem::size_of::<OpenHow>(),
                )
            };
            if fd >= 0 {
                let opened = unsafe { OwnedFd::from_raw_fd(fd as sys::c_int) };
                return Ok(Some(fs::File::from(opened)));
            }
            let err = io::Error::last_os_error();
            if is_missing_or_unsupported(&err) {
                return Ok(None);
            }
            match err.raw_os_error() {
                Some(sys::ENOSYS) | Some(sys::EPERM) => {
                    OPENAT2_UNAVAILABLE.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                Some(sys::EINVAL) | Some(sys::EOPNOTSUPP) => {}
                _ => return Err(err),
            }
        }
    }

    match open_local_file_components(directory, relative) {
        Ok(file) => Ok(Some(file)),
        Err(err) if is_missing_or_unsupported(&err) => Ok(None),
        Err(err) => Err(err),
    }
}

#[cfg(unix)]
fn open_local_file_components(mut directory: OwnedFd, relative: &Path) -> io::Result<fs::File> {
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
            sys::O_RDONLY | sys::O_CLOEXEC | sys::O_NOFOLLOW | sys::O_NONBLOCK
        } else {
            sys::O_RDONLY | sys::O_CLOEXEC | sys::O_DIRECTORY | sys::O_NOFOLLOW
        };
        // SAFETY: `directory.as_raw_fd()` is an open, valid directory descriptor.
        // `name` is a valid null-terminated C string containing no intermediate slashes.
        let descriptor = unsafe { sys::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if descriptor == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `descriptor` was checked to be non-negative, representing a newly
        // opened, valid file descriptor whose unique ownership is transferred to `OwnedFd`.
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
        .custom_flags(sys::O_CLOEXEC | sys::O_DIRECTORY | sys::O_NOFOLLOW);
    Ok(options.open(path)?.into())
}

#[cfg(windows)]
fn open_local_file(root: &Path, path: &Path) -> io::Result<Option<fs::File>> {
    let mut options = OpenOptions::new();
    options.read(true);
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = match options.open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    if !verify_beneath_root(root, &file)? {
        return Ok(None);
    }
    Ok(Some(file))
}

#[cfg(windows)]
fn verify_beneath_root(root: &Path, file: &fs::File) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;

    let canonical_file = get_final_path_from_handle(file.as_raw_handle())?;
    if is_verbatim_path(root) {
        Ok(is_strictly_beneath(root, &canonical_file))
    } else {
        let canonical_root = fs::canonicalize(root)?;
        Ok(is_strictly_beneath(&canonical_root, &canonical_file))
    }
}

#[cfg(windows)]
fn is_verbatim_path(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    matches!(
        path.components().next(),
        Some(Component::Prefix(p))
            if matches!(
                p.kind(),
                Prefix::Verbatim(_) | Prefix::VerbatimDisk(_) | Prefix::VerbatimUNC(_, _)
            )
    )
}

#[cfg(windows)]
fn get_final_path_from_handle(
    handle: std::os::windows::io::RawHandle,
) -> io::Result<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    unsafe extern "system" {
        fn GetFinalPathNameByHandleW(
            h_file: *mut std::ffi::c_void,
            lpsz_file_path: *mut u16,
            cch_file_path: u32,
            dw_flags: u32,
        ) -> u32;
    }

    let mut buf = vec![0u16; 1024];
    loop {
        let res = unsafe {
            GetFinalPathNameByHandleW(
                handle,
                buf.as_mut_ptr(),
                buf.len() as u32,
                0, // FILE_NAME_NORMALIZED (0) | VOLUME_NAME_DOS (0)
            )
        };
        if res == 0 {
            return Err(io::Error::last_os_error());
        }
        if (res as usize) < buf.len() {
            let os_string = std::ffi::OsString::from_wide(&buf[..res as usize]);
            return Ok(std::path::PathBuf::from(os_string));
        }
        buf.resize(res as usize, 0);
    }
}

#[cfg(windows)]
fn is_strictly_beneath(root: &Path, file: &Path) -> bool {
    use std::path::Component;

    let mut root_comps = root.components();
    let mut file_comps = file.components();

    loop {
        match (root_comps.next(), file_comps.next()) {
            (Some(Component::Prefix(p1)), Some(Component::Prefix(p2))) => {
                if !p1
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&p2.as_os_str().to_string_lossy())
                {
                    return false;
                }
            }
            (Some(Component::RootDir), Some(Component::RootDir)) => {}
            (Some(Component::Normal(n1)), Some(Component::Normal(n2))) => {
                if !n1
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&n2.to_string_lossy())
                {
                    return false;
                }
            }
            (None, Some(_)) => {
                return true;
            }
            _ => return false,
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn open_local_file(_root: &Path, path: &Path) -> io::Result<Option<fs::File>> {
    match OpenOptions::new().read(true).open(path) {
        Ok(file) => Ok(Some(file)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

#[cfg(unix)]
fn is_missing_or_unsupported(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
        || error.raw_os_error() == Some(sys::ELOOP)
        || error.raw_os_error() == Some(sys::ENXIO)
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
        let target = root.join("outside.txt");
        let link = root.join("link.txt");
        fs::write(&target, "outside")?;
        symlink(&target, &link)?;

        assert!(matches!(read_local_file(&root, &link)?, Probe::Missing));
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
        let path = root.join("candidate");
        let c_path = CString::new(path.as_os_str().as_bytes())?;
        assert_eq!(
            unsafe { sys::mkfifo(c_path.as_ptr(), 0o600 as sys::mode_t) },
            0
        );

        let (sender, receiver) = mpsc::channel();
        let thread_path = path.clone();
        let thread_root = root.to_path_buf();
        let worker = thread::spawn(move || {
            let result = read_local_file(&thread_root, &thread_path);
            sender.send(result).expect("result receiver should exist");
        });
        let result = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("FIFO candidate must not block local ingestion")?;

        assert!(matches!(result, Probe::Missing));
        worker.join().expect("file reader thread should finish");
        Ok(())
    }

    #[test]
    fn bounds_local_file_reads() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("bounded_read")?;
        let path = root.join("large.txt");
        fs::write(&path, vec![b'x'; MAX_FILE_SIZE as usize + 1])?;

        assert!(matches!(read_local_file(&root, &path)?, Probe::TooLarge));
        Ok(())
    }

    #[test]
    fn includes_file_at_exact_size_limit() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("exact_size")?;
        let path = root.join("exact.txt");
        fs::write(&path, vec![b'x'; MAX_FILE_SIZE as usize])?;

        assert!(matches!(read_local_file(&root, &path)?, Probe::Bytes(_)));
        Ok(())
    }

    #[test]
    fn handles_unlinked_file_as_missing() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("missing_file")?;
        let path = root.join("nonexistent.txt");

        assert!(matches!(read_local_file(&root, &path)?, Probe::Missing));
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_linux_openat2_constants() {
        assert_eq!(RESOLVE_BENEATH, 0x08);
        assert_eq!(RESOLVE_NO_SYMLINKS, 0x04);
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_is_strictly_beneath_boundaries() {
        let root = Path::new(r"\\?\C:\project\repo");
        assert!(is_strictly_beneath(
            root,
            Path::new(r"\\?\C:\project\repo\file.txt")
        ));
        assert!(is_strictly_beneath(
            root,
            Path::new(r"\\?\c:\PROJECT\REPO\sub\file.txt")
        ));
        assert!(!is_strictly_beneath(
            root,
            Path::new(r"\\?\C:\project\repo-evil\file.txt")
        ));
        assert!(!is_strictly_beneath(
            root,
            Path::new(r"\\?\C:\project\repo")
        ));
        assert!(!is_strictly_beneath(
            root,
            Path::new(r"\\?\C:\other\file.txt")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_read_local_file_rejects_escaping_file() -> Result<(), Box<dyn std::error::Error>>
    {
        let root_a = temp_dir("win_root_a")?;
        let root_b = temp_dir("win_root_b")?;

        let inside = root_a.join("inside.txt");
        let outside = root_b.join("outside.txt");
        fs::write(&inside, "inside content")?;
        fs::write(&outside, "outside content")?;

        // File inside root succeeds
        assert!(matches!(
            read_local_file(&root_a, &inside)?,
            Probe::Bytes(_)
        ));

        // File outside root is detected as root escape and excluded as Missing
        assert!(matches!(
            read_local_file(&root_a, &outside)?,
            Probe::Missing
        ));

        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_intermediate_junction_escape_rejected() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::process::Command;

        let root = temp_dir("win_junc_root")?;
        let outside = temp_dir("win_junc_outside")?;

        let secret = outside.join("secret.txt");
        fs::write(&secret, "confidential")?;

        let junction = root.join("link_to_outside");
        let status = Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                junction.to_str().unwrap(),
                outside.to_str().unwrap(),
            ])
            .status();

        if let Ok(s) = status
            && s.success()
        {
            let path_via_junction = junction.join("secret.txt");
            let probe = read_local_file(&root, &path_via_junction)?;
            assert!(
                matches!(probe, Probe::Missing),
                "File accessed through escaping junction must be treated as Probe::Missing"
            );
            let _ = fs::remove_dir(&junction);
        }

        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_symlink_escape_rejected_if_supported() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::windows::fs::symlink_file;

        let root = temp_dir("win_sym_root")?;
        let outside = temp_dir("win_sym_outside")?;

        let secret = outside.join("secret.txt");
        fs::write(&secret, "confidential")?;

        let symlink = root.join("link_to_secret.txt");
        // If system allows symlink creation (e.g. Developer Mode enabled)
        if symlink_file(&secret, &symlink).is_ok() {
            let probe = read_local_file(&root, &symlink)?;
            assert!(
                matches!(probe, Probe::Missing),
                "File accessed through escaping symlink must be treated as Probe::Missing"
            );
            let _ = fs::remove_file(&symlink);
        }

        Ok(())
    }
}
