//! Find the program the way a shell would: `claude` may be `claude.exe` or an npm
//! `claude.cmd` shim. Rust's `Command` only appends `.exe` on its own.

use std::path::{Path, PathBuf};

const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

pub fn resolve_program(program: &str) -> PathBuf {
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| DEFAULT_PATHEXT.to_string());
    let dirs: Vec<PathBuf> = std::env::split_paths(&path_var).collect();
    resolve_in(program, &dirs, &pathext).unwrap_or_else(|| PathBuf::from(program))
}

/// The program and leading arguments that actually launch `program`. A `.ps1` shim is
/// run by PowerShell 7 when installed, else Windows PowerShell.
pub fn launcher(program: &str) -> (PathBuf, Vec<String>) {
    let path = resolve_program(program);
    let is_ps1 = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("ps1"));
    if !is_ps1 {
        return (path, Vec::new());
    }
    let pwsh = resolve_program("pwsh");
    let shell = if pwsh.is_file() {
        pwsh
    } else {
        PathBuf::from("powershell.exe")
    };
    let args = vec![
        "-NoProfile".to_string(),
        "-File".to_string(),
        path.to_string_lossy().into_owned(),
    ];
    (shell, args)
}

pub fn resolve_in(program: &str, dirs: &[PathBuf], pathext: &str) -> Option<PathBuf> {
    let given = Path::new(program);
    // An explicit path or extension is the user's choice; do not second-guess it.
    if given.components().count() > 1 || given.extension().is_some() {
        return None;
    }
    // PowerShell shims (`codex.ps1`) are not in PATHEXT but run from a PowerShell prompt.
    let exts: Vec<&str> = pathext
        .split(';')
        .filter(|e| !e.is_empty())
        .chain([".PS1"])
        .collect();
    for dir in dirs {
        for ext in &exts {
            let candidate = dir.join(format!("{program}{}", ext.to_ascii_lowercase()));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_cmd_shim_and_prefers_pathext_order() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::write(a.path().join("agent.cmd"), "").unwrap();
        fs::write(b.path().join("agent.exe"), "").unwrap();
        let dirs = vec![a.path().to_path_buf(), b.path().to_path_buf()];
        // First directory wins, like PATH.
        assert_eq!(
            resolve_in("agent", &dirs, DEFAULT_PATHEXT),
            Some(a.path().join("agent.cmd"))
        );
        fs::write(a.path().join("agent.exe"), "").unwrap();
        // Within a directory, PATHEXT order wins (.EXE before .CMD).
        assert_eq!(
            resolve_in("agent", &dirs, DEFAULT_PATHEXT),
            Some(a.path().join("agent.exe"))
        );
    }

    #[test]
    fn finds_powershell_shim_last() {
        let a = tempfile::tempdir().unwrap();
        fs::write(a.path().join("agent.ps1"), "").unwrap();
        let dirs = vec![a.path().to_path_buf()];
        assert_eq!(
            resolve_in("agent", &dirs, DEFAULT_PATHEXT),
            Some(a.path().join("agent.ps1"))
        );
        fs::write(a.path().join("agent.cmd"), "").unwrap();
        assert_eq!(
            resolve_in("agent", &dirs, DEFAULT_PATHEXT),
            Some(a.path().join("agent.cmd"))
        );
    }

    #[test]
    fn leaves_explicit_paths_alone() {
        let dirs = vec![std::env::temp_dir()];
        assert_eq!(resolve_in("tool.exe", &dirs, DEFAULT_PATHEXT), None);
        assert_eq!(resolve_in(r"C:\x\tool", &dirs, DEFAULT_PATHEXT), None);
    }

    #[test]
    fn missing_program_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = vec![dir.path().to_path_buf()];
        assert_eq!(resolve_in("nothing-here", &dirs, DEFAULT_PATHEXT), None);
    }
}
