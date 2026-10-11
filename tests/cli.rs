use std::process::Command;

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
            "Usage: codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <FORMAT>]",
            "args: {args:?}"
        );
        assert!(output.stderr.is_empty(), "args: {args:?}");
    }
}

#[test]
fn invalid_arguments_exit_nonzero() {
    for args in [
        &[][..],
        &[".", "--unknown"][..],
        &[".", "--output"][..],
        &[".", "--output", "--help"][..],
        &[".", "--ref"][..],
        &[".", "--ref", "main"][..],
        &[".", "--format"][..],
        &[".", "--format", "invalid"][..],
        &["ingest", "."][..],
    ] {
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
    assert!(stderr.contains("error: unknown option: --unknown"));
    assert!(!stderr.contains("error: invalid source: unknown option"));
}

#[test]
fn creates_default_output_without_subcommand() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("codefeed_cli_default_{suffix}"));
    std::fs::create_dir_all(&root).expect("temporary directory should be created");
    std::fs::write(root.join("visible.txt"), "visible").expect("source file should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_codefeed"))
        .current_dir(&root)
        .arg(".")
        .output()
        .expect("CLI process should start");

    assert!(output.status.success(), "output: {output:?}");
    assert!(root.join("digest.txt").is_file());
    std::fs::remove_dir_all(root).expect("temporary directory should be removed");
    Ok(())
}

#[test]
fn writes_digest_only_to_stdout_for_dash_output() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("codefeed_cli_stdout_{suffix}"));
    std::fs::create_dir_all(&root).expect("temporary directory should be created");
    std::fs::write(root.join("visible.txt"), "visible").expect("source file should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_codefeed"))
        .current_dir(&root)
        .arg(".")
        .arg("--output")
        .arg("-")
        .output()
        .expect("CLI process should start");

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
    assert!(!root.join("digest.txt").exists(), "no output file expected");
    std::fs::remove_dir_all(root).expect("temporary directory should be removed");
    Ok(())
}

#[test]
fn reports_estimated_tokens_in_file_summary() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("codefeed_cli_tokens_{suffix}"));
    std::fs::create_dir_all(&root).expect("temporary directory should be created");
    std::fs::write(root.join("code.rs"), "pub fn test() -> i32 { 42 }")
        .expect("source file should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_codefeed"))
        .current_dir(&root)
        .arg(".")
        .output()
        .expect("CLI process should start");

    assert!(output.status.success(), "output: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Wrote digest.txt (1 files, 0 skipped, ~"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("tokens)"), "stdout: {stdout}");
    std::fs::remove_dir_all(root).expect("temporary directory should be removed");
    Ok(())
}

#[test]
fn outputs_xml_when_format_xml_specified() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("codefeed_cli_xml_{suffix}"));
    std::fs::create_dir_all(&root).expect("temporary directory should be created");
    std::fs::write(root.join("file.rs"), "fn main() { let x = 1 < 2; }")
        .expect("file should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_codefeed"))
        .current_dir(&root)
        .arg(".")
        .arg("--output")
        .arg("-")
        .arg("--format")
        .arg("xml")
        .output()
        .expect("CLI process should start");

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
    std::fs::remove_dir_all(root).expect("temporary directory should be removed");
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
        "Usage: codefeed <SOURCE> [--output <PATH|->] [--ref <REF>] [--format <FORMAT>]"
    );
    assert!(output.stderr.is_empty(), "output: {output:?}");
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn accepts_non_utf8_local_source_path() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the Unix epoch")
        .as_nanos();
    let parent = std::env::temp_dir().join(format!("codefeed_cli_{suffix}"));
    let source = parent.join(OsString::from_vec(b"source-\xff".to_vec()));
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
    std::fs::remove_dir_all(parent).expect("temporary directory should be removed");
}
