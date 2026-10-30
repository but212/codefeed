//! Candidate discovery: a sorted, gitignore-aware walk and per-entry path classification.
//!
//! The walk owns `.gitignore` loading. Each directory's file is read once, when the walk enters it,
//! and the same bytes both validate the file and compile its rules. No later step reads it again.

use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::vec;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::core::error::DomainError;
use crate::core::policy::{PathStatus, classify_path_segments, is_hidden_segment};
use crate::shell::IngestError;
use crate::shell::error::io_error_at;

/// Byte order mark that Git ignores at the start of a `.gitignore`.
const UTF8_BOM: &str = "\u{feff}";

/// A regular file found by the walk. The walk yields no other entry kind, so classification never sees one.
pub(super) struct RegularFile(PathBuf);

impl RegularFile {
    pub(super) fn as_path(&self) -> &Path {
        &self.0
    }

    pub(super) fn into_path(self) -> PathBuf {
        self.0
    }
}

/// Depth-first walk in file-name order. The frames are the directories on the current path, innermost last.
pub(super) struct Walk {
    frames: Vec<Frame>,
}

/// One entered directory: its children not yet visited, and the rules its `.gitignore` compiled to.
struct Frame {
    entries: vec::IntoIter<fs::DirEntry>,
    rules: Gitignore,
}

impl Walk {
    /// Enters `root`. Its `.gitignore` and listing are read here, so a failure ends the run before any candidate.
    pub(super) fn new(root: &Path) -> Result<Self, IngestError> {
        Ok(Self {
            frames: vec![Frame::enter(root)?],
        })
    }

    /// Prunes, enters, or yields one child of the innermost directory.
    fn visit(&mut self, entry: &fs::DirEntry) -> Result<Option<RegularFile>, IngestError> {
        if is_hidden_segment(entry.file_name().as_encoded_bytes()) {
            return Ok(None);
        }
        let path = entry.path();
        let kind = entry.file_type().map_err(|err| io_error_at(&path, &err))?;
        let is_dir = kind.is_dir();
        if (!is_dir && !kind.is_file()) || is_ignored(&self.frames, &path, is_dir) {
            return Ok(None);
        }
        if is_dir {
            self.frames.push(Frame::enter(&path)?);
            Ok(None)
        } else {
            Ok(Some(RegularFile(path)))
        }
    }
}

impl Iterator for Walk {
    type Item = Result<RegularFile, IngestError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let frame = self.frames.last_mut()?;
            let Some(entry) = frame.entries.next() else {
                self.frames.pop();
                continue;
            };
            match self.visit(&entry) {
                Ok(Some(file)) => return Some(Ok(file)),
                Ok(None) => {}
                Err(err) => return Some(Err(err)),
            }
        }
    }
}

impl Frame {
    /// Reads `dir`'s `.gitignore`, then lists `dir` with children in file-name order.
    fn enter(dir: &Path) -> Result<Self, IngestError> {
        let rules = read_rules(dir)?;
        let mut entries = fs::read_dir(dir)
            .and_then(Iterator::collect::<io::Result<Vec<_>>>)
            .map_err(|err| io_error_at(dir, &err))?;
        entries.sort_by_key(fs::DirEntry::file_name);
        Ok(Self {
            entries: entries.into_iter(),
            rules,
        })
    }
}

/// Whether the closest `.gitignore` that has an opinion on `path` excludes it. A `!` pattern there re-includes it.
fn is_ignored(frames: &[Frame], path: &Path, is_dir: bool) -> bool {
    frames
        .iter()
        .rev()
        .map(|frame| frame.rules.matched(path, is_dir))
        .find(|found| !found.is_none())
        .is_some_and(|found| found.is_ignore())
}

