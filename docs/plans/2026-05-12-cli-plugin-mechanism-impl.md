# CLI Plugin Mechanism (git-style) — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make `fulgur <name>` dispatch to `fulgur-<name>` on `$PATH` when `<name>` is not a built-in subcommand, plus a new `fulgur plugins` listing command. Tier 1 only (PATH lookup, no fetch).

**Architecture:** Add a `plugin` module to `fulgur-cli` that owns PATH walking, dispatch (Unix `exec`, Windows spawn), and `(name, path, shadowed)` listing. Wire it into clap via the `external_subcommand` variant plus a new built-in `Plugins` subcommand. `main.rs` stays a thin dispatcher.

**Tech Stack:** Rust, clap 4 (derive), `which = "7"` for `$PATH` resolution, `libc` (Unix `exec`), `tempfile` (dev) for integration tests.

**Design source:** `docs/plans/2026-05-08-cli-plugin-mechanism-design.md`. Error-message format follows git/cargo convention: `fulgur: '<name>' is not a fulgur command. See 'fulgur --help'.` (single-quoted, matching `git: 'foo' is not a git command.`). Exit code on missing plugin: 127. The beads issue's acceptance criteria are updated alongside this plan to match.

---

## Task 1: Baseline verification + dependency bump

**Files:**

- Modify: `crates/fulgur-cli/Cargo.toml`

**Step 1: Run baseline tests**

```bash
cargo test -p fulgur-cli
```

Expected: all existing tests pass (bookmarks_cli, examples_determinism, inspect_test, tagged_cli).

If a baseline test fails, stop and report — do not proceed.

**Step 2: Add `which = "7"` to `[dependencies]`**

Edit `crates/fulgur-cli/Cargo.toml`, inserting `which = "7"` in the `[dependencies]` table alphabetically (after `serde_json`).

```toml
[dependencies]
fulgur = { version = "0.15.0", path = "../fulgur" }
clap = { version = "4", features = ["derive"] }
serde_json = "1"
which = "7"
```

**Step 3: Verify it builds**

```bash
cargo build -p fulgur-cli
```

Expected: success, no warnings about unused dep (we will use it in the next task; if cargo warns now, ignore — it disappears once `plugin.rs` lands).

**Step 4: Commit**

```bash
git add crates/fulgur-cli/Cargo.toml Cargo.lock
git commit -m "build(fulgur-cli): add which 7 for plugin PATH resolution"
```

---

## Task 2: Plugin module skeleton with `PluginEntry` and `list_from_paths` (TDD)

**Files:**

- Create: `crates/fulgur-cli/src/plugin.rs`
- Modify: `crates/fulgur-cli/src/main.rs` (add `mod plugin;`)

**Step 1: Wire empty module**

Append to `crates/fulgur-cli/src/main.rs` near the top (after the `use` block):

```rust
mod plugin;
```

Create `crates/fulgur-cli/src/plugin.rs`:

```rust
//! git-style plugin dispatch for the `fulgur` CLI.
//!
//! `fulgur <name>` execs `fulgur-<name>` from `$PATH` when `<name>` is not
//! a built-in subcommand. `fulgur plugins` lists discovered plugins.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// One plugin entry discovered on `$PATH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginEntry {
    /// Plugin name with the `fulgur-` prefix stripped.
    pub name: String,
    /// Absolute path to the executable.
    pub path: PathBuf,
    /// `true` if an earlier directory on `$PATH` already provided a plugin
    /// with the same name.
    pub shadowed: bool,
}
```

**Step 2: Write the failing unit test for `list_from_paths`**

Append at the bottom of `plugin.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use tempfile::tempdir;

    #[cfg(unix)]
    fn write_exec(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, "#!/bin/sh\n").unwrap();
        let mut perm = fs::metadata(&p).unwrap().permissions();
        perm.set_mode(0o755);
        fs::set_permissions(&p, perm).unwrap();
        p
    }

    #[cfg(unix)]
    #[test]
    fn list_finds_fulgur_prefixed_executables() {
        let dir = tempdir().unwrap();
        write_exec(dir.path(), "fulgur-chart");
        write_exec(dir.path(), "fulgur-math");
        write_exec(dir.path(), "other-tool"); // not a plugin
        fs::write(dir.path().join("fulgur-readme"), "not exec").unwrap(); // not executable

        let entries = list_from_paths(std::iter::once(dir.path().to_path_buf()));
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"chart"));
        assert!(names.contains(&"math"));
        assert!(!names.contains(&"readme")); // skipped: not executable
        assert!(!names.iter().any(|n| n.starts_with("other"))); // not prefixed
        assert!(entries.iter().all(|e| !e.shadowed));
    }

    #[cfg(unix)]
    #[test]
    fn list_marks_shadowed_duplicates() {
        let a = tempdir().unwrap();
        let b = tempdir().unwrap();
        write_exec(a.path(), "fulgur-dup");
        write_exec(b.path(), "fulgur-dup");

        let entries = list_from_paths(
            [a.path().to_path_buf(), b.path().to_path_buf()].into_iter(),
        );
        let dups: Vec<&PluginEntry> = entries.iter().filter(|e| e.name == "dup").collect();
        assert_eq!(dups.len(), 2);
        assert!(!dups[0].shadowed);
        assert!(dups[1].shadowed);
        assert_eq!(dups[0].path, a.path().join("fulgur-dup"));
        assert_eq!(dups[1].path, b.path().join("fulgur-dup"));
    }
}
```

Add `tempfile` to `[dev-dependencies]` (it's already there) — no change needed for tests, but verify with `grep tempfile crates/fulgur-cli/Cargo.toml`.

**Step 3: Run the failing tests**

```bash
cargo test -p fulgur-cli --lib plugin
```

Expected: compile error — `list_from_paths` does not exist.

**Step 4: Implement `list_from_paths`**

Insert in `plugin.rs` between the `PluginEntry` struct and `#[cfg(test)]`:

