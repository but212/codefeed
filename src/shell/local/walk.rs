//! Candidate discovery: a sorted, gitignore-aware walk and per-entry path classification.
//!
//! The walk loads each directory's `.gitignore` rules once, when entering it, and the same bytes
//! both validate the file and compile its rules. No later step reads it again.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::vec;

use ignore::gitignore::Gitignore;

use crate::core::error::DomainError;
use crate::core::path::{AdmittedDir, Candidate, admit_dir, classify_leaf};
use crate::core::policy::{
    ExcludePatterns, MAX_DIR_ENTRIES, MAX_FILE_SIZE, compile_gitignore, is_hidden_segment,
};
use crate::shell::error::{IngestError, io_error_at, sanitized_io_error};

/// Depth-first walk in file-name order. The frames are the directories on the current path, innermost last.
/// Excluded directories are pruned here, so neither their children nor their `.gitignore` is ever read.
pub(super) struct Walk<'a> {
    output: Option<&'a Path>,
    exclude: &'a ExcludePatterns,
    aborted: &'a AtomicBool,
    frames: Vec<Frame>,
}

/// One entered directory: its children not yet visited, the rules its `.gitignore` compiled to, and
/// its admitted state.
struct Frame {
    entries: vec::IntoIter<fs::DirEntry>,
    rules: Gitignore,
    admitted: AdmittedDir,
}

impl<'a> Walk<'a> {
    /// Enters `root`. Its `.gitignore` and listing are read here, so a failure ends the run before any candidate.
    pub(super) fn new(
        root: &Path,
        output: Option<&'a Path>,
        exclude: &'a ExcludePatterns,
        aborted: &'a AtomicBool,
    ) -> Result<Self, IngestError> {
        Ok(Self {
            output,
            exclude,
            aborted,
            frames: vec![Frame::enter(root, AdmittedDir::root())?],
        })
    }

    fn next_candidate(&mut self) -> Result<Option<Candidate<PathBuf>>, IngestError> {
        // Check cancellation even between entries that never yield a slot.
        while !self.frames.is_empty() && !self.aborted.load(Ordering::Relaxed) {
            let top = self.frames.len() - 1;
            let Some(entry) = self.frames[top].entries.next() else {
                self.frames.pop();
                continue;
            };
            let name = entry.file_name();
            let name = name.as_encoded_bytes();
            if is_hidden_segment(name) {
                continue;
            }
            let kind = entry
                .file_type()
                .map_err(|err| io_error_at(&entry.path(), &err))?;
            if !kind.is_dir() && !kind.is_file() {
                continue;
            }
            let path = entry.path();
            if is_ignored(&self.frames, &path, kind.is_dir()) {
                continue;
            }
            let parent = &self.frames[top].admitted;
            if kind.is_dir() {
                if let Some(child) = admit_dir(parent.clone(), name, self.exclude, "local path")? {
                    self.frames.push(Frame::enter(&path, child)?);
                }
            } else if self.output != Some(path.as_path()) {
                match classify_leaf(parent, name, self.exclude) {
                    Some(Candidate::Valid(relative, ())) => {
                        return Ok(Some(Candidate::Valid(relative, path)));
                    }
                    Some(Candidate::Unstorable(key)) => {
                        return Ok(Some(Candidate::Unstorable(key)));
                    }
                    None => {}
                }
            }
        }
        Ok(None)
    }
}

impl Iterator for Walk<'_> {
    type Item = Result<Candidate<PathBuf>, IngestError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_candidate().transpose()
    }
}

impl Frame {
    /// Reads `dir`'s `.gitignore`, then lists `dir` with children in file-name order.
    fn enter(dir: &Path, admitted: AdmittedDir) -> Result<Self, IngestError> {
        let rules = read_rules(dir)?;
        let mut entries = list_dir(dir)?;
        entries.sort_by_key(fs::DirEntry::file_name);
        Ok(Self {
            entries: entries.into_iter(),
            rules,
            admitted,
        })
    }
}

