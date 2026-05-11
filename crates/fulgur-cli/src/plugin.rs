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
            out.push(PluginEntry {
                name,
                path,
                shadowed,
            });
        }
    }
    out
}

/// Walk `$PATH` and return all `fulgur-*` plugins. Convenience over
/// [`list_from_paths`] for the production caller; tests use the explicit
/// path iterator instead.
pub fn list() -> Vec<PluginEntry> {
    let paths: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    list_from_paths(paths)
}

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
            eprintln!("fulgur: '{name}' is not a fulgur command. See 'fulgur --help'.");
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
    std::fs::metadata(path)
        .map(|m| m.is_file())
        .unwrap_or(false)
}

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

        let entries = list_from_paths([a.path().to_path_buf(), b.path().to_path_buf()].into_iter());
        let dups: Vec<&PluginEntry> = entries.iter().filter(|e| e.name == "dup").collect();
        assert_eq!(dups.len(), 2);
        assert!(!dups[0].shadowed);
        assert!(dups[1].shadowed);
        assert_eq!(dups[0].path, a.path().join("fulgur-dup"));
        assert_eq!(dups[1].path, b.path().join("fulgur-dup"));
    }

    #[test]
    fn strip_plugin_name_rejects_non_plugins() {
        assert_eq!(strip_plugin_name("fulgur-chart"), Some("chart".to_owned()));
        assert_eq!(strip_plugin_name("fulgur-"), None);
        assert_eq!(strip_plugin_name("fulgur-foo.bak"), None);
        assert_eq!(strip_plugin_name("other-tool"), None);
        #[cfg(windows)]
        assert_eq!(
            strip_plugin_name("fulgur-chart.exe"),
            Some("chart".to_owned())
        );
    }
}
