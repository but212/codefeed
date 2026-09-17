use super::collect::{github, local};
use super::decide::{Item, ItemPath, Probe, Selector};
use super::model::MAX_FILE_COUNT;
use super::*;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn parses_public_github_repository() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        parse_source("https://github.com/rust-lang/cargo.git")?,
        Source::GitHub {
            owner: "rust-lang".into(),
            repo: "cargo".into(),
        }
    );
    Ok(())
}

#[test]
fn rejects_unsupported_repository_url() {
    assert!(parse_source("git@github.com:owner/repo.git").is_err());
    assert!(parse_source("https://gitlab.com/owner/repo").is_err());
    assert!(parse_source("https://github.com/owner/repo/issues").is_err());
}

#[test]
fn rejects_dot_path_segments_in_repository_urls() {
    assert!(parse_source("https://github.com/owner/..").is_err());
    assert!(parse_source("https://github.com/owner/.").is_err());
    assert!(parse_source("https://github.com/./repo").is_err());
    assert!(parse_source("https://github.com/../repo").is_err());
}

#[test]
fn rejects_invalid_repository_url_ports() -> Result<(), Box<dyn std::error::Error>> {
    assert!(parse_source("https://github.com:99999/owner/repo").is_err());
    assert!(parse_source("https://github.com:abc/owner/repo").is_err());
    let expected = Source::GitHub {
        owner: "owner".into(),
        repo: "repo".into(),
    };
    assert_eq!(parse_source("https://github.com:443/owner/repo")?, expected);
    assert_eq!(parse_source("https://github.com:/owner/repo")?, expected);
    Ok(())
}

#[test]
fn renders_files_in_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
    let files = vec![
        digest::FileRecord {
            path: "z.txt".into(),
            content: "z".into(),
        },
        digest::FileRecord {
            path: "a.txt".into(),
            content: "a".into(),
        },
    ];
    let digest = digest::render_digest("repo", Some("abc"), &files, 1)?;
    let first = digest
        .find("FILE: a.txt")
        .ok_or_else(|| std::io::Error::other("a.txt is missing"))?;
    let second = digest
        .find("FILE: z.txt")
        .ok_or_else(|| std::io::Error::other("z.txt is missing"))?;
    assert!(first < second);
    assert!(digest.contains("Revision: abc"));
    assert!(digest.contains("Skipped: 1"));
    Ok(())
}

#[test]
fn escapes_revision_header() -> Result<(), Box<dyn std::error::Error>> {
    let revision = "feature\\x\r\nFiles: 9";
    let rendered = digest::render_digest("repo", Some(revision), &[], 0)?;

    assert_eq!(
        rendered.lines().nth(1),
        Some("Revision: feature\\\\x\\r\\nFiles: 9")
    );
    assert!(!rendered.contains(revision));
    Ok(())
}

