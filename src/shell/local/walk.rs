//! Candidate discovery: a sorted, gitignore-aware walk and per-entry path classification.
//!
//! The walk loads `.gitignore` rules through `gitignore.rs`. Each directory's file is read once,
//! when the walk enters it, and the same bytes both validate the file and compile its rules.
//! No later step reads it again.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::vec;

use ignore::gitignore::Gitignore;

use super::gitignore::read_rules;
use crate::core::error::DomainError;
use crate::core::path::{AdmittedDir, CandidatePath, DepthExceededError, admit_dir, classify_leaf};
use crate::core::policy::{ExcludePatterns, MAX_DIR_ENTRIES, is_hidden_segment};
use crate::shell::IngestError;
use crate::shell::error::io_error_at;

/// A regular file found by the walk. The walk yields no other entry kind, so classification never sees one.
/// `parent` is the admitted directory that holds the file, so classification checks only the file itself.
pub(super) struct RegularFile {
    path: PathBuf,
    parent: Arc<AdmittedDir>,
    name: OsString,
}

impl RegularFile {
    pub(super) fn into_path(self) -> PathBuf {
        self.path
    }

    /// Test constructor. `relative` must have no parent directories, so the root is its parent.
    #[cfg(test)]
    pub(super) fn from_parts(path: PathBuf, relative: &Path) -> Self {
        let name = relative
            .file_name()
            .expect("test path has a file name")
            .to_os_string();
        Self {
            path,
            parent: Arc::new(AdmittedDir::root()),
            name,
        }
    }
}

/// Depth-first walk in file-name order. The frames are the directories on the current path, innermost last.
/// Excluded directories are pruned here, so neither their children nor their `.gitignore` is ever read.
pub(super) struct Walk<'a> {
    exclude: &'a ExcludePatterns,
    frames: Vec<Frame>,
}

/// One entered directory: its children not yet visited, the rules its `.gitignore` compiled to, and
/// its admitted state.
struct Frame {
    entries: vec::IntoIter<fs::DirEntry>,
    rules: Gitignore,
    admitted: Arc<AdmittedDir>,
}

impl<'a> Walk<'a> {
    /// Enters `root`. Its `.gitignore` and listing are read here, so a failure ends the run before any candidate.
    pub(super) fn new(root: &Path, exclude: &'a ExcludePatterns) -> Result<Self, IngestError> {
        Ok(Self {
            exclude,
            frames: vec![Frame::enter(root, Arc::new(AdmittedDir::root()))?],
        })
    }

    /// Prunes, enters, or yields one child of the innermost directory. `parent` is the innermost
    /// directory's admitted state.
    fn visit(
        &mut self,
        entry: &fs::DirEntry,
        parent: Arc<AdmittedDir>,
    ) -> Result<Option<RegularFile>, IngestError> {
        let name = entry.file_name();
        if is_hidden_segment(name.as_encoded_bytes()) {
            return Ok(None);
        }
        let path = entry.path();
        let kind = entry.file_type().map_err(|err| io_error_at(&path, &err))?;
        let is_dir = kind.is_dir();
        if (!is_dir && !kind.is_file()) || is_ignored(&self.frames, &path, is_dir) {
            return Ok(None);
        }
        if is_dir {
            if let Some(admitted) = admit_dir(
                AdmittedDir::clone(&parent),
                name.as_encoded_bytes(),
                self.exclude,
            ) {
                self.frames.push(Frame::enter(&path, Arc::new(admitted))?);
            }
            Ok(None)
        } else {
            Ok(Some(RegularFile { path, parent, name }))
        }
    }
}

impl Iterator for Walk<'_> {
    type Item = Result<RegularFile, IngestError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let frame = self.frames.last_mut()?;
            let Some(entry) = frame.entries.next() else {
                self.frames.pop();
                continue;
            };
            let parent = Arc::clone(&frame.admitted);
            match self.visit(&entry, parent) {
                Ok(Some(file)) => return Some(Ok(file)),
                Ok(None) => {}
                Err(err) => return Some(Err(err)),
            }
        }
    }
}

