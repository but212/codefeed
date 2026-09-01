use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rustingest"))
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
            "Usage: rustingest <SOURCE> [--output <PATH>] [--ref <REF>]",
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
        &["ingest", "."][..],
    ] {
        let output = run(args);
        assert!(!output.status.success(), "args: {args:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("error:"));
    }
}

#[test]
fn creates_default_output_without_subcommand() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("rustingest_cli_default_{suffix}"));
    std::fs::create_dir_all(&root).expect("temporary directory should be created");
    std::fs::write(root.join("visible.txt"), "visible").expect("source file should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_rustingest"))
        .current_dir(&root)
        .arg(".")
        .output()
        .expect("CLI process should start");

    assert!(output.status.success(), "output: {output:?}");
    assert!(root.join("digest.txt").is_file());
    std::fs::remove_dir_all(root).expect("temporary directory should be removed");
    Ok(())
}

#[cfg(unix)]
#[test]
fn accepts_non_utf8_local_source_path() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the Unix epoch")
        .as_nanos();
    let parent = std::env::temp_dir().join(format!("rustingest_cli_{suffix}"));
    let source = parent.join(OsString::from_vec(b"source-\xff".to_vec()));
    let output = source.join("digest.txt");
    std::fs::create_dir_all(&source).expect("source directory should be created");
    std::fs::write(source.join("visible.txt"), "visible").expect("source file should be written");

    let result = Command::new(env!("CARGO_BIN_EXE_rustingest"))
        .arg(&source)
        .arg("--output")
        .arg(&output)
        .output()
        .expect("CLI process should start");

    assert!(result.status.success(), "output: {result:?}");
    assert!(output.is_file());
    std::fs::remove_dir_all(parent).expect("temporary directory should be removed");
}
