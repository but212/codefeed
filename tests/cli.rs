use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static CLI_TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

// Note: Integration tests in tests/ cannot access internal pub(super) helpers in src/shell/testing.rs.
// Per AGENTS.md policy, unit tests use shell::testing::temp_dir while integration tests maintain
// this lightweight local fixture.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let pid = std::process::id();
        let path = loop {
            let counter = CLI_TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let candidate =
                std::env::temp_dir().join(format!("codefeed_cli_{label}_{pid}_{counter}"));
            match std::fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(err) => panic!(
                    "failed to create temporary directory {}: {err}",
                    candidate.display()
                ),
            }
        };
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_codefeed"))
        .args(args)
        .output()
        .expect("CLI process should start")
}

#[test]
fn help_exits_successfully_without_ingesting() {
    let cases: &[&[&str]] = &[&["--help"], &[".", "--help"]];
    for args in cases {
        let output = run(args);
        assert!(
            output.status.success(),
            "args: {args:?}, output: {output:?}"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.matches("Usage:").count(), 1, "args: {args:?}");
        assert_eq!(
            stdout.trim(),
            "Usage: codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <text|xml>] [--exclude <PATTERN>]...",
            "args: {args:?}"
        );
        assert!(output.stderr.is_empty(), "args: {args:?}");
    }
}

#[test]
fn invalid_arguments_exit_nonzero() {
    for args in [&[][..], &[".", "--format"][..]] {
        let output = run(args);
        assert!(!output.status.success(), "args: {args:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("error:"));
    }
}

#[test]
fn usage_error_formatting_does_not_prefix_invalid_source() {
    let output = run(&[".", "--unknown"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unexpected argument '--unknown' found")
            || stderr.contains("unknown option: --unknown")
    );
    assert!(!stderr.contains("error: invalid source: unknown option"));
}

#[test]
fn creates_default_output_without_subcommand() -> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new("default");
    std::fs::write(root.path.join("visible.txt"), "visible")?;

    let output = Command::new(env!("CARGO_BIN_EXE_codefeed"))
        .current_dir(&root.path)
        .arg(".")
        .output()?;

    assert!(output.status.success(), "output: {output:?}");
    assert!(root.path.join("digest.txt").is_file());
    Ok(())
}

#[test]
fn writes_digest_only_to_stdout_for_dash_output() -> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new("stdout");
    std::fs::write(root.path.join("visible.txt"), "visible")?;

    let output = Command::new(env!("CARGO_BIN_EXE_codefeed"))
        .current_dir(&root.path)
        .arg(".")
        .arg("--output")
        .arg("-")
        .output()?;

    assert!(output.status.success(), "output: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("FILE: visible.txt"), "stdout: {stdout}");
    assert!(!stdout.contains("Wrote"), "stdout: {stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Wrote to stdout (1 files, 0 skipped, ~"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("tokens)"), "stderr: {stderr}");
    assert!(
        !root.path.join("digest.txt").exists(),
        "no output file expected"
    );
    Ok(())
}

#[test]
fn reports_estimated_tokens_in_file_summary() -> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new("tokens");
    std::fs::write(root.path.join("code.rs"), "pub fn test() -> i32 { 42 }")?;

    let output = Command::new(env!("CARGO_BIN_EXE_codefeed"))
        .current_dir(&root.path)
        .arg(".")
        .output()?;

    assert!(output.status.success(), "output: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Wrote digest.txt (1 files, 0 skipped, ~"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("tokens)"), "stdout: {stdout}");
    Ok(())
}

#[test]
fn exclude_flag_drops_matching_paths_from_the_digest() -> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new("exclude");
    std::fs::create_dir(root.path.join("build"))?;
    std::fs::write(root.path.join("build").join("out.txt"), "generated")?;
    std::fs::write(root.path.join("app.log"), "log line")?;
    std::fs::write(root.path.join("visible.txt"), "visible")?;

    let output = Command::new(env!("CARGO_BIN_EXE_codefeed"))
        .current_dir(&root.path)
        .arg(".")
        .args(["-x", "build/", "--exclude=*.log", "-o", "-"])
        .output()?;

    assert!(output.status.success(), "output: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("FILE: visible.txt"), "stdout: {stdout}");
    assert!(!stdout.contains("generated"), "stdout: {stdout}");
    assert!(!stdout.contains("log line"), "stdout: {stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Wrote to stdout (1 files, 0 skipped, ~"),
        "stderr: {stderr}"
    );
    Ok(())
}

#[test]
fn exclude_invalid_pattern_is_a_usage_error() {
    let output = run(&[".", "-x", "["]);
    assert_eq!(output.status.code(), Some(1), "output: {output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: invalid value '[' for '--exclude <PATTERN>'"),
        "stderr: {stderr}"
    );
}