```rust
/// Walk the given directories and return all `fulgur-*` executables, marking
/// later duplicates with `shadowed = true`. Entries are returned in `$PATH`
/// traversal order, then by filename within each directory.
pub fn list_from_paths<I>(paths: I) -> Vec<PluginEntry>
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut out: Vec<PluginEntry> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for dir in paths {
        let read = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let mut in_dir: Vec<(String, PathBuf)> = Vec::new();
        for entry in read.flatten() {
            let file_name = entry.file_name();
            let name_str = match file_name.to_str() {
                Some(s) => s.to_owned(),
                None => continue,
            };
            let Some(stripped) = strip_plugin_name(&name_str) else {
                continue;
            };
            let path = entry.path();
            if !is_executable(&path) {
                continue;
            }
            in_dir.push((stripped, path));
        }
        in_dir.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, path) in in_dir {
            let shadowed = !seen.insert(name.clone());
            out.push(PluginEntry { name, path, shadowed });
        }
    }
    out
}

/// Strip the `fulgur-` prefix and platform-specific executable extension
/// (`.exe` on Windows). Returns `None` if `name` is not a plugin filename.
fn strip_plugin_name(name: &str) -> Option<String> {
    let stem = name.strip_prefix("fulgur-")?;
    #[cfg(windows)]
    {
        if let Some(s) = stem.strip_suffix(".exe") {
            return Some(s.to_owned());
        }
    }
    if stem.is_empty() || stem.contains('.') {
        // Skip "fulgur-" with no name, or names with extra dots (e.g.
        // "fulgur-foo.bak", README-like files). On Windows `.exe` is
        // already handled above.
        return None;
    }
    Some(stem.to_owned())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
        .unwrap_or(false)
}

#[cfg(windows)]
fn is_executable(path: &Path) -> bool {
    // v1: only `.exe` is recognised on Windows (see `strip_plugin_name`).
    // `.bat` / `.cmd` / `PATHEXT` matching is deferred to a follow-up —
    // the integration test suite is `#![cfg(unix)]` so the gap is
    // tolerated for now.
    std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
}
```

**Step 5: Run tests**

```bash
cargo test -p fulgur-cli --lib plugin
```

Expected: both tests PASS.

**Step 6: Add `list()` wrapper that reads `$PATH`**

Append between `list_from_paths` and `strip_plugin_name`:

```rust
/// Walk `$PATH` and return all `fulgur-*` plugins. Convenience over
/// [`list_from_paths`] for the production caller; tests use the explicit
/// path iterator instead.
pub fn list() -> Vec<PluginEntry> {
    let paths: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    list_from_paths(paths)
}
```

**Step 7: Run lib tests + commit**

```bash
cargo test -p fulgur-cli --lib
cargo fmt
git add crates/fulgur-cli/src/main.rs crates/fulgur-cli/src/plugin.rs
git commit -m "feat(fulgur-cli): plugin discovery — PATH walk + shadow marking"
```

---

## Task 3: Dispatch logic (Unix exec, Windows spawn, missing plugin)

**Files:**

- Modify: `crates/fulgur-cli/src/plugin.rs`

**Step 1: Add `dispatch()` function**

Append after `list()`:

```rust
/// Resolve `fulgur-<name>` on `$PATH` and execute it with the remaining
/// arguments. Never returns: either replaces the current process (Unix),
/// exits with the child's status (Windows), or exits 127 if the plugin
/// is not found.
///
/// `args[0]` is the subcommand name (as clap routes the
/// `#[command(external_subcommand)]` variant). `args[1..]` are forwarded
/// verbatim to the plugin.
pub fn dispatch(args: Vec<OsString>) -> ! {
    let mut iter = args.into_iter();
    let Some(name_os) = iter.next() else {
        eprintln!("fulgur: empty external subcommand");
        std::process::exit(2);
    };
    let name = name_os.to_string_lossy().into_owned();
    let plugin_args: Vec<OsString> = iter.collect();

    let binary = format!("fulgur-{name}");
    let plugin_path = match which::which(&binary) {
        Ok(p) => p,
        Err(_) => {
            eprintln!(
                "fulgur: '{name}' is not a fulgur command. See 'fulgur --help'."
            );
            std::process::exit(127);
        }
    };

    let exec_path = std::env::current_exe()
        .ok()
        .map(|p| p.into_os_string())
        .unwrap_or_else(|| OsString::from("fulgur"));
    let version = env!("CARGO_PKG_VERSION");

    run_plugin(&plugin_path, &plugin_args, &exec_path, version)
}

