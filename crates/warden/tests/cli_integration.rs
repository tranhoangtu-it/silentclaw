use std::process::Command;

#[test]
fn test_warden_version() {
    let output = Command::new("cargo")
        .args(["run", "--bin", "warden", "--", "--version"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("0.1.0"));
}

#[test]
#[ignore] // Requires full build
fn test_warden_run_plan_dry_run() {
    let output = Command::new("cargo")
        .args([
            "run",
            "--bin",
            "warden",
            "--",
            "run-plan",
            "--file",
            "examples/plan_hello.json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
}

#[test]
fn test_warden_help() {
    let output = Command::new("cargo")
        .args(["run", "--bin", "warden", "--", "--help"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("run-plan"));
}
