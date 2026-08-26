use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_local_ingest"))
        .args(args)
        .output()
        .expect("CLI process should start")
}

#[test]
fn help_exits_successfully_without_ingesting() {
    let cases: &[&[&str]] = &[
        &["--help"],
        &["ingest", "--help"],
        &["ingest", ".", "--help"],
    ];
    for args in cases {
        let output = run(args);
        assert!(
            output.status.success(),
            "args: {args:?}, output: {output:?}"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.matches("Usage:").count(), 1, "args: {args:?}");
        assert!(output.stderr.is_empty(), "args: {args:?}");
    }
}

#[test]
fn invalid_arguments_exit_nonzero() {
    for args in [
        &["ingest", ".", "--unknown"][..],
        &["ingest", ".", "--output", "--help"][..],
    ] {
        let output = run(args);
        assert!(!output.status.success(), "args: {args:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("error:"));
    }
}