#[cfg(unix)]
fn run_plugin(
    plugin_path: &Path,
    args: &[OsString],
    exec_path: &std::ffi::OsStr,
    version: &str,
) -> ! {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(plugin_path)
        .args(args)
        .env("FULGUR_EXEC_PATH", exec_path)
        .env("FULGUR_VERSION", version)
        .exec();
    // `exec` only returns on failure.
    eprintln!("fulgur: failed to exec {}: {err}", plugin_path.display());
    std::process::exit(1);
}

#[cfg(windows)]
fn run_plugin(
    plugin_path: &Path,
    args: &[OsString],
    exec_path: &std::ffi::OsStr,
    version: &str,
) -> ! {
    let status = std::process::Command::new(plugin_path)
        .args(args)
        .env("FULGUR_EXEC_PATH", exec_path)
        .env("FULGUR_VERSION", version)
        .status();
    match status {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("fulgur: failed to spawn {}: {e}", plugin_path.display());
            std::process::exit(1);
        }
    }
}
```

**Step 2: Add a unit test for the missing-plugin error path**

Inside the `#[cfg(test)] mod tests` block, add:

```rust
    #[test]
    fn strip_plugin_name_rejects_non_plugins() {
        assert_eq!(strip_plugin_name("fulgur-chart"), Some("chart".to_owned()));
        assert_eq!(strip_plugin_name("fulgur-"), None);
        assert_eq!(strip_plugin_name("fulgur-foo.bak"), None);
        assert_eq!(strip_plugin_name("other-tool"), None);
        #[cfg(windows)]
        assert_eq!(strip_plugin_name("fulgur-chart.exe"), Some("chart".to_owned()));
    }
```

**Step 3: Verify it compiles + tests pass**

```bash
cargo test -p fulgur-cli --lib plugin
```

Expected: all plugin tests pass. The `dispatch` function is uncovered by unit tests (success paths require real exec); integration tests cover it in Task 5.

**Step 4: Commit**

```bash
cargo fmt
git add crates/fulgur-cli/src/plugin.rs
git commit -m "feat(fulgur-cli): plugin dispatch — Unix exec / Windows spawn / 127 fallback"
```

---

## Task 4: Wire `External` + `Plugins` into clap

**Files:**

- Modify: `crates/fulgur-cli/src/main.rs`

**Step 1: Add the two variants to `Commands`**

Locate the `enum Commands` definition (line ~120 in `main.rs`). After the `Template { ... }` variant, add:

```rust
    /// List discovered plugins on `$PATH`.
    Plugins,
    /// External plugin: dispatches to `fulgur-<name>` on `$PATH`.
    #[command(external_subcommand)]
    External(Vec<std::ffi::OsString>),
```

**Step 2: Handle both arms in `main()`**

In the `match cli.command { ... }` block, after the `Commands::Template { command } => { ... }` arm, add:

```rust
        Commands::Plugins => {
            let entries = plugin::list();
            if entries.is_empty() {
                eprintln!("No fulgur plugins found on $PATH.");
                return;
            }
            println!("Available plugins (from $PATH):");
            for entry in entries {
                let suffix = if entry.shadowed { "  (shadowed)" } else { "" };
                println!(
                    "  fulgur-{:<12} {}{}",
                    entry.name,
                    entry.path.display(),
                    suffix
                );
            }
        }
        Commands::External(args) => {
            plugin::dispatch(args);
        }
```

