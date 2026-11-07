//! `.gitignore` loading for one directory: its bytes are read once and compile into the rules the walk matches.

use std::fs;
use std::io::{self, BufRead, Read};
use std::path::Path;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::core::policy::MAX_FILE_SIZE;
use crate::shell::error::{IngestError, io_error_at};

/// Byte order mark that Git ignores at the start of a `.gitignore`.
const UTF8_BOM: &str = "\u{feff}";

/// Reads `dir`'s `.gitignore`. A missing file, or a directory, contributes no rules. A symlink, a path
/// that is neither a regular file nor a directory (such as a FIFO), a file over `MAX_FILE_SIZE`, or
/// content that fails to parse fails the run, so no rule is skipped silently.
pub(super) fn read_rules(dir: &Path) -> Result<Gitignore, IngestError> {
    let gitignore = dir.join(".gitignore");
    let meta = match fs::symlink_metadata(&gitignore) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Gitignore::empty()),
        Err(err) => return Err(io_error_at(&gitignore, &err)),
    };
    if meta.is_symlink() {
        let err = io::Error::new(io::ErrorKind::InvalidInput, "symbolic link is not followed");
        return Err(io_error_at(&gitignore, &err));
    }
    if meta.is_dir() {
        return Ok(Gitignore::empty());
    }
    if !meta.is_file() {
        let err = io::Error::new(io::ErrorKind::InvalidInput, "not a regular file");
        return Err(io_error_at(&gitignore, &err));
    }
    let file = fs::File::open(&gitignore).map_err(|err| io_error_at(&gitignore, &err))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_SIZE + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| io_error_at(&gitignore, &err))?;
    if bytes.len() as u64 > MAX_FILE_SIZE {
        let err = io::Error::new(io::ErrorKind::FileTooLarge, "file exceeds the size limit");
        return Err(io_error_at(&gitignore, &err));
    }
    parse_rules(dir, bytes.as_slice()).map_err(|err| io_error_at(&gitignore, &err))
}

/// Compiles `reader` as the `.gitignore` rules of `dir`. Like `ignore`, it strips a leading BOM and
/// skips lines that are not valid globs. Unlike `ignore`, a line that is not UTF-8, or a rule set that
/// fails to compile, is an error, so no rule is dropped silently.
fn parse_rules<R: BufRead>(dir: &Path, reader: R) -> io::Result<Gitignore> {
    let mut builder = GitignoreBuilder::new(dir);
    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        let line = if index == 0 {
            line.trim_start_matches(UTF8_BOM)
        } else {
            line.as_str()
        };
        // An invalid glob line is skipped; the other lines still apply.
        let _ = builder.add_line(None, line);
    }
    builder
        .build()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parse_rules_strips_bom_skips_invalid_globs_and_rejects_non_utf8() {
        let dir = Path::new("root");
        let bom = parse_rules(dir, Cursor::new("\u{feff}*.bak\n")).expect("BOM line parses");
        assert!(bom.matched("root/x.bak", false).is_ignore());

        let invalid =
            parse_rules(dir, Cursor::new("[\n*.dat\n")).expect("invalid glob line is skipped");
        assert!(invalid.matched("root/z.dat", false).is_ignore());

        let err = parse_rules(dir, Cursor::new(b"ok\n\xff\n".to_vec()))
            .expect_err("non-UTF-8 line is an I/O error");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);

        let empty = parse_rules(dir, Cursor::new("")).expect("empty file parses");
        assert!(empty.matched("root/a.txt", false).is_none());
    }
}
