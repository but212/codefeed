//! Local directory collection: sorted walk, bounded reads, and in-order selection.
//!
//! The work is split by concern:
//! - `walk`: which entries are candidates, and how their paths are classified.
//! - `gitignore`: how each directory's `.gitignore` is read and compiled into rules.
//! - `read`: how one candidate's metadata and bounded body are read.
//! - `pipeline`: the sorted producer, the read workers, and settlement in walk order.

mod gitignore;
mod pipeline;
mod read;
mod walk;

use std::fs;
use std::path::{Path, PathBuf};

use self::pipeline::fold_candidates;
use super::error::IngestError;
use super::output::ResolvedOutput;
use crate::core::collection::{CollectionResult, repository_name};
use crate::core::error::DomainError;
use crate::core::policy::ExcludePatterns;
use crate::core::selector::Selector;

/// Collects files from a local directory root, excluding the output file if located inside it.
///
/// A producer thread walks in sorted order, workers read file bodies concurrently, and the calling
/// thread settles results in that same order, so `Selector` observes candidates in `cmp_path` order.
pub(super) fn collect(
    root: &Path,
    output: &ResolvedOutput,
    exclude: &ExcludePatterns,
) -> Result<CollectionResult, IngestError> {
    let canonical_root = validate_root(root)?;
    let label = repository_name(&canonical_root).to_string();
    let selector = fold_candidates(
        &canonical_root,
        output.file(),
        exclude,
        Selector::new(),
        |selector, observation| Ok(selector.observe(observation)?),
    )?;

    Ok(selector.finish(label, None)?)
}

