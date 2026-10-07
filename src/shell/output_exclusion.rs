use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::core::error::IngestError;

pub(crate) fn is_same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    #[cfg(unix)]
    {
        if let (Ok(meta_a), Ok(meta_b)) = (fs::metadata(a), fs::metadata(b)) {
            use std::os::unix::fs::MetadataExt;
            if meta_a.dev() == meta_b.dev() && meta_a.ino() == meta_b.ino() {
                return true;
            }
        }
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

pub(crate) fn output_identity(output: &Path) -> Result<PathBuf, IngestError> {
    let absolute = if output.is_absolute() {
        output.to_path_buf()
    } else {
        env::current_dir()?.join(output)
    };
    if let Ok(canonical) = fs::canonicalize(&absolute) {
        return Ok(canonical);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_parent_and_current_dir_components() {
        let path = Path::new("/a/b/../c/./d");
        let normalized = normalize_path(path);
        assert_eq!(normalized, Path::new("/a/c/d"));
    }

    #[test]
    fn exact_path_is_same() {
        assert!(is_same_path(Path::new("a/b/c"), Path::new("a/b/c")));
        assert!(!is_same_path(Path::new("a/b/c"), Path::new("a/b/d")));
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn matches_non_ascii_cased_files_pointing_to_same_file() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "output_non_ascii_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        fs::create_dir_all(&root)?;
        let file_a = root.join("вывод.txt");
        fs::write(&file_a, "test")?;
        let file_b = root.join("ВЫВОД.TXT");
        let same = is_same_path(&file_a, &file_b);
        fs::remove_dir_all(&root)?;
        assert!(same);
        Ok(())
    }
}