impl Frame {
    /// Reads `dir`'s `.gitignore`, then lists `dir` with children in file-name order.
    fn enter(dir: &Path, admitted: Arc<AdmittedDir>) -> Result<Self, IngestError> {
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

/// Classifies one regular file on the producer thread. `None` covers the output file and ignored files.
pub(super) fn classify_entry(
    output: Option<&Path>,
    exclude: &ExcludePatterns,
    file: &RegularFile,
) -> Result<Option<CandidatePath>, IngestError> {
    if output.is_some_and(|target| file.path.as_path() == target) {
        return Ok(None);
    }
    classify_leaf(&file.parent, file.name.as_encoded_bytes(), exclude)
        .map_err(|_| DepthExceededError::domain_error("local path").into())
}

#[cfg(test)]
mod tests {
    use super::super::validate_root;
    use super::*;
    use crate::core::policy::MAX_PATH_DEPTH;
    use crate::shell::testing::temp_dir;
    use ignore::WalkBuilder;

    #[test]
    fn rejects_excessive_path_depth() {
        let exclude = ExcludePatterns::default();
        // Admits `directories` nested directories, as the walk does before it reaches the file.
        let nested = |directories: usize| {
            let mut dir = AdmittedDir::root();
            for _ in 0..directories {
                dir = admit_dir(dir, b"d", &exclude).expect("d is not excluded");
            }
            dir
        };
        let file = |directories: usize| RegularFile {
            path: PathBuf::from("file.txt"),
            parent: Arc::new(nested(directories)),
            name: OsString::from("file.txt"),
        };
        assert!(classify_entry(None, &exclude, &file(MAX_PATH_DEPTH)).is_ok());

        let err = classify_entry(None, &exclude, &file(MAX_PATH_DEPTH + 1))
            .expect_err("one level past the limit must be rejected");
        assert!(
            err.to_string()
                .contains("local path exceeds maximum directory depth"),
            "unexpected message: {err}"
        );
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

    fn walk_files(root: &Path, exclude: &ExcludePatterns) -> Vec<String> {
        Walk::new(root, exclude)
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
        let temp = temp_dir("walk_reference")?;
        let root = validate_root(&temp)?;
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
        let temp = temp_dir("excluded_dir_prune")?;
        let root = validate_root(&temp)?;
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
    fn gitignore_is_read_once_per_directory() -> Result<(), Box<dyn std::error::Error>> {
        let temp = temp_dir("gitignore_read_once")?;
        let root = validate_root(&temp)?;
        fs::create_dir(root.join("sub"))?;
        fs::write(root.join("sub").join(".gitignore"), "two.txt\n")?;
        fs::write(root.join("sub").join("one.txt"), "one")?;
        fs::write(root.join("sub").join("two.txt"), "two")?;

        let binding = ExcludePatterns::default();
        let mut walk = Walk::new(&root, &binding)?;
        let first = walk.next().expect("one.txt is yielded")?;
        assert_eq!(first.into_path(), root.join("sub").join("one.txt"));

        // Corrupt the rules after they were read. A second read would fail with InvalidData;
        // the walk must keep the rules it already holds, so two.txt stays excluded and the walk ends.
        fs::write(root.join("sub").join(".gitignore"), b"\xff\n")?;
        assert!(walk.next().is_none());
        Ok(())
    }

    #[test]
    fn walk_propagates_error_from_invalid_nested_gitignore()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = temp_dir("walk_error_nested_gitignore")?;
        let root = validate_root(&temp)?;
        fs::create_dir(root.join("sub"))?;
        fs::write(root.join("sub").join(".gitignore"), b"\xff\n")?;
        fs::write(root.join("sub").join("file.txt"), "hello")?;

        let exclude = ExcludePatterns::default();
        let mut walk = Walk::new(&root, &exclude)?;
        let item = walk.next();
        assert!(item.is_some());
        assert!(item.unwrap().is_err());
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
        let err = Walk::new(&root, &exclude).err();
        assert!(
            matches!(&err, Some(IngestError::Domain(DomainError::InvalidMetadata(msg))) if msg.contains("exceeds")),
            "expected directory entry limit failure, got: {err:?}"
        );
        Ok(())
    }
}
