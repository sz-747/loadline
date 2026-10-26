//! End-to-end checks of `loadline run` against real processes and real job limits.
//! Memory pressure comes from PowerShell allocating a .NET byte array, which commits
//! the memory immediately.

use std::path::Path;
use std::process::{Command, Output};

fn loadline(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_loadline"))
        .env("LOADLINE_HOME", home)
        .args(args)
        .output()
        .expect("run loadline")
}

fn alloc_script(mb: u32, sleep_secs: u32) -> String {
    format!("$a = New-Object byte[] ({mb}MB); Start-Sleep -Seconds {sleep_secs}")
}

fn events(home: &Path) -> String {
    std::fs::read_to_string(home.join("events.jsonl")).unwrap_or_default()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn propagates_exit_code() {
    let home = tempfile::tempdir().unwrap();
    let out = loadline(home.path(), &["run", "--", "cmd", "/c", "exit", "7"]);
    assert_eq!(out.status.code(), Some(7), "{}", stderr(&out));
    assert!(stderr(&out).contains("exited with code 7"));
}

#[test]
fn soft_budget_reports_without_killing() {
    let home = tempfile::tempdir().unwrap();
    let script = alloc_script(400, 3);
    let out = loadline(
        home.path(),
        &[
            "run",
            "--budget",
            "250M",
            "--",
            "powershell",
            "-NoProfile",
            "-Command",
            &script,
        ],
    );
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(err.contains("went over budget"), "{err}");
    assert!(events(home.path()).contains("\"soft_limit\""));
}

#[test]
fn hard_budget_stops_the_allocation() {
    let home = tempfile::tempdir().unwrap();
    let script = alloc_script(400, 1) + "; exit 0";
    let out = loadline(
        home.path(),
        &[
            "run",
            "--budget",
            "250M",
            "--hard",
            "--",
            "powershell",
            "-NoProfile",
            "-Command",
            &script,
        ],
    );
    let err = stderr(&out);
    assert!(err.contains("hit the hard limit"), "{err}");
    assert!(events(home.path()).contains("\"hard_limit\""));
}

#[test]
fn refuses_what_cannot_fit() {
    let home = tempfile::tempdir().unwrap();
    let out = loadline(
        home.path(),
        &["run", "--budget", "100T", "--", "cmd", "/c", "exit", "0"],
    );
    assert_eq!(out.status.code(), Some(75));
    assert!(stderr(&out).contains("not starting cmd"));
    assert!(events(home.path()).contains("\"refused\""));
}

#[test]
fn ends_leftover_processes() {
    let home = tempfile::tempdir().unwrap();
    // `start /b` leaves a ping running after cmd exits: an orphan without Loadline.
    let out = loadline(
        home.path(),
        &[
            "run",
            "--",
            "cmd",
            "/c",
            "start",
            "/b",
            "ping",
            "-n",
            "30",
            "127.0.0.1",
            ">nul",
        ],
    );
    let err = stderr(&out);
    assert!(err.contains("ended 1 leftover process"), "{err}");
}

#[test]
fn top_json_is_valid() {
    let home = tempfile::tempdir().unwrap();
    let out = loadline(home.path(), &["top", "--json"]);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert!(v["safe_zone"].as_u64().is_some());
    assert!(v["agents"].is_array());
}
