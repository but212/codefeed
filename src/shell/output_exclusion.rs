use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::core::error::IngestError;

#[derive(Debug, Clone)]
pub(crate) struct OutputFilter {
    canonical: PathBuf,
    #[cfg(unix)]
    dev_ino: Option<(u64, u64)>,
}

impl OutputFilter {
    pub(crate) fn new(output: &Path) -> Result<Option<Self>, IngestError> {
        let absolute = if output.is_absolute() {
            output.to_path_buf()
        } else {
            env::current_dir()?.join(output)
        };
        let normalized = normalize_path(&absolute);
        let Ok(canonical) = fs::canonicalize(&normalized) else {
            // An output file that does not exist on disk prior to ingest cannot be visited during walk
            return Ok(None);
        };

        #[cfg(unix)]
        let dev_ino = fs::symlink_metadata(&canonical).ok().map(|m| {
            use std::os::unix::fs::MetadataExt;
            (m.dev(), m.ino())
        });

        Ok(Some(Self {
            canonical,
            #[cfg(unix)]
            dev_ino,
        }))
    }

    pub(crate) fn is_excluded(&self, candidate: &Path) -> bool {
        #[cfg(unix)]
        if let (Some(target_id), Ok(meta)) = (self.dev_ino, fs::symlink_metadata(candidate)) {
            use std::os::unix::fs::MetadataExt;
            if (meta.dev(), meta.ino()) == target_id {
                return true;
            }
        }

        let Some(cand_name) = candidate.file_name() else {
            return false;
        };
        let Some(target_name) = self.canonical.file_name() else {
            return false;
        };

        if !names_match(cand_name, target_name) {
            return false;
        }

        candidate == self.canonical
            || fs::canonicalize(candidate).is_ok_and(|c| c == self.canonical)
    }
}

#[inline]
fn names_match(candidate: &std::ffi::OsStr, target: &std::ffi::OsStr) -> bool {
    #[cfg(any(windows, target_os = "macos"))]
    {
        if candidate.is_ascii() && target.is_ascii() {
            candidate
                .to_string_lossy()
                .eq_ignore_ascii_case(&target.to_string_lossy())
        } else {
            candidate
                .to_string_lossy()
                .chars()
                .flat_map(char::to_lowercase)
                .eq(target
                    .to_string_lossy()
                    .chars()
                    .flat_map(char::to_lowercase))
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        candidate == target
    }
}

pub(crate) fn normalize_path(path: &Path) -> PathBuf {
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

#[cfg(test)]
mod tests {
    use super::super::temp_dir;
    use super::*;

    #[test]
    fn normalizes_parent_and_current_dir_components() {
        let path = Path::new("/a/b/../c/./d");
        let normalized = normalize_path(path);
        assert_eq!(normalized, Path::new("/a/c/d"));
    }

    #[test]
    fn nonexistent_output_yields_none() -> Result<(), Box<dyn std::error::Error>> {
        let filter = OutputFilter::new(Path::new("nonexistent_output_definitely_missing.txt"))?;
        assert!(filter.is_none());
        Ok(())
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn matches_non_ascii_cased_files_pointing_to_same_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_non_ascii")?;
        let file_a = root.join("вывод.txt");
        fs::write(&file_a, "test")?;
        let file_b = root.join("ВЫВОД.TXT");
        let filter = OutputFilter::new(&file_a)?.expect("filter exists");
        let same = filter.is_excluded(&file_b);
        assert!(same);
        Ok(())
    }

    #[test]
    fn filter_skips_different_filename() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_filter_diff")?;
        let output = root.join("digest.txt");
        fs::write(&output, "output")?;
        let filter = OutputFilter::new(&output)?.expect("filter exists");
        assert!(!filter.is_excluded(&root.join("src/main.rs")));
        assert!(!filter.is_excluded(&root.join("digest_other.txt")));
        Ok(())
    }

    #[test]
    fn filter_identifies_output_file() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_filter_test")?;
        fs::create_dir_all(root.join("subdir"))?;
        let output = root.join("digest.txt");
        fs::write(&output, "output")?;

        let filter = OutputFilter::new(&output)?.expect("filter exists");
        assert!(filter.is_excluded(&output));
        assert!(filter.is_excluded(&root.join("subdir").join("..").join("digest.txt")));

        #[cfg(any(windows, target_os = "macos"))]
        assert!(filter.is_excluded(&root.join("DIGEST.TXT")));

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn excludes_hard_link_to_output_file() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_hard_link")?;
        let output = root.join("digest.txt");
        let hardlink = root.join("notes.txt");
        fs::write(&output, "output content")?;
        fs::hard_link(&output, &hardlink)?;

        let filter = OutputFilter::new(&output)?.expect("filter exists");
        assert!(filter.is_excluded(&hardlink));

        Ok(())
    }
}
