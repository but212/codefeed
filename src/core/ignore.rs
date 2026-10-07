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
    if !parent.is_empty() && crate::core::github_tree::is_hidden_path(parent) {
        return false;
    }
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
    if absolute == Path::new(REMOTE_ROOT) {
        return Vec::new();
    }
    let mut directories = Vec::new();
    let mut current = absolute.parent();
    while let Some(directory) = current {
        if !directory.starts_with(REMOTE_ROOT) || directory == Path::new(REMOTE_ROOT) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(base: &str, pattern: &str) -> RemoteIgnoreRule {
        let base = Path::new(REMOTE_ROOT).join(base);
        let mut builder = ignore::gitignore::GitignoreBuilder::new(&base);
        builder.add_line(None, pattern).unwrap();
        RemoteIgnoreRule {
            base,
            matcher: builder.build().unwrap(),
        }
    }

    #[test]
    fn matches_remote_gitignore_hierarchy() {
        let rules = vec![
            rule("", "target/"),
            rule("", "*.log"),
            rule("sub", "!keep.log"),
            rule("", "ignored/"),
            rule("ignored", "!keep.txt"),
        ];
        assert!(ignored_remote_path("target/debug/app", &rules));
        assert!(!ignored_remote_path("sub/keep.log", &rules));
        assert!(ignored_remote_path("sub/drop.log", &rules));
        assert!(ignored_remote_path("other/keep.log", &rules));
        assert!(ignored_remote_path("ignored/keep.txt", &rules));
    }

    #[test]
    fn loads_ignored_nested_gitignore_when_parent_is_visible() {
        let file_is_ignored = vec![rule("", "sub/.gitignore")];
        assert!(remote_ignore_file_is_reachable(
            "sub/.gitignore",
            &file_is_ignored
        ));

        let rules = vec![rule("", "sub/.gitignore"), rule("sub", "secret.txt")];
        assert!(ignored_remote_path("sub/secret.txt", &rules));

        let parent_is_ignored = vec![rule("", "sub/")];
        assert!(!remote_ignore_file_is_reachable(
            "sub/.gitignore",
            &parent_is_ignored
        ));
    }

    #[test]
    fn rejects_hidden_parent_remote_gitignore() {
        assert!(!remote_ignore_file_is_reachable(".github/.gitignore", &[]));
        assert!(!remote_ignore_file_is_reachable(
            "sub/.hidden/.gitignore",
            &[]
        ));
        assert!(remote_ignore_file_is_reachable(".gitignore", &[]));
        assert!(remote_ignore_file_is_reachable("visible/.gitignore", &[]));
    }

    #[test]
    fn does_not_traverse_above_remote_root() {
        let mut builder = ignore::gitignore::GitignoreBuilder::new("/");
        builder.add_line(None, "*").unwrap();
        let root_rule = RemoteIgnoreRule {
            base: PathBuf::from("/"),
            matcher: builder.build().unwrap(),
        };
        let root = Path::new(REMOTE_ROOT);
        assert_eq!(
            ignored_parent_directories(root, &[root_rule]),
            Vec::<PathBuf>::new()
        );
    }
}
