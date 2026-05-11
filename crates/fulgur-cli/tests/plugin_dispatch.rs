//! Integration tests for the `fulgur` CLI plugin dispatch mechanism.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn cli_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fulgur"))
}

fn write_stub(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, body).unwrap();
    let mut perm = fs::metadata(&p).unwrap().permissions();
    perm.set_mode(0o755);
    fs::set_permissions(&p, perm).unwrap();
    p
}

fn cmd(path_dirs: &[&Path]) -> Command {
    let mut c = Command::new(cli_binary());
    let path = std::env::join_paths(path_dirs.iter().map(|p| p.to_path_buf())).expect("join_paths");
    c.env("PATH", path);
    c
}

#[test]
fn dispatch_runs_plugin_binary() {
    let dir = TempDir::new().unwrap();
    write_stub(dir.path(), "fulgur-stub", "#!/bin/sh\necho \"hello\"\n");

    let out = cmd(&[dir.path()]).args(["stub"]).output().expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello");
}

#[test]
fn dispatch_forwards_args_verbatim() {
    let dir = TempDir::new().unwrap();
    write_stub(
        dir.path(),
        "fulgur-stub",
        "#!/bin/sh\nprintf '%s\\n' \"$@\"\n",
    );

    let out = cmd(&[dir.path()])
        .args(["stub", "--flag", "a", "b"])
        .output()
        .expect("run");
    assert!(out.status.success());
    let lines: Vec<&str> = std::str::from_utf8(&out.stdout).unwrap().lines().collect();
    assert_eq!(lines, vec!["--flag", "a", "b"]);
}

#[test]
fn dispatch_sets_env_vars() {
    let dir = TempDir::new().unwrap();
    write_stub(
        dir.path(),
        "fulgur-stub",
        "#!/bin/sh\necho \"$FULGUR_EXEC_PATH\"\necho \"$FULGUR_VERSION\"\n",
    );

    let out = cmd(&[dir.path()]).args(["stub"]).output().expect("run");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines();
    let exec_path = lines.next().unwrap_or("");
    let version = lines.next().unwrap_or("");
    assert!(
        !exec_path.is_empty() && Path::new(exec_path).exists(),
        "FULGUR_EXEC_PATH was empty or non-existent: {exec_path:?}"
    );
    assert_eq!(version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn missing_plugin_reports_and_exits_127() {
    let dir = TempDir::new().unwrap(); // empty
    let out = cmd(&[dir.path()])
        .args(["nonexistent-xyzzy"])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(127));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("'nonexistent-xyzzy' is not a fulgur command"),
        "stderr: {stderr}"
    );
}

#[test]
fn plugins_command_lists_discovered_entries() {
    let dir = TempDir::new().unwrap();
    write_stub(dir.path(), "fulgur-alpha", "#!/bin/sh\n");
    write_stub(dir.path(), "fulgur-beta", "#!/bin/sh\n");

    let out = cmd(&[dir.path()]).arg("plugins").output().expect("run");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("fulgur-alpha"), "stdout: {stdout}");
    assert!(stdout.contains("fulgur-beta"), "stdout: {stdout}");
    assert!(stdout.contains(dir.path().to_str().unwrap()));
    assert!(!stdout.contains("(shadowed)"));
}

#[test]
fn plugins_command_marks_shadowed_duplicates() {
    let a = TempDir::new().unwrap();
    let b = TempDir::new().unwrap();
    write_stub(a.path(), "fulgur-dup", "#!/bin/sh\n");
    write_stub(b.path(), "fulgur-dup", "#!/bin/sh\n");

    let out = cmd(&[a.path(), b.path()])
        .arg("plugins")
        .output()
        .expect("run");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let dup_lines: Vec<&str> = stdout
        .lines()
        .filter(|l| l.contains("fulgur-dup"))
        .collect();
    assert_eq!(dup_lines.len(), 2, "stdout: {stdout}");
    assert!(!dup_lines[0].contains("(shadowed)"));
    assert!(dup_lines[1].contains("(shadowed)"));
}

#[test]
fn plugins_command_marks_builtin_shadowed_entries() {
    let dir = TempDir::new().unwrap();
    write_stub(dir.path(), "fulgur-render", "#!/bin/sh\n");

    let out = cmd(&[dir.path()]).arg("plugins").output().expect("run");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let render_line = stdout
        .lines()
        .find(|l| l.contains("fulgur-render"))
        .unwrap_or_else(|| panic!("no fulgur-render line in stdout: {stdout}"));
    assert!(
        render_line.contains("(shadowed by built-in)"),
        "expected built-in shadow marker on line: {render_line}"
    );
}

#[test]
fn plugins_command_reports_no_plugins_for_empty_path() {
    let dir = TempDir::new().unwrap(); // empty
    let out = cmd(&[dir.path()]).arg("plugins").output().expect("run");
    assert!(
        out.status.success(),
        "exit={:?}, stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("No fulgur plugins found on $PATH."),
        "stderr: {stderr}"
    );
}

#[test]
fn builtin_render_is_not_shadowed_by_path_entry() {
    let dir = TempDir::new().unwrap();
    // A `fulgur-render` on PATH that would corrupt output if invoked.
    write_stub(
        dir.path(),
        "fulgur-render",
        "#!/bin/sh\necho 'PLUGIN RENDER RAN' >&2\nexit 42\n",
    );

    // `fulgur render --help` should hit the built-in clap path and exit 0
    // with a usage banner — never the stub.
    let out = cmd(&[dir.path()])
        .args(["render", "--help"])
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "exit={:?}, stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("PLUGIN RENDER RAN"),
        "built-in render was shadowed by PATH entry; stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Render HTML to PDF") || stdout.contains("Usage:"),
        "stdout: {stdout}"
    );
}