/// Lists `dir`, failing once it holds more than `MAX_DIR_ENTRIES` entries, so the listing is bounded
/// before any entry is classified.
fn list_dir(dir: &Path) -> Result<Vec<fs::DirEntry>, IngestError> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).map_err(|err| io_error_at(dir, &err))? {
        if entries.len() >= MAX_DIR_ENTRIES {
            return Err(DomainError::invalid_metadata(format!(
                "directory exceeds {MAX_DIR_ENTRIES} entries: {}",
                dir.display()
            ))
            .into());
        }
        entries.push(entry.map_err(|err| io_error_at(dir, &err))?);
    }
    Ok(entries)
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

/// Reads `dir`'s `.gitignore`. A missing file, or a directory, contributes no rules. A symlink, a path
/// that is neither a regular file nor a directory (such as a FIFO), a file over `MAX_FILE_SIZE`, or
/// content rejected by [`compile_gitignore`] fails the run.
fn read_rules(dir: &Path) -> Result<Gitignore, IngestError> {
    let gitignore = dir.join(".gitignore");
    let meta = match fs::symlink_metadata(&gitignore) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Gitignore::empty()),
        Err(err) => return Err(io_error_at(&gitignore, &err)),
    };
    if meta.is_symlink() {
        return Err(sanitized_io_error(
            io::ErrorKind::InvalidInput,
            format!("{}: symbolic link is not followed", gitignore.display()),
        ));
    }
    if meta.is_dir() {
        return Ok(Gitignore::empty());
    }
    if !meta.is_file() {
        return Err(sanitized_io_error(
            io::ErrorKind::InvalidInput,
            format!("{}: not a regular file", gitignore.display()),
        ));
    }
    let file = fs::File::open(&gitignore).map_err(|err| io_error_at(&gitignore, &err))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_SIZE + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| io_error_at(&gitignore, &err))?;
    if bytes.len() as u64 > MAX_FILE_SIZE {
        return Err(sanitized_io_error(
            io::ErrorKind::FileTooLarge,
            format!("{}: file exceeds the size limit", gitignore.display()),
        ));
    }
    compile_gitignore(dir, bytes.as_slice()).map_err(|msg| {
        sanitized_io_error(
            io::ErrorKind::InvalidData,
            format!("{}: {msg}", gitignore.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::policy::MAX_PATH_DEPTH;
    use crate::shell::testing::temp_dir;
    use ignore::WalkBuilder;

    static NOT_CANCELLED: AtomicBool = AtomicBool::new(false);

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

    fn walk_files(root: &Path, exclude: &ExcludePatterns) -> Vec<String> {
        Walk::new(root, None, exclude, &NOT_CANCELLED)
            .expect("walk opens the root")
            .map(|item| match item.expect("walk succeeds") {
                Candidate::Valid(relative, path) => {
                    assert_eq!(&*relative, relative_string(root, &path));
                    relative.to_string()
                }
                Candidate::Unstorable(_) => panic!("fixture paths are storable"),
            })
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
            "*.log\n!keep.log\n/top_only\nbuild/\npruned_parent/\n*.level\ntarget_dir_only/\ntarget_either\n",
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

        fs::create_dir_all(root.join("bom_double"))?;
        fs::write(
            root.join("bom_double").join(".gitignore"),
            "\u{feff}\u{feff}*.bak\n",
        )?;
        fs::write(root.join("bom_double").join("x.bak"), "x")?;
        fs::write(root.join("bom_double").join("y.txt"), "y")?;

        fs::create_dir_all(root.join("bad"))?;
        fs::write(root.join("bad").join(".gitignore"), "[\n*.dat\n")?;
        fs::write(root.join("bad").join("z.dat"), "z")?;
        fs::write(root.join("bad").join("z.txt"), "z")?;

        fs::create_dir_all(root.join("pruned_parent"))?;
        fs::write(
            root.join("pruned_parent").join(".gitignore"),
            "!reinclude.txt\n",
        )?;
        fs::write(root.join("pruned_parent").join("reinclude.txt"), "r")?;

        fs::write(root.join("root_dropped.level"), "d")?;
        fs::create_dir_all(root.join("tier").join("deep"))?;
        fs::write(root.join("tier").join(".gitignore"), "!keep.level\n")?;
        fs::write(root.join("tier").join("keep.level"), "k")?;
        fs::write(root.join("tier").join("other.level"), "o")?;
        fs::write(
            root.join("tier").join("deep").join(".gitignore"),
            "keep.level\n",
        )?;
        fs::write(root.join("tier").join("deep").join("keep.level"), "k")?;

        fs::create_dir_all(root.join("sub_slash").join("target_dir_only"))?;
        fs::write(
            root.join("sub_slash")
                .join("target_dir_only")
                .join("child.txt"),
            "c",
        )?;
        fs::create_dir_all(root.join("sub_slash").join("target_either"))?;
        fs::write(
            root.join("sub_slash")
                .join("target_either")
                .join("child.txt"),
            "c",
        )?;
        fs::create_dir_all(root.join("sub_file"))?;
        fs::write(root.join("sub_file").join("target_dir_only"), "file")?;
        fs::write(root.join("sub_file").join("target_either"), "file")?;

        fs::create_dir_all(root.join("bom_mixed"))?;
        fs::write(
            root.join("bom_mixed").join(".gitignore"),
            "\u{feff}# Comment\n\n*.bomdrop\n",
        )?;
        fs::write(root.join("bom_mixed").join("drop.bomdrop"), "d")?;
        fs::write(root.join("bom_mixed").join("keep.txt"), "k")?;
        fs::write(
            root.join("bom_mixed").join("bom_content.txt"),
            "\u{feff}bom body",
        )?;

        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("b.txt"), root.join("link.txt"))?;
        Ok(())
    }

    #[test]
    fn walk_matches_ignore_crate_reference() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("walk_reference")?;
        write_reference_fixture(&root)?;

        let ours = walk_files(&root, &ExcludePatterns::default());
        assert_eq!(ours, reference_files(&root));

        // Equality alone would also hold if both sides were empty, so pin the rules themselves.
        let kept = [
            "keep.log",
            "b.txt",
            "sub/top_only",
            "sub/keep.log",
            "sub/deeper/secret.txt",
            "bom/y.txt",
            "bom_double/y.txt",
            "bad/z.txt",
            "tier/keep.level",
            "sub_file/target_dir_only",
            "bom_mixed/keep.txt",
            "bom_mixed/bom_content.txt",
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
            "bom_double/x.bak",
            "bad/z.dat",
            ".hidden.txt",
            ".h/x.txt",
            "pruned_parent/reinclude.txt",
            "root_dropped.level",
            "tier/other.level",
            "tier/deep/keep.level",
            "sub_slash/target_dir_only/child.txt",
            "sub_slash/target_either/child.txt",
            "sub_file/target_either",
            "bom_mixed/drop.bomdrop",
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
    fn user_excluded_directory_is_pruned_without_reading_its_gitignore()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("excluded_dir_prune")?;
        fs::create_dir(root.join("build"))?;
        // Non-UTF-8 rules fail the run if the walk ever reads them.
        fs::write(root.join("build").join(".gitignore"), b"\xff\n")?;
        fs::write(root.join("build").join("out.txt"), "out")?;
        fs::write(root.join("keep.txt"), "keep")?;
        let exclude = ExcludePatterns::new(["build/"])?;

        assert_eq!(walk_files(&root, &exclude), vec!["keep.txt"]);
        Ok(())
    }

    #[test]
    fn walk_enforces_depth_limit() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("walk_depth_limit")?;
        let mut deepest = root.to_path_buf();
        for _ in 0..MAX_PATH_DEPTH {
            deepest.push("d");
            fs::create_dir(&deepest)?;
        }
        let exclude = ExcludePatterns::default();
        assert!(
            Walk::new(&root, None, &exclude, &NOT_CANCELLED)?
                .next()
                .is_none()
        );
        fs::create_dir(deepest.join("pruned"))?;
        fs::write(deepest.join("pruned/.gitignore"), b"\xff\n")?;
        let prune = ExcludePatterns::new(["pruned/"])?;
        assert!(
            Walk::new(&root, None, &prune, &NOT_CANCELLED)?
                .next()
                .is_none()
        );

        let item = Walk::new(&root, None, &exclude, &NOT_CANCELLED)?.next();
        assert!(
            matches!(item, Some(Err(IngestError::Domain(DomainError::InvalidMetadata(ref msg))))
            if msg.contains("local path exceeds maximum directory depth"))
        );

        let leaf = deepest.join("file.txt");
        fs::write(&leaf, "at the depth limit")?;
        let relative = format!("{}file.txt", "d/".repeat(MAX_PATH_DEPTH));
        let item = Walk::new(&root, None, &prune, &NOT_CANCELLED)?.next();
        assert!(matches!(
            item,
            Some(Ok(Candidate::Valid(path, entry_path))) if *path == relative && entry_path == leaf
        ));
        Ok(())
    }

    #[test]
    fn cancelled_walk_does_not_open_a_directory_after_its_last_candidate()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("walk_cancelled")?;
        fs::write(root.join("a.txt"), "first candidate")?;
        fs::create_dir(root.join("z"))?;
        // No more candidates will be yielded. Reading the tail is observable only via this error.
        fs::write(root.join("z/.gitignore"), b"\xff\n")?;

        let exclude = ExcludePatterns::default();
        let aborted = AtomicBool::new(false);
        let mut walk = Walk::new(&root, None, &exclude, &aborted)?;
        let first = walk.next().expect("first file is yielded")?;
        assert!(matches!(first, Candidate::Valid(_, path) if path == root.join("a.txt")));
        aborted.store(true, Ordering::Relaxed);
        assert!(walk.next().is_none());
        Ok(())
    }

    #[test]
    fn gitignore_is_read_once_per_directory() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("gitignore_read_once")?;
        fs::create_dir(root.join("sub"))?;
        fs::write(root.join("sub").join(".gitignore"), "two.txt\n")?;
        fs::write(root.join("sub").join("one.txt"), "one")?;
        fs::write(root.join("sub").join("two.txt"), "two")?;

        let binding = ExcludePatterns::default();
        let mut walk = Walk::new(&root, None, &binding, &NOT_CANCELLED)?;
        let first = walk.next().expect("one.txt is yielded")?;
        assert!(matches!(first, Candidate::Valid(_, path) if path == root.join("sub/one.txt")));

        // Corrupt the rules after they were read. A second read would fail with InvalidData;
        // the walk must keep the rules it already holds, so two.txt stays excluded and the walk ends.
        fs::write(root.join("sub").join(".gitignore"), b"\xff\n")?;
        assert!(walk.next().is_none());
        Ok(())
    }

    #[test]
    fn directory_with_more_entries_than_the_limit_fails_before_listing_all()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::core::policy::MAX_DIR_ENTRIES;

        let root = temp_dir("walk_dir_entry_limit")?;
        // Hidden entries never become candidates, so only the listing limit can stop them.
        for index in 0..=MAX_DIR_ENTRIES {
            fs::write(root.join(format!(".h{index}")), "")?;
        }
        let exclude = ExcludePatterns::default();
        let err = Walk::new(&root, None, &exclude, &NOT_CANCELLED).err();
        assert!(
            matches!(&err, Some(IngestError::Domain(DomainError::InvalidMetadata(msg))) if msg.contains("exceeds")),
            "expected directory entry limit failure, got: {err:?}"
        );
        Ok(())
    }

    #[test]
    fn gitignore_metadata_error_other_than_not_found_fails_the_run() {
        // A NUL byte makes std reject the path before the syscall, on every platform.
        let err = read_rules(Path::new("dir\0name")).expect_err("only NotFound means no rules");
        match err {
            IngestError::Io(err) => {
                assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
                assert!(err.to_string().contains(".gitignore"), "got: {err}");
            }
            other => panic!("expected IngestError::Io, got {other:?}"),
        }
    }
}