/// Reads `dir`'s `.gitignore`. A missing file, or one that is not a regular file, contributes no rules.
fn read_rules(dir: &Path) -> Result<Gitignore, IngestError> {
    let gitignore = dir.join(".gitignore");
    match fs::metadata(&gitignore) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Ok(Gitignore::empty()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Gitignore::empty()),
        Err(err) => return Err(io_error_at(&gitignore, &err)),
    }
    let file = fs::File::open(&gitignore).map_err(|err| io_error_at(&gitignore, &err))?;
    parse_rules(dir, BufReader::new(file)).map_err(|err| io_error_at(&gitignore, &err))
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

/// Classifies one regular file on the producer thread. `Ignored` covers the output file.
pub(super) fn classify_entry(
    root: &Path,
    output: Option<&Path>,
    path: &Path,
) -> Result<PathStatus, IngestError> {
    if output.is_some_and(|target| path == target) {
        return Ok(PathStatus::Ignored);
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|err| DomainError::invalid_metadata(err.to_string()))?;
    classify_local_path(relative).map_err(IngestError::from)
}

fn classify_local_path(relative: &Path) -> Result<PathStatus, DomainError> {
    let segments = relative
        .components()
        .map(|c| c.as_os_str().as_encoded_bytes());
    classify_path_segments(segments)
        .map_err(|_| DomainError::invalid_metadata("local path exceeds maximum directory depth"))
}

#[cfg(test)]
mod tests {
    use super::super::validate_root;
    use super::*;
    use crate::shell::testing::temp_dir;
    use ignore::WalkBuilder;
    use std::io::Cursor;
    use std::path::PathBuf;

    #[test]
    fn rejects_excessive_path_depth() {
        let mut excessive = PathBuf::new();
        for _ in 0..257 {
            excessive.push("d");
        }
        excessive.push("file.txt");
        let result = classify_local_path(&excessive);
        assert!(result.is_err());

        let mut normal = PathBuf::new();
        for _ in 0..256 {
            normal.push("d");
        }
        normal.push("file.txt");
        let result = classify_local_path(&normal);
        assert!(result.is_ok());
    }

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

    /// Regular files under `root` as `ignore::WalkBuilder` reports them with the same settings.
    fn reference_files(root: &Path) -> Vec<String> {
        WalkBuilder::new(root)
            .hidden(false)
            .filter_entry(|entry| {
                entry.depth() == 0 || !is_hidden_segment(entry.file_name().as_encoded_bytes())
            })
            .ignore(false)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(false)
            .require_git(false)
            .parents(false)
            .follow_links(false)
            .sort_by_file_name(std::cmp::Ord::cmp)
            .build()
            .map(|entry| entry.expect("reference walk succeeds"))
            .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
            .map(|entry| relative_string(root, entry.path()))
            .collect()
    }

    fn walk_files(root: &Path) -> Vec<String> {
        Walk::new(root)
            .expect("walk opens the root")
            .map(|file| relative_string(root, &file.expect("walk yields files").into_path()))
            .collect()
    }