#[test]
fn outputs_xml_when_format_xml_specified() -> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new("xml");
    std::fs::write(root.path.join("file.rs"), "fn main() { let x = 1 < 2; }")?;

    let output = Command::new(env!("CARGO_BIN_EXE_codefeed"))
        .current_dir(&root.path)
        .arg(".")
        .arg("--output")
        .arg("-")
        .arg("--format")
        .arg("xml")
        .output()?;

    assert!(output.status.success(), "output: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("<repository name="), "stdout: {stdout}");
    assert!(stdout.contains("<directory_structure>"), "stdout: {stdout}");
    assert!(
        stdout.contains("<file path=\"file.rs\">"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("1 &lt; 2"), "stdout: {stdout}");
    assert!(stdout.contains("</repository>"), "stdout: {stdout}");
    Ok(())
}

#[cfg(unix)]
#[test]
fn help_after_non_utf8_source_exits_successfully() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let source = OsString::from_vec(b"source-\xff".to_vec());
    let output = Command::new(env!("CARGO_BIN_EXE_codefeed"))
        .arg(source)
        .arg("--help")
        .output()
        .expect("CLI process should start");

    assert!(output.status.success(), "output: {output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "Usage: codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <text|xml>] [--exclude <PATTERN>]..."
    );
    assert!(output.stderr.is_empty(), "output: {output:?}");
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn accepts_non_utf8_local_source_path() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let parent = TempDir::new("non_utf8");
    let source = parent
        .path
        .join(OsString::from_vec(b"source-\xff".to_vec()));
    let output = source.join("digest.txt");
    std::fs::create_dir_all(&source).expect("source directory should be created");
    std::fs::write(source.join("visible.txt"), "visible").expect("source file should be written");

    let result = Command::new(env!("CARGO_BIN_EXE_codefeed"))
        .arg(&source)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("CLI process should start");

    assert!(result.status.success(), "output: {result:?}");
    assert!(output.is_file());
}

#[test]
fn text_and_xml_stdout_bytes_are_stable() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new("golden");
    let root = temp.path.join("fixture");
    std::fs::create_dir_all(root.join("src/nested/deep"))?;
    std::fs::create_dir_all(root.join("bin"))?;
    std::fs::write(root.join("README.md"), "# fixture\n")?;
    std::fs::write(root.join("src/lib.rs"), "pub fn lib() {}\n")?;
    std::fs::write(root.join("src/nested/deep/mod.rs"), "pub mod deep;\n")?;
    std::fs::write(root.join("bin/tool.bin"), [0u8, 1, 2])?;
    std::fs::write(root.join(".hidden"), "secret\n")?;
    std::fs::write(root.join("Cargo.lock"), "[lockfile]\n")?;
    let root = root.to_str().expect("UTF-8 temp path");

    let expected_text = r"Repository: fixture
Files: 3
Skipped: 1

Directory structure:
├── README.md
└── src/
    ├── lib.rs
    └── nested/
        └── deep/
            └── mod.rs

Files:

==================================================
FILE: README.md
==================================================

# fixture

==================================================
FILE: src/lib.rs
==================================================

pub fn lib() {}

==================================================
FILE: src/nested/deep/mod.rs
==================================================

pub mod deep;

";
    let text = run(&[root, "-o", "-", "-f", "text"]);
    assert!(text.status.success(), "output: {text:?}");
    assert_eq!(text.stdout, expected_text.as_bytes());

    let expected_xml = r#"<repository name="fixture">
<directory_structure>
├── README.md
└── src/
    ├── lib.rs
    └── nested/
        └── deep/
            └── mod.rs
</directory_structure>
<files>
<file path="README.md">
# fixture
</file>
<file path="src/lib.rs">
pub fn lib() {}
</file>
<file path="src/nested/deep/mod.rs">
pub mod deep;
</file>
</files>
</repository>
"#;
    let xml = run(&[root, "-o", "-", "-f", "xml"]);
    assert!(xml.status.success(), "output: {xml:?}");
    assert_eq!(xml.stdout, expected_xml.as_bytes());
    Ok(())
}

#[test]
fn file_output_bytes_match_stdout_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new("file_matches_stdout");
    let root = temp.path.join("fixture");
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(root.join("src/lib.rs"), "pub fn lib() {}\n")?;
    let root = root.to_str().expect("UTF-8 temp path");
    let output = temp.path.join("digest.txt");
    let output = output.to_str().expect("UTF-8 temp path");

    let stdout = run(&[root, "-o", "-"]);
    assert!(stdout.status.success(), "output: {stdout:?}");
    let file = run(&[root, "-o", output]);
    assert!(file.status.success(), "output: {file:?}");
    assert_eq!(std::fs::read(output)?, stdout.stdout);
    Ok(())
}

#[test]
fn ingest_fails_fast_on_directory_output_target() {
    let output = Command::new(env!("CARGO_BIN_EXE_codefeed"))
        .args(["--output", ".", "."])
        .output()
        .expect("CLI process should start");

    assert!(!output.status.success(), "output: {output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("output path has no filename")
            || stderr.contains("output path is a directory")
            || stderr.contains("Is a directory"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn closed_output_pipes_do_not_panic() {
    let fixture = TempDir::new("closed_pipe");
    std::fs::write(fixture.path.join("a.txt"), "hello\n").expect("fixture should be written");
    let fixture_arg = fixture.path.to_str().expect("temp path should be UTF-8");

    // Each run closes both pipes before the CLI writes, so every write sees a broken pipe.
    let cases: [(&[&str], i32); 2] = [(&[fixture_arg, "-o", "-"], 0), (&["--bogus"], 1)];
    for (args, expected_code) in cases {
        let mut child = Command::new(env!("CARGO_BIN_EXE_codefeed"))
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("CLI process should start");
        drop(child.stdout.take());
        drop(child.stderr.take());
        let status = child.wait().expect("CLI process should exit");
        assert_eq!(status.code(), Some(expected_code), "args={args:?}");
    }
}