**Step 3: Build + smoke-test**

```bash
cargo build -p fulgur-cli
cargo run -p fulgur-cli -- plugins
```

Expected: build succeeds. `fulgur plugins` prints either the "No fulgur plugins found" line or a listing — on a fresh machine without plugins installed, the former.

```bash
cargo run -p fulgur-cli -- nonexistent 2>&1; echo "exit=$?"
```

Expected: `fulgur: 'nonexistent' is not a fulgur command. See 'fulgur --help'.` on stderr, exit code 127.

**Step 4: Commit**

```bash
cargo fmt
git add crates/fulgur-cli/src/main.rs
git commit -m "feat(fulgur-cli): wire External + Plugins subcommands into clap"
```

---

## Task 5: Integration tests for dispatch + listing

**Files:**

- Create: `crates/fulgur-cli/tests/plugin_dispatch.rs`

**Step 1: Write the test file**

```rust
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
    let path = std::env::join_paths(path_dirs.iter().map(|p| p.to_path_buf()))
        .expect("join_paths");
    c.env("PATH", path);
    c
}

#[test]
fn dispatch_success_echoes_args() {
    let dir = TempDir::new().unwrap();
    write_stub(dir.path(), "fulgur-stub", "#!/bin/sh\necho \"hello\"\n");

    let out = cmd(&[dir.path()])
        .args(["stub"])
        .output()
        .expect("run");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
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
    let lines: Vec<&str> = std::str::from_utf8(&out.stdout)
        .unwrap()
        .lines()
        .collect();
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

    let out = cmd(&[dir.path()])
        .args(["stub"])
        .output()
        .expect("run");
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

    let out = cmd(&[dir.path()])
        .arg("plugins")
        .output()
        .expect("run");
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
    let dup_lines: Vec<&str> = stdout.lines().filter(|l| l.contains("fulgur-dup")).collect();
    assert_eq!(dup_lines.len(), 2, "stdout: {stdout}");
    assert!(!dup_lines[0].contains("(shadowed)"));
    assert!(dup_lines[1].contains("(shadowed)"));
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
```

**Step 2: Run the new test file**

```bash
cargo test -p fulgur-cli --test plugin_dispatch
```

Expected: 7 tests pass.

If `dispatch_success_echoes_args` fails on the missing-plugin path because `which` couldn't find the stub, the most common cause is that `cmd()` did not propagate the test's PATH cleanly. Verify the `env` setting overrides rather than appending — `Command::env()` replaces the single variable, which is what we want.

**Step 3: Run the full crate test suite**

```bash
cargo test -p fulgur-cli
```

Expected: all crates' tests pass (existing + 7 new).

**Step 4: Commit**

```bash
cargo fmt
git add crates/fulgur-cli/tests/plugin_dispatch.rs
git commit -m "test(fulgur-cli): integration tests for plugin dispatch + listing"
```

---

## Task 6: Lint sweep + workspace check

**Step 1: Format check**

```bash
cargo fmt --check
```

Expected: zero diff. If anything was missed in earlier tasks, `cargo fmt` then re-commit (`style: cargo fmt`).

**Step 2: Clippy on the affected crate**

```bash
cargo clippy -p fulgur-cli --all-targets -- -D warnings
```

Expected: zero warnings. If clippy fires on the new code, fix in place (favour idiomatic rewrites over `#[allow(...)]`). Workspace-wide clippy regressions surface in CI on PR review — out of scope here.

**Step 3: Final workspace test sweep**

```bash
cargo test -p fulgur-cli
```

Expected: all tests pass.

**Step 4: Commit any lint fixups**

If a fixup was needed:

```bash
git add -u
git commit -m "style(fulgur-cli): clippy/fmt cleanup for plugin module"
```

Otherwise this task closes without a commit.

---

## Risks / open points (carried over from design)

- `CommandExt::exec` on Unix never returns on success; do not put any
  drop-relevant state on the stack between `dispatch()` entry and the
  `exec()` call. (`StdoutIsolator` is render-path-only, so this is
  already safe.)
- Windows signal forwarding is not handled in v1 (matches design's
  acceptance).
- `fulgur plugins` `read_dir` walk is not parallelised; cold PATH walks
  on Windows network drives may be slow. Out of scope.

## Out of scope (deferred)

- Tier 2: `fulgur x <name>[@<ver>]` with `~/.cache/fulgur/plugins/`
  cache + `cargo binstall` fallback. New beads issue when Tier 1 ships.
- `--json` flag on `fulgur plugins`.
- Shell completion knowledge of plugins.
- Plugin manifest / capability declaration protocol.