#[test]
fn digest_budget_matches_rendered_size() -> Result<(), Box<dyn std::error::Error>> {
    let files = vec![
        digest::FileRecord {
            path: "z.txt".into(),
            content: "z\n".into(),
        },
        digest::FileRecord {
            path: "a.txt".into(),
            content: "a".into(),
        },
    ];
    for revision in ["abc", "feature\\x\r\nFiles: 9"] {
        let mut budget = digest::DigestBudget::new("repo", Some(revision))?;
        for file in &files {
            budget.add(file)?;
        }
        budget.skip()?;
        let rendered = digest::render_digest("repo", Some(revision), &files, budget.skipped())?;

        assert_eq!(
            budget.size(),
            Some(rendered.len() as u64),
            "revision: {revision:?}"
        );
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn preserves_distinct_unix_paths() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_dir("path_encoding")?;
    fs::create_dir_all(root.join("a"))?;
    fs::write(root.join("a\\b.txt"), "backslash")?;
    fs::write(root.join("a").join("b.txt"), "slash")?;
    let output = root.join("digest.txt");

    let report = ingest(
        &Source::Local(root.clone()),
        &Options {
            output: output.clone(),
            reference: None,
        },
    )?;
    let digest = fs::read_to_string(output)?;

    assert_eq!(report.included_files, 2);
    assert!(digest.contains("FILE: a\\\\b.txt"));
    assert!(digest.contains("FILE: a/b.txt"));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn excludes_output_through_nonexistent_parent_alias() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_dir("output_alias")?;
    fs::create_dir_all(&root)?;
    fs::write(root.join("source.txt"), "source")?;
    fs::write(root.join("digest.txt"), "previous output")?;
    let output = root.join("missing").join("..").join("digest.txt");

    let report = ingest(
        &Source::Local(root.clone()),
        &Options {
            output,
            reference: None,
        },
    )?;
    let digest = fs::read_to_string(root.join("digest.txt"))?;

    assert_eq!(report.included_files, 1);
    assert!(!digest.contains("previous output"));
    assert!(digest.contains("FILE: source.txt"));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn ingests_local_text_and_skips_binary_and_git_directory() -> Result<(), Box<dyn std::error::Error>>
{
    let root = temp_dir("local")?;
    fs::create_dir_all(root.join(".git"))?;
    fs::write(root.join("README.md"), "hello")?;
    fs::write(root.join("binary.bin"), [0, 1, 2])?;
    fs::write(root.join(".git/config"), "secret")?;
    fs::write(root.join(".gitignore"), "ignored.txt\n")?;
    fs::write(root.join("ignored.txt"), "ignored")?;
    let output = root.join("digest.txt");

    let report = ingest(
        &Source::Local(root.clone()),
        &Options {
            output: output.clone(),
            reference: None,
        },
    )?;
    let digest = fs::read_to_string(output)?;
    assert_eq!(report.included_files, 1);
    assert_eq!(report.skipped_files, 1);
    assert!(digest.contains("FILE: README.md"));
    assert!(!digest.contains("binary.bin"));
    assert!(!digest.contains(".git/config"));
    assert!(!digest.contains("ignored.txt"));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn rejects_digest_before_storing_over_budget_content() {
    let candidate = digest::FileRecord {
        path: "large.txt".into(),
        content: "x".repeat(MAX_OUTPUT_SIZE as usize),
    };
    assert!(matches!(
        digest::render_digest("repo", None, &[candidate], 0),
        Err(IngestError::OutputTooLarge)
    ));
}

#[cfg(unix)]
#[test]
fn does_not_follow_local_symlink() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::symlink;

    let root = temp_dir("symlink")?;
    fs::create_dir_all(&root)?;
    let target = root.join("outside.txt");
    let link = root.join("link.txt");
    fs::write(&target, "outside")?;
    symlink(&target, &link)?;

    assert!(matches!(
        local::read_local_file(&root, &link)?,
        Probe::Missing
    ));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn does_not_block_when_candidate_becomes_fifo() -> Result<(), Box<dyn std::error::Error>> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    let root = temp_dir("fifo")?;
    fs::create_dir_all(&root)?;
    let path = root.join("candidate");
    let c_path = CString::new(path.as_os_str().as_bytes())?;
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

    let (sender, receiver) = mpsc::channel();
    let thread_path = path.clone();
    let thread_root = root.clone();
    let worker = thread::spawn(move || {
        let result = local::read_local_file(&thread_root, &thread_path);
        sender.send(result).expect("result receiver should exist");
    });
    let result = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("FIFO candidate must not block local ingestion")?;

    assert!(matches!(result, Probe::Missing));
    worker.join().expect("file reader thread should finish");
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn bounds_local_file_reads() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_dir("bounded_read")?;
    fs::create_dir_all(&root)?;
    let path = root.join("large.txt");
    fs::write(&path, vec![b'x'; MAX_FILE_SIZE as usize + 1])?;

    assert!(matches!(
        local::read_local_file(&root, &path)?,
        Probe::TooLarge
    ));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn includes_file_at_exact_size_limit() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_dir("exact_size")?;
    fs::create_dir_all(&root)?;
    let path = root.join("exact.txt");
    fs::write(&path, vec![b'x'; MAX_FILE_SIZE as usize])?;

    assert!(matches!(
        local::read_local_file(&root, &path)?,
        Probe::Bytes(_)
    ));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn applies_gitignore_without_git_directory() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_dir("ignore_without_git")?;
    fs::create_dir_all(&root)?;
    fs::write(root.join(".gitignore"), "ignored.txt\n")?;
    fs::write(root.join("kept.txt"), "kept")?;
    fs::write(root.join("ignored.txt"), "ignored")?;
    let output = root.join("digest.txt");

    let report = ingest(
        &Source::Local(root.clone()),
        &Options {
            output: output.clone(),
            reference: None,
        },
    )?;
    let digest = fs::read_to_string(output)?;
    assert_eq!(report.included_files, 1);
    assert!(digest.contains("FILE: kept.txt"));
    assert!(!digest.contains("ignored.txt"));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn does_not_apply_dot_ignore_files() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_dir("dot_ignore")?;
    fs::create_dir_all(&root)?;
    fs::write(root.join(".ignore"), "important.txt\n")?;
    fs::write(root.join("important.txt"), "important")?;
    let output = root.join("digest.txt");

    let report = ingest(
        &Source::Local(root.clone()),
        &Options {
            output: output.clone(),
            reference: None,
        },
    )?;
    let digest = fs::read_to_string(output)?;

    assert_eq!(report.included_files, 1);
    assert!(digest.contains("FILE: important.txt"));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn rejects_remote_ignore_budget_overflow() {
    let mut budget = github::RemoteIgnoreBudget::default();
    for _ in 0..MAX_REMOTE_IGNORE_FILES {
        budget.reserve_file().unwrap();
    }
    assert!(matches!(
        budget.reserve_file(),
        Err(IngestError::InvalidMetadata(message)) if message.contains(".gitignore files")
    ));

    let mut budget = github::RemoteIgnoreBudget::default();
    budget
        .record_bytes(MAX_REMOTE_IGNORE_BYTES as usize)
        .unwrap();
    assert!(budget.read_limit().is_err());
}

#[test]
fn rejects_oversized_remote_ignore_body() {
    let body = github::classify_body(vec![0; MAX_FILE_SIZE as usize + 1], MAX_FILE_SIZE);
    assert!(matches!(
        github::remote_ignore_body(".gitignore", body),
        Err(IngestError::InvalidMetadata(message)) if message.contains(".gitignore")
    ));
}

#[test]
fn matches_remote_gitignore_hierarchy() {
    fn rule(base: &str, pattern: &str) -> github::RemoteIgnoreRule {
        let base = Path::new(REMOTE_ROOT).join(base);
        let mut builder = ignore::gitignore::GitignoreBuilder::new(&base);
        builder.add_line(None, pattern).unwrap();
        github::RemoteIgnoreRule {
            base,
            matcher: builder.build().unwrap(),
        }
    }

    let rules = vec![
        rule("", "target/"),
        rule("", "*.log"),
        rule("sub", "!keep.log"),
        rule("", "ignored/"),
        rule("ignored", "!keep.txt"),
    ];
    assert!(github::ignored_remote_path("target/debug/app", &rules));
    assert!(!github::ignored_remote_path("sub/keep.log", &rules));
    assert!(github::ignored_remote_path("sub/drop.log", &rules));
    assert!(github::ignored_remote_path("other/keep.log", &rules));
    assert!(github::ignored_remote_path("ignored/keep.txt", &rules));
}

#[test]
fn loads_ignored_nested_gitignore_when_parent_is_visible() {
    fn rule(base: &str, pattern: &str) -> github::RemoteIgnoreRule {
        let base = Path::new(REMOTE_ROOT).join(base);
        let mut builder = ignore::gitignore::GitignoreBuilder::new(&base);
        builder.add_line(None, pattern).unwrap();
        github::RemoteIgnoreRule {
            base,
            matcher: builder.build().unwrap(),
        }
    }

    let file_is_ignored = vec![rule("", "sub/.gitignore")];
    assert!(github::remote_ignore_file_is_reachable(
        "sub/.gitignore",
        &file_is_ignored
    ));

    let rules = vec![rule("", "sub/.gitignore"), rule("sub", "secret.txt")];
    assert!(github::ignored_remote_path("sub/secret.txt", &rules));

    let parent_is_ignored = vec![rule("", "sub/")];
    assert!(!github::remote_ignore_file_is_reachable(
        "sub/.gitignore",
        &parent_is_ignored
    ));
}

#[test]
fn rejects_non_regular_git_blobs() {
    let symlink = github::TreeEntry {
        path: "link".into(),
        mode: "120000".into(),
        entry_type: "blob".into(),
        size: Some(4),
    };
    let executable = github::TreeEntry {
        path: "run.sh".into(),
        mode: "100755".into(),
        entry_type: "blob".into(),
        size: Some(4),
    };
    assert!(!github::is_regular_blob(&symlink));
    assert!(github::is_regular_blob(&executable));
}

#[test]
fn counts_candidate_before_content_filtering() {
    let mut candidates = crate::CandidateBudget::default();
    for _ in 0..MAX_FILE_COUNT {
        candidates.add().unwrap();
    }
    assert!(candidates.add().is_err());
}

#[test]
fn distinguishes_complete_and_oversized_bodies() {
    assert!(matches!(
        github::classify_body(vec![0; 8], 8),
        github::LimitedBody::Complete(body) if body.len() == 8
    ));
    assert!(matches!(
        github::classify_body(vec![0; 9], 8),
        github::LimitedBody::Exceeded
    ));
}

#[test]
fn percent_encodes_url_path_segments() {
    assert_eq!(github::encode_path_segment("AZaz09-._~"), "AZaz09-._~");
    assert_eq!(github::encode_path_segment("feature/x"), "feature%2Fx");
    assert_eq!(github::encode_path_segment("a b"), "a%20b");
    assert_eq!(github::encode_path_segment("100%"), "100%25");
    assert_eq!(github::encode_path_segment("a#b?c"), "a%23b%3Fc");
    assert_eq!(github::encode_path_segment("café"), "caf%C3%A9");
}

#[test]
fn builds_github_api_and_raw_urls() {
    assert_eq!(
        github::api_url(&["repos", "rust-lang", "cargo", "commits", "master"]),
        "https://api.github.com/repos/rust-lang/cargo/commits/master"
    );
    assert_eq!(
        github::raw_url("rust-lang", "cargo", "abc123", "src/main.rs"),
        "https://raw.githubusercontent.com/rust-lang/cargo/abc123/src/main.rs"
    );
    assert_eq!(
        github::raw_url("o", "r", "feature/x", "dir/a b.txt"),
        "https://raw.githubusercontent.com/o/r/feature%2Fx/dir/a%20b.txt"
    );
}

#[cfg(unix)]
#[test]
fn skips_non_utf8_paths() -> Result<(), Box<dyn std::error::Error>> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let root = temp_dir("invalid_paths")?;
    fs::create_dir_all(&root)?;
    fs::write(root.join(OsString::from_vec(b"a\x80".to_vec())), "a")?;
    fs::write(root.join(OsString::from_vec(b"b\x81".to_vec())), "b")?;
    let output = root.join("digest.txt");
    let report = ingest(
        &Source::Local(root.clone()),
        &Options {
            output,
            reference: None,
        },
    )?;
    assert_eq!(report.included_files, 0);
    assert_eq!(report.skipped_files, 2);
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn atomic_write_ignores_stale_temporary_file() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_dir("atomic")?;
    fs::create_dir_all(&root)?;
    let output = root.join("digest.txt");
    fs::write(root.join(".digest.txt.123.tmp"), "stale")?;
    output::write_atomic(&output, b"new")?;
    assert_eq!(fs::read(&output)?, b"new");
    assert_eq!(
        fs::read_to_string(root.join(".digest.txt.123.tmp"))?,
        "stale"
    );
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn preserves_previous_output_on_atomic_failure() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_dir("atomic_failure")?;
    fs::create_dir_all(&root)?;
    let result = output::write_atomic(&root, b"new");
    assert!(result.is_err());
    assert!(root.is_dir());
    fs::remove_dir_all(root)?;
    Ok(())
}

fn decide_item(path: &str, probe: Probe) -> Item {
    Item {
        path: ItemPath::Utf8(path.to_owned()),
        probe,
    }
}

#[test]
fn decide_includes_text_candidate() -> Result<(), Box<dyn std::error::Error>> {
    let mut selector = Selector::new("repo", None)?;
    selector.decide(decide_item("a.txt", Probe::Bytes(b"hello".to_vec())))?;
    let (files, skipped) = selector.into_parts();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "a.txt");
    assert_eq!(skipped, 0);
    Ok(())
}

#[test]
fn decide_skips_binary_candidate() -> Result<(), Box<dyn std::error::Error>> {
    let mut selector = Selector::new("repo", None)?;
    selector.decide(decide_item("bin", Probe::Bytes(vec![0, 1, 2])))?;
    selector.decide(decide_item("bad", Probe::Bytes(vec![0xff])))?;
    let (files, skipped) = selector.into_parts();
    assert!(files.is_empty());
    assert_eq!(skipped, 2);
    Ok(())
}

#[test]
fn decide_handles_size_boundary() -> Result<(), Box<dyn std::error::Error>> {
    let mut selector = Selector::new("repo", None)?;
    selector.decide(decide_item("big", Probe::TooLarge))?;
    selector.decide(decide_item("gone", Probe::Missing))?;
    selector.decide(decide_item(
        "exact.txt",
        Probe::Bytes(vec![b'x'; MAX_FILE_SIZE as usize]),
    ))?;
    let (files, skipped) = selector.into_parts();
    assert_eq!(files.len(), 1);
    assert_eq!(skipped, 2);
    Ok(())
}

#[test]
fn github_ignored_is_not_skipped() -> Result<(), Box<dyn std::error::Error>> {
    let mut selector = Selector::new("repo", None)?;
    for _ in 0..MAX_FILE_COUNT {
        selector.decide(decide_item("ignored.txt", Probe::Ignored))?;
    }
    selector.decide(decide_item("kept.txt", Probe::Bytes(b"kept".to_vec())))?;
    let (files, skipped) = selector.into_parts();
    assert_eq!(files.len(), 1);
    assert_eq!(skipped, 0);
    Ok(())
}

#[test]
fn decide_counts_candidate_before_content() -> Result<(), Box<dyn std::error::Error>> {
    let mut selector = Selector::new("repo", None)?;
    for index in 0..MAX_FILE_COUNT {
        selector.decide(decide_item(
            &format!("binary-{index}"),
            Probe::Bytes(vec![0]),
        ))?;
    }
    assert!(matches!(
        selector.decide(decide_item("extra", Probe::Bytes(b"ok".to_vec()))),
        Err(IngestError::InvalidMetadata(_))
    ));
    Ok(())
}

#[test]
fn decide_rejects_output_overflow() -> Result<(), Box<dyn std::error::Error>> {
    let mut selector = Selector::new("repo", None)?;
    let result = selector.decide(decide_item(
        "large.txt",
        Probe::Bytes(vec![b'x'; MAX_OUTPUT_SIZE as usize]),
    ));
    assert!(matches!(result, Err(IngestError::OutputTooLarge)));
    Ok(())
}

fn temp_dir(label: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    Ok(env::temp_dir().join(format!("local_ingest_{label}_{nanos}")))
}