fn validate_root(root: &Path) -> Result<PathBuf, DomainError> {
    let root_display = root.display();
    let canonical = fs::canonicalize(root)
        .map_err(|error| DomainError::invalid_source(format!("{root_display} ({error})")))?;
    if !canonical.is_dir() {
        let display = canonical.display();
        return Err(DomainError::invalid_source(format!(
            "{display} is not a directory"
        )));
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{DigestFormat, OutputTarget, RunConfig};
    use crate::core::policy::MAX_FILE_SIZE_USIZE;
    use crate::core::source::Source;
    use crate::ingest;
    use crate::shell::github::archive;
    use crate::shell::testing::{gzip_tarball, run_with_timeout, temp_dir};
    use std::io;
    use tar::EntryType;

    fn collect_default(
        root: &Path,
        output: &ResolvedOutput,
    ) -> Result<CollectionResult, IngestError> {
        collect(root, output, &ExcludePatterns::default())
    }

    #[test]
    fn user_exclusions_select_the_same_files_locally_and_from_an_archive()
    -> Result<(), Box<dyn std::error::Error>> {
        let entries: [(&str, &[u8]); 7] = [
            ("src/main.rs", b"fn main() {}"),
            ("src/app.log", b"log"),
            ("build/out.txt", b"out"),
            ("lib/build/x.txt", b"x"),
            ("top", b"anchored"),
            ("lib/top", b"nested"),
            ("data.bin", b"\0\x01"),
        ];
        let exclude = ExcludePatterns::new(["build/", "*.log", "/top"])?;

        let root = temp_dir("differential_exclude")?;
        for (path, body) in entries {
            let file = root.join(path);
            fs::create_dir_all(file.parent().ok_or("entry has a parent")?)?;
            fs::write(file, body)?;
        }
        let local = collect(&root, &ResolvedOutput::STDOUT, &exclude)?;

        let names: Vec<String> = entries
            .iter()
            .map(|(path, _)| format!("repo-root/{path}"))
            .collect();
        let tar_entries: Vec<(&str, &[u8], EntryType)> = names
            .iter()
            .zip(entries)
            .map(|(name, (_, body))| (name.as_str(), body, EntryType::Regular))
            .collect();
        let tarball = gzip_tarball(&tar_entries);
        let remote = archive::collect(tarball.as_slice(), "owner/repo", None, &exclude)?;

        let local_paths: Vec<&str> = local.files.iter().map(|f| f.path).collect();
        let remote_paths: Vec<&str> = remote.files.iter().map(|f| f.path).collect();
        assert_eq!(local_paths, vec!["lib/top", "src/main.rs"]);
        assert_eq!(local_paths, remote_paths);
        assert_eq!(local.skipped, 1, "data.bin is binary");
        assert_eq!(local.skipped, remote.skipped);
        Ok(())
    }

    #[test]
    fn read_local_file_enforces_size_boundaries() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("size_boundaries")?;
        fs::write(root.join("exact.txt"), vec![b'x'; MAX_FILE_SIZE_USIZE])?;
        fs::write(root.join("large.txt"), vec![b'x'; MAX_FILE_SIZE_USIZE + 1])?;

        let res = collect_default(&root, &ResolvedOutput::STDOUT)?;
        assert_eq!(res.files.len(), 1, "exact-size file is included");
        assert_eq!(res.skipped, 1, "one byte over the limit is skipped");
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

        let res = collect_default(&root, &ResolvedOutput::STDOUT)?;
        assert_eq!(res.files.len(), 1, "only the regular target is collected");
        assert_eq!(res.skipped, 0);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "creating a symlink on Windows requires Developer Mode or elevation"]
    fn rejects_windows_symlink_file() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::windows::fs::symlink_file;

        let root = temp_dir("win_symlink_test")?;
        let target = root.join("target.txt");
        let link = root.join("link.txt");
        fs::write(&target, "content")?;
        symlink_file(&target, &link)?;

        let res = collect_default(&root, &ResolvedOutput::STDOUT)?;
        assert_eq!(res.files.len(), 1, "only the regular target is collected");
        assert_eq!(res.skipped, 0);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn fifo_and_socket_are_not_candidates() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::net::UnixListener;

        // Short label: the socket path must fit the platform's `sun_path` limit.
        let root = temp_dir("sf")?;
        fs::write(root.join("a.txt"), "content")?;
        crate::shell::testing::make_fifo(&root.join("pipe"))?;
        let _socket = UnixListener::bind(root.join("sock"))?;

        let dir = root.to_path_buf();
        let res = run_with_timeout(move || collect_default(&dir, &ResolvedOutput::STDOUT))?;

        assert_eq!(res.files.len(), 1, "only the regular file is a candidate");
        assert_eq!(res.skipped, 0, "special files are dropped in the walk");
        Ok(())
    }

    /// Adds a link to a real directory and a link to the parent (`..`), then checks that only the
    /// real file is collected, once. `link_dir` creates a directory link, using the platform's flavor.
    fn assert_directory_links_are_not_followed(
        link_dir: impl Fn(&Path, &Path) -> io::Result<()>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("dir_links")?;
        let real = root.join("real");
        fs::create_dir(&real)?;
        fs::write(real.join("x.txt"), "x")?;
        link_dir(&real, &root.join("link"))?;
        link_dir(Path::new(".."), &root.join("loop"))?;

        let dir = root.to_path_buf();
        let res = run_with_timeout(move || collect_default(&dir, &ResolvedOutput::STDOUT))?;

        let paths: Vec<&str> = res.files.iter().map(|f| f.path).collect();
        assert_eq!(paths, vec!["real/x.txt"]);
        assert_eq!(res.skipped, 0);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_directories_are_not_followed() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;
        assert_directory_links_are_not_followed(|target, link| symlink(target, link))
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "creating a symlink on Windows requires Developer Mode or elevation"]
    fn windows_symlinked_directories_are_not_followed() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::windows::fs::symlink_dir;
        assert_directory_links_are_not_followed(|target, link| symlink_dir(target, link))
    }

    /// Collects through a directory link given as the root. The root is canonicalized, so the walk
    /// runs in the target and the label is the target's basename.
    fn assert_symlinked_root_is_followed(
        link_dir: impl Fn(&Path, &Path) -> io::Result<()>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let base = temp_dir("symlinked_root")?;
        let real = base.join("real_repo");
        fs::create_dir(&real)?;
        fs::write(real.join("x.txt"), "x")?;
        let alias = base.join("alias");
        link_dir(&real, &alias)?;

        let res = run_with_timeout(move || collect_default(&alias, &ResolvedOutput::STDOUT))?;

        let paths: Vec<&str> = res.files.iter().map(|f| f.path).collect();
        assert_eq!(paths, vec!["x.txt"]);
        assert_eq!(
            res.label, "real_repo",
            "label is the canonical target basename"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_root_is_followed() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;
        assert_symlinked_root_is_followed(|target, link| symlink(target, link))
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "creating a symlink on Windows requires Developer Mode or elevation"]
    fn windows_symlinked_root_is_followed() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::windows::fs::symlink_dir;
        assert_symlinked_root_is_followed(|target, link| symlink_dir(target, link))
    }

    #[cfg(unix)]
    #[test]
    fn preserves_distinct_unix_paths() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("path_encoding")?;
        fs::create_dir_all(root.join("a"))?;
        fs::write(root.join("a\\b.txt"), "backslash")?;
        fs::write(root.join("a").join("b.txt"), "slash")?;
        let output = root.join("digest.txt");

        let report = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output.clone()),
            DigestFormat::Text,
        ))?;
        let digest = fs::read_to_string(output)?;

        assert_eq!(report.included_files, 2);
        assert!(digest.contains("FILE: a\\\\b.txt"));
        assert!(digest.contains("FILE: a/b.txt"));
        Ok(())
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn excludes_output_case_insensitively() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("output_case")?;
        fs::write(root.join("source.txt"), "source")?;
        fs::write(root.join("digest.txt"), "previous output")?;
        let output = root.join("DIGEST.TXT");

        let report = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output),
            DigestFormat::Text,
        ))?;
        let digest = fs::read_to_string(root.join("DIGEST.TXT"))?;

        assert_eq!(report.included_files, 1);
        assert!(!digest.contains("previous output"));
        assert!(digest.contains("FILE: source.txt"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directory_fails_fast_with_path_context() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("unreadable_dir")?;
        let locked = ["a_locked", "b_locked"].map(|name| root.join(name));
        for dir in &locked {
            fs::create_dir(dir)?;
            fs::write(dir.join("file.txt"), "content")?;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o0000))?;
        }

        let res = collect_default(&root, &ResolvedOutput::STDOUT);

        // Restore permissions so TempDirGuard can clean up
        for dir in &locked {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o0755));
        }

        let err = res.unwrap_err();
        assert!(
            matches!(&err, IngestError::Io(io_err) if io_err.kind() == std::io::ErrorKind::PermissionDenied),
            "expected IngestError::Io with ErrorKind::PermissionDenied, got: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("a_locked") && !msg.contains("b_locked"),
            "first directory in walk order must be reported alone, got: {msg}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_file_is_counted_as_skipped() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("unreadable_file")?;
        let locked = root.join("a.txt");
        fs::write(&locked, "secret")?;
        fs::write(root.join("b.txt"), "visible")?;
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o0000))?;

        let res = collect_default(&root, &ResolvedOutput::STDOUT)?;
        assert_eq!(res.files.len(), 1);
        assert_eq!(res.skipped, 1);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_gitignore_fails_the_run() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("unreadable_gitignore")?;
        let gitignore = root.join(".gitignore");
        fs::write(&gitignore, "secret.txt\n")?;
        fs::set_permissions(&gitignore, fs::Permissions::from_mode(0o0000))?;

        let res = collect_default(&root, &ResolvedOutput::STDOUT);
        let _ = fs::set_permissions(&gitignore, fs::Permissions::from_mode(0o0644));

        let err = res.unwrap_err();
        assert!(
            matches!(&err, IngestError::Io(io_err) if io_err.kind() == std::io::ErrorKind::PermissionDenied),
            "expected IngestError::Io with ErrorKind::PermissionDenied, got: {err:?}"
        );
        Ok(())
    }

    #[test]
    fn invalid_gitignore_content_fails_the_run() -> Result<(), Box<dyn std::error::Error>> {
        let cases: [(&str, Vec<u8>, std::io::ErrorKind); 2] = [
            (
                "non_utf8_gitignore",
                b"# caf\xe9\nsecret.txt\n".to_vec(),
                std::io::ErrorKind::InvalidData,
            ),
            (
                "oversized_gitignore",
                vec![b'#'; MAX_FILE_SIZE_USIZE + 1],
                std::io::ErrorKind::FileTooLarge,
            ),
        ];
        for (name, content, kind) in cases {
            let root = temp_dir(name)?;
            fs::write(root.join(".gitignore"), content)?;
            fs::write(root.join("secret.txt"), "secret")?;

            let err = collect_default(&root, &ResolvedOutput::STDOUT).unwrap_err();
            assert!(
                matches!(&err, IngestError::Io(io_err) if io_err.kind() == kind),
                "{name}: expected IngestError::Io with {kind:?}, got: {err:?}"
            );
            assert!(err.to_string().contains(".gitignore"), "{name}: {err}");
        }
        Ok(())
    }

    #[test]
    fn gitignore_at_size_limit_is_read_in_full() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("gitignore_at_limit")?;
        // The rule sits on the last line, so it applies only if the whole file is parsed.
        let mut content = vec![b'#'; MAX_FILE_SIZE_USIZE - b"\nsecret.txt".len()];
        content.extend_from_slice(b"\nsecret.txt");
        fs::write(root.join(".gitignore"), content)?;
        fs::write(root.join("secret.txt"), "secret")?;
        fs::write(root.join("a.txt"), "a")?;

        let res = collect_default(&root, &ResolvedOutput::STDOUT)?;
        assert_eq!(
            res.files.len(),
            1,
            "secret.txt is excluded by the last line"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_gitignore_fails_the_run() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let root = temp_dir("symlinked_gitignore")?;
        let rules = root.join("rules.txt");
        fs::write(&rules, "secret.txt\n")?;
        symlink(&rules, root.join(".gitignore"))?;
        fs::write(root.join("secret.txt"), "secret")?;

        let err = collect_default(&root, &ResolvedOutput::STDOUT).unwrap_err();
        assert!(matches!(err, IngestError::Io(_)), "got: {err:?}");
        assert!(err.to_string().contains(".gitignore"), "got: {err}");
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "creating a symlink on Windows requires Developer Mode or elevation"]
    fn symlinked_gitignore_fails_the_run() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::windows::fs::symlink_file;

        let root = temp_dir("win_symlinked_gitignore")?;
        let rules = root.join("rules.txt");
        fs::write(&rules, "secret.txt\n")?;
        fs::write(root.join("secret.txt"), "secret")?;
        symlink_file(&rules, root.join(".gitignore"))?;

        let err = collect_default(&root, &ResolvedOutput::STDOUT).unwrap_err();
        assert!(matches!(err, IngestError::Io(_)), "got: {err:?}");
        assert!(err.to_string().contains(".gitignore"), "got: {err}");
        Ok(())
    }

    #[test]
    fn gitignore_directory_is_not_read_as_rules() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("gitignore_directory")?;
        fs::create_dir(root.join(".gitignore"))?;
        fs::write(root.join("a.txt"), "a")?;

        let res = collect_default(&root, &ResolvedOutput::STDOUT)?;
        assert_eq!(res.files.len(), 1);
        assert_eq!(res.skipped, 0);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn locked_file_is_counted_as_skipped() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::windows::fs::OpenOptionsExt;

        let root = temp_dir("locked_file")?;
        let locked = root.join("a.txt");
        fs::write(&locked, "secret")?;
        fs::write(root.join("b.txt"), "visible")?;
        let _guard = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked)?;

        let res = collect_default(&root, &ResolvedOutput::STDOUT)?;
        assert_eq!(res.files.len(), 1);
        assert_eq!(res.skipped, 1);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn gitignore_fifo_fails_the_run() -> Result<(), Box<dyn std::error::Error>> {
        // A FIFO is neither a regular file nor a directory, so its rules cannot be read. Opening it would
        // block, so the run must fail on its metadata and name `.gitignore`.
        let root = temp_dir("gitignore_fifo")?;
        crate::shell::testing::make_fifo(&root.join(".gitignore"))?;
        fs::write(root.join("a.txt"), "a")?;

        let dir = root.to_path_buf();
        let err =
            run_with_timeout(move || collect_default(&dir, &ResolvedOutput::STDOUT)).unwrap_err();
        assert!(
            matches!(&err, IngestError::Io(io_err) if io_err.kind() == io::ErrorKind::InvalidInput),
            "expected IngestError::Io with ErrorKind::InvalidInput, got: {err:?}"
        );
        assert!(err.to_string().contains(".gitignore"), "got: {err}");
        Ok(())
    }

    #[test]
    fn ingests_local_directory_respects_all_exclusion_policies()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("comprehensive_local_policies")?;
        fs::create_dir_all(root.join(".git"))?;
        fs::create_dir_all(root.join("certs"))?;
        fs::create_dir_all(root.join("sub"))?;

        // 1. Text file: included
        fs::write(root.join("README.md"), "hello")?;

        // 2. Binary file: skipped (increments skipped count)
        fs::write(root.join("binary.bin"), [0, 1, 2])?;

        // 3. Noise files: ignored without incrementing skipped
        fs::write(root.join("Cargo.lock"), "[lockfile]")?;
        fs::write(root.join("poetry.lock"), "[poetry]")?;
        fs::write(root.join("sub").join("package-lock.json"), "{}")?;

        // 4. Sensitive files: ignored without incrementing skipped
        fs::write(root.join(".env"), "SECRET=true")?;
        fs::write(root.join(".env.production"), "KEY=123")?;
        fs::write(root.join("certs").join("privkey.key"), "PRIVATE")?;
        fs::write(root.join("certs").join("cert.pfx"), "CERT")?;
        fs::write(root.join("id_rsa"), "SSH")?;
        fs::write(root.join("id_ecdsa"), "SSH_ECDSA")?;

        // 5. Git metadata: ignored
        fs::write(root.join(".git/config"), "secret")?;

        // 6. Gitignore: ignored without git directory
        fs::write(root.join(".gitignore"), "ignored.txt\n")?;
        fs::write(root.join("ignored.txt"), "ignored")?;

        let output = root.join("digest.txt");
        let report = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output.clone()),
            DigestFormat::Text,
        ))?;
        let digest = fs::read_to_string(output)?;

        assert_eq!(report.included_files, 1);
        assert_eq!(report.skipped_files, 1);
        assert!(digest.contains("FILE: README.md"));
        assert!(!digest.contains("binary.bin"));
        assert!(!digest.contains("Cargo.lock"));
        assert!(!digest.contains("poetry.lock"));
        assert!(!digest.contains("package-lock.json"));
        assert!(!digest.contains(".env"));
        assert!(!digest.contains("privkey.key"));
        assert!(!digest.contains("cert.pfx"));
        assert!(!digest.contains("id_rsa"));
        assert!(!digest.contains("id_ecdsa"));
        assert!(!digest.contains(".git/config"));
        assert!(!digest.contains("ignored.txt"));
        Ok(())
    }

    #[test]
    fn does_not_apply_dot_ignore_files() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("dot_ignore")?;
        fs::write(root.join(".ignore"), "important.txt\n")?;
        fs::write(root.join("important.txt"), "important")?;
        let output = root.join("digest.txt");

        let report = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output.clone()),
            DigestFormat::Text,
        ))?;
        let digest = fs::read_to_string(output)?;

        assert_eq!(report.included_files, 1);
        assert!(digest.contains("FILE: important.txt"));
        Ok(())
    }

    #[test]
    fn label_uses_directory_basename() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("custom_repo_name")?;
        fs::write(root.join("file.txt"), "hello")?;
        let output = root.join("digest.txt");
        let _ = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output.clone()),
            DigestFormat::Text,
        ))?;
        let digest = fs::read_to_string(&output)?;
        let expected_basename = root.file_name().unwrap().to_str().unwrap();
        assert!(digest.starts_with(&format!("Repository: {expected_basename}\n")));
        Ok(())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn skips_non_utf8_paths() -> Result<(), Box<dyn std::error::Error>> {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let root = temp_dir("invalid_paths")?;
        fs::write(root.join(".gitignore"), "!*\n")?;
        fs::create_dir(root.join(".hidden"))?;
        fs::write(
            root.join(".hidden")
                .join(OsString::from_vec(b"c\x82".to_vec())),
            "c",
        )?;
        fs::write(root.join(OsString::from_vec(b"a\x80".to_vec())), "a")?;
        fs::write(root.join(OsString::from_vec(b"b\x81".to_vec())), "b")?;
        let output = root.join("digest.txt");
        let report = ingest(&RunConfig::new(
            Source::Local(root.to_path_buf()),
            OutputTarget::File(output),
            DigestFormat::Text,
        ))?;
        assert_eq!(report.included_files, 0);
        assert_eq!(report.skipped_files, 2);
        Ok(())
    }

    #[test]
    fn hidden_directories_and_files_are_pruned_by_policy() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = temp_dir("hidden_pruning")?;
        fs::create_dir_all(root.join(".hidden_dir"))?;
        fs::write(root.join(".hidden_dir").join("nested.txt"), "nested")?;
        fs::write(root.join(".dotfile"), "secret")?;
        fs::write(root.join("visible.txt"), "visible")?;

        let result = collect_default(&root, &ResolvedOutput::STDOUT)?;
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files.iter().next().unwrap().path, "visible.txt");
        assert_eq!(result.skipped, 0);
        Ok(())
    }

    #[test]
    fn validate_root_rejects_regular_file() -> Result<(), Box<dyn std::error::Error>> {
        let root = temp_dir("root_not_dir")?;
        let file = root.join("file.txt");
        fs::write(&file, "hello")?;
        let err = validate_root(&file).unwrap_err();
        assert!(err.to_string().contains("is not a directory"));
        Ok(())
    }
}