    fn relative_string(root: &Path, path: &Path) -> String {
        path.strip_prefix(root)
            .expect("entry under root")
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Nested rules, negation, anchoring, directory-only patterns, pruning, BOM, and an invalid glob.
    /// `build/` and `sub/cache/` hold non-UTF-8 `.gitignore` files that must never be read.
    fn write_reference_fixture(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
        fs::write(
            root.join(".gitignore"),
            "*.log\n!keep.log\n/top_only\nbuild/\n",
        )?;
        fs::write(root.join("a.log"), "a")?;
        fs::write(root.join("keep.log"), "keep")?;
        fs::write(root.join("top_only"), "anchored")?;
        fs::write(root.join("b.txt"), "b")?;
        fs::write(root.join(".hidden.txt"), "hidden")?;

        fs::create_dir_all(root.join(".h"))?;
        fs::write(root.join(".h").join("x.txt"), "x")?;

        fs::create_dir_all(root.join("build"))?;
        fs::write(root.join("build").join(".gitignore"), b"\xff\n")?;
        fs::write(root.join("build").join("inside.txt"), "inside")?;

        fs::create_dir_all(root.join("sub").join("deeper"))?;
        fs::write(
            root.join("sub").join(".gitignore"),
            "secret.txt\n*.tmp\ncache/\n",
        )?;
        fs::write(root.join("sub").join("secret.txt"), "s")?;
        fs::write(root.join("sub").join("top_only"), "nested")?;
        fs::write(root.join("sub").join("x.tmp"), "t")?;
        fs::write(root.join("sub").join("keep.log"), "k")?;
        fs::write(
            root.join("sub").join("deeper").join(".gitignore"),
            "!secret.txt\n",
        )?;
        fs::write(root.join("sub").join("deeper").join("secret.txt"), "s")?;
        fs::write(root.join("sub").join("deeper").join("y.tmp"), "y")?;

        fs::create_dir_all(root.join("sub").join("cache"))?;
        fs::write(root.join("sub").join("cache").join(".gitignore"), b"\xff\n")?;
        fs::write(root.join("sub").join("cache").join("data.txt"), "data")?;

        fs::create_dir_all(root.join("bom"))?;
        fs::write(root.join("bom").join(".gitignore"), "\u{feff}*.bak\n")?;
        fs::write(root.join("bom").join("x.bak"), "x")?;
        fs::write(root.join("bom").join("y.txt"), "y")?;

        fs::create_dir_all(root.join("bad"))?;
        fs::write(root.join("bad").join(".gitignore"), "[\n*.dat\n")?;
        fs::write(root.join("bad").join("z.dat"), "z")?;
        fs::write(root.join("bad").join("z.txt"), "z")?;

        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("b.txt"), root.join("link.txt"))?;
        Ok(())
    }

    #[test]
    fn walk_matches_ignore_crate_reference() -> Result<(), Box<dyn std::error::Error>> {
        let temp = temp_dir("walk_reference")?;
        let root = validate_root(&temp)?;
        write_reference_fixture(&root)?;

        let ours = walk_files(&root);
        assert_eq!(ours, reference_files(&root));

        // Equality alone would also hold if both sides were empty, so pin the rules themselves.
        let kept = [
            "keep.log",
            "b.txt",
            "sub/top_only",
            "sub/keep.log",
            "sub/deeper/secret.txt",
            "bom/y.txt",
            "bad/z.txt",
        ];
        let dropped = [
            "a.log",
            "top_only",
            "sub/secret.txt",
            "sub/x.tmp",
            "sub/deeper/y.tmp",
            "build/inside.txt",
            "sub/cache/data.txt",
            "bom/x.bak",
            "bad/z.dat",
            ".hidden.txt",
            ".h/x.txt",
        ];
        for path in kept {
            assert!(
                ours.iter().any(|found| found == path),
                "{path} missing from {ours:?}"
            );
        }
        for path in dropped {
            assert!(
                !ours.iter().any(|found| found == path),
                "{path} present in {ours:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn gitignore_is_read_once_per_directory() -> Result<(), Box<dyn std::error::Error>> {
        let temp = temp_dir("gitignore_read_once")?;
        let root = validate_root(&temp)?;
        fs::create_dir(root.join("sub"))?;
        fs::write(root.join("sub").join(".gitignore"), "two.txt\n")?;
        fs::write(root.join("sub").join("one.txt"), "one")?;
        fs::write(root.join("sub").join("two.txt"), "two")?;

        let mut walk = Walk::new(&root)?;
        let first = walk.next().expect("one.txt is yielded")?;
        assert_eq!(first.into_path(), root.join("sub").join("one.txt"));

        // Corrupt the rules after they were read. A second read would fail with InvalidData;
        // the walk must keep the rules it already holds, so two.txt stays excluded and the walk ends.
        fs::write(root.join("sub").join(".gitignore"), b"\xff\n")?;
        assert!(walk.next().is_none());
        Ok(())
    }
}
