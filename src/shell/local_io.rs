//! Local file acquisition: non-symlink metadata validation and bounded streaming read.

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use crate::core::decide::Probe;
use crate::core::error::IngestError;
use crate::core::policy::MAX_FILE_SIZE;

pub(crate) fn read_local_file_into<'b>(
    path: &Path,
    buf: &'b mut Vec<u8>,
) -> Result<Probe<'b>, IngestError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Probe::Missing),
        Err(err) => return Err(IngestError::Io(err)),
    };
    if meta.is_symlink() || !meta.is_file() {
        return Ok(Probe::Missing);
    }
    if meta.len() > MAX_FILE_SIZE {
        return Ok(Probe::TooLarge);
    }
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Probe::Missing),
        Err(err) => return Err(IngestError::Io(err)),
    };
    read_bounded_content_into(file, buf).map_err(Into::into)
}

fn read_bounded_content_into<'b>(file: fs::File, buf: &'b mut Vec<u8>) -> io::Result<Probe<'b>> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Ok(Probe::Missing);
    }
    if metadata.len() > MAX_FILE_SIZE {
        return Ok(Probe::TooLarge);
    }

    let mut handle = file.take(MAX_FILE_SIZE.saturating_add(1));
    handle.read_to_end(buf)?;
    if buf.len() as u64 > MAX_FILE_SIZE {
        return Ok(Probe::TooLarge);
    }
    Ok(Probe::Bytes(std::borrow::Cow::Borrowed(buf.as_slice())))
}

#[cfg(test)]
mod tests {
    use super::super::temp_dir;
    use super::*;

    #[test]
    fn read_local_file_enforces_size_boundaries() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("size_boundaries")?;
        let exact_path = root.join("exact.txt");
        let too_large_path = root.join("large.txt");
        fs::write(&exact_path, vec![b'x'; MAX_FILE_SIZE as usize])?;
        fs::write(&too_large_path, vec![b'x'; MAX_FILE_SIZE as usize + 1])?;

        let mut buf = Vec::new();
        assert!(matches!(
            read_local_file_into(&exact_path, &mut buf)?,
            Probe::Bytes(_)
        ));
        buf.clear();
        assert!(matches!(
            read_local_file_into(&too_large_path, &mut buf)?,
            Probe::TooLarge
        ));
        Ok(())
    }

    #[test]
    fn handles_unlinked_file_as_missing() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("missing_file")?;
        let path = root.join("nonexistent.txt");

        let mut buf = Vec::new();
        assert!(matches!(
            read_local_file_into(&path, &mut buf)?,
            Probe::Missing
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_file() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let root = temp_dir("symlink_test")?;
        let target = root.join("target.txt");
        let link = root.join("link.txt");
        fs::write(&target, "content")?;
        symlink(&target, &link)?;

        let mut buf = Vec::new();
        assert!(matches!(
            read_local_file_into(&link, &mut buf)?,
            Probe::Missing
        ));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_symlink_file() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::windows::fs::symlink_file;

        let root = temp_dir("win_symlink_test")?;
        let target = root.join("target.txt");
        let link = root.join("link.txt");
        fs::write(&target, "content")?;

        if symlink_file(&target, &link).is_ok() {
            let mut buf = Vec::new();
            assert!(matches!(
                read_local_file_into(&link, &mut buf)?,
                Probe::Missing
            ));
            let _ = fs::remove_file(&link);
        }
        Ok(())
    }
}
