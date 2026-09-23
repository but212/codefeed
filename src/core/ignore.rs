use std::path::{Path, PathBuf};

use crate::core::model::REMOTE_ROOT;

pub(crate) struct RemoteIgnoreRule {
    pub(crate) base: PathBuf,
    pub(crate) matcher: ignore::gitignore::Gitignore,
}

pub(crate) fn remote_ignore_file_is_reachable(path: &str, rules: &[RemoteIgnoreRule]) -> bool {
    let parent = Path::new(path)
        .parent()
        .and_then(Path::to_str)
        .unwrap_or("");
    !ignored_remote_directory(parent, rules)
}

pub(crate) fn ignored_remote_path(path: &str, rules: &[RemoteIgnoreRule]) -> bool {
    let absolute = Path::new(REMOTE_ROOT).join(path);
    let ignored_directories = ignored_parent_directories(&absolute, rules);
    if ignored_directories
        .iter()
        .any(|directory| absolute.starts_with(directory))
    {
        return true;
    }
    remote_match(&absolute, false, rules, &ignored_directories)
}

fn ignored_remote_directory(path: &str, rules: &[RemoteIgnoreRule]) -> bool {
    let absolute = Path::new(REMOTE_ROOT).join(path);
    let ignored_directories = ignored_parent_directories(&absolute, rules);
    if ignored_directories
        .iter()
        .any(|directory| absolute.starts_with(directory))
    {
        return true;
    }
    remote_match(&absolute, true, rules, &ignored_directories)
}

fn ignored_parent_directories(absolute: &Path, rules: &[RemoteIgnoreRule]) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let mut current = absolute.parent();
    while let Some(directory) = current {
        if directory == Path::new(REMOTE_ROOT) {
            break;
        }
        directories.push(directory.to_path_buf());
        current = directory.parent();
    }
    directories.reverse();

    let mut ignored_directories = Vec::new();
    for directory in directories {
        if remote_match(&directory, true, rules, &ignored_directories) {
            ignored_directories.push(directory);
        }
    }
    ignored_directories
}

fn remote_match(
    path: &Path,
    is_dir: bool,
    rules: &[RemoteIgnoreRule],
    ignored_directories: &[PathBuf],
) -> bool {
    let mut ignored = false;
    for rule in rules {
        if !path.starts_with(&rule.base)
            || ignored_directories
                .iter()
                .any(|directory| rule.base.starts_with(directory))
        {
            continue;
        }
        match rule.matcher.matched_path_or_any_parents(path, is_dir) {
            result if result.is_ignore() => ignored = true,
            result if result.is_whitelist() => ignored = false,
            _ => {}
        }
    }
    ignored
}
