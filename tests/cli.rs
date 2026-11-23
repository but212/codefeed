use std::process::{Command, Stdio};

#[path = "../src/shell/testing/temp_dir.rs"]
mod temp_dir;
use temp_dir::TempDirGuard;

fn temp_dir(label: &str) -> TempDirGuard {
    TempDirGuard::create_in(&std::env::temp_dir(), label).expect("create CLI fixture directory")
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_codefeed"))
        .args(args)
        .output()
        .expect("CLI process should start")
}

#[test]
fn help_and_version_exit_successfully_without_ingesting() {
    let expected_help = "Usage: codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <text|xml>] [--exclude <PATTERN>]...\n";
    let expected_version = format!("codefeed {}\n", env!("CARGO_PKG_VERSION"));
    let cases: [(&[&str], &str); 2] = [
        (&[".", "--help"], expected_help),
        (&[".", "-V"], &expected_version),
    ];
    for (args, expected_stdout) in cases {
        let dir = temp_dir("meta_action");
        let output = Command::new(env!("CARGO_BIN_EXE_codefeed"))
            .args(args)
            .current_dir(&dir)
            .output()
            .expect("CLI process should start");
        assert!(
            output.status.success(),
            "args: {args:?}, output: {output:?}"
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            expected_stdout,
            "args: {args:?}"
        );
        assert!(output.stderr.is_empty(), "args: {args:?}");
        assert!(!dir.join("digest.txt").exists(), "args: {args:?}");
    }
}

#[test]
fn invalid_invocation_exits_one_with_an_error_line() {
    let output = run(&[".", "--unknown"]);
    assert_eq!(output.status.code(), Some(1), "output: {output:?}");
    assert!(output.stdout.is_empty(), "output: {output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: unknown option: --unknown"),
        "stderr: {stderr}"
    );
    assert!(
        !stderr.contains("error: invalid source: unknown option"),
        "stderr: {stderr}"
    );
}

#[test]
fn default_file_and_dash_stdout_outputs_match() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_dir("default_and_stdout");
    std::fs::write(root.join("visible.txt"), "visible")?;

    let dash_output = Command::new(env!("CARGO_BIN_EXE_codefeed"))
        .current_dir(&root)
        .args([".", "--output", "-"])
        .output()?;

    assert!(dash_output.status.success(), "output: {dash_output:?}");
    let dash_stdout = String::from_utf8_lossy(&dash_output.stdout);
    assert!(
        dash_stdout.contains("FILE: visible.txt"),
        "stdout: {dash_stdout}"
    );
    assert!(!dash_stdout.contains("Wrote"), "stdout: {dash_stdout}");
    let dash_stderr = String::from_utf8_lossy(&dash_output.stderr);
    assert!(
        dash_stderr.contains("Wrote to stdout (1 files, 0 skipped, ~"),
        "stderr: {dash_stderr}"
    );
    assert!(!root.join("digest.txt").exists(), "no output file expected");

    let file_output = Command::new(env!("CARGO_BIN_EXE_codefeed"))
        .current_dir(&root)
        .arg(".")
        .output()?;

    assert!(file_output.status.success(), "output: {file_output:?}");
    assert!(root.join("digest.txt").is_file());
    // File mode reports its summary on stdout; stream mode uses stderr.
    let file_stdout = String::from_utf8_lossy(&file_output.stdout);
    assert!(
        file_stdout.contains("Wrote digest.txt (1 files, 0 skipped, ~"),
        "stdout: {file_stdout}"
    );
    assert_eq!(std::fs::read(root.join("digest.txt"))?, dash_output.stdout);
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

#[test]
fn text_and_xml_stdout_bytes_are_stable() -> Result<(), Box<dyn std::error::Error>> {
    let temp = temp_dir("golden");
    let root = temp.join("fixture");
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
fn closed_output_pipes_do_not_panic() {
    let fixture = temp_dir("closed_pipe");
    std::fs::write(fixture.join("a.txt"), "hello\n").expect("fixture should be written");
    let fixture_arg = fixture.to_str().expect("temp path should be UTF-8");

    // The read ends are dropped before spawn, so every child write sees a broken pipe.
    let cases: [(&[&str], i32); 2] = [(&[fixture_arg, "-o", "-"], 0), (&["--bogus"], 1)];
    for (args, expected_code) in cases {
        let (stdout_reader, stdout_writer) = std::io::pipe().expect("stdout pipe");
        let (stderr_reader, stderr_writer) = std::io::pipe().expect("stderr pipe");
        drop(stdout_reader);
        drop(stderr_reader);
        let status = Command::new(env!("CARGO_BIN_EXE_codefeed"))
            .args(args)
            .stdout(Stdio::from(stdout_writer))
            .stderr(Stdio::from(stderr_writer))
            .status()
            .expect("CLI process should run");
        assert_eq!(status.code(), Some(expected_code), "args={args:?}");
    }
}
