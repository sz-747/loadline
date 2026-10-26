//! On-disk state under `%LOCALAPPDATA%\optimus`: one file per live guarded run (read by
//! `optimus top`) and an append-only event log.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunRecord {
    /// Root process of the guarded agent tree.
    pub pid: u32,
    /// The `optimus run` process that owns the job.
    pub guard_pid: u32,
    pub command: String,
    pub budget: Option<u64>,
    pub hard: bool,
    pub started: u64,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn root_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("OPTIMUS_HOME") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("optimus")
}

fn runs_dir() -> PathBuf {
    root_dir().join("runs")
}

pub fn write_run(record: &RunRecord) -> io::Result<PathBuf> {
    let dir = runs_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", record.guard_pid));
    fs::write(&path, serde_json::to_vec_pretty(record)?)?;
    Ok(path)
}

/// Live runs. Records whose guard process is gone (crash, power loss) are deleted.
pub fn list_runs(is_alive: impl Fn(u32) -> bool) -> Vec<RunRecord> {
    let Ok(entries) = fs::read_dir(runs_dir()) else {
        return Vec::new();
    };
    let mut runs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let record = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<RunRecord>(&bytes).ok());
        match record {
            Some(r) if is_alive(r.guard_pid) => runs.push(r),
            _ => {
                let _ = fs::remove_file(&path);
            }
        }
    }
    runs.sort_by_key(|r| r.started);
    runs
}

#[derive(Serialize)]
struct Event<'a> {
    ts: u64,
    event: &'a str,
    pid: u32,
    command: &'a str,
    detail: String,
}

/// Append one JSON line to `events.jsonl`. Logging failures never stop a run.
pub fn log_event(event: &str, pid: u32, command: &str, detail: String) {
    let dir = root_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let line = Event {
        ts: now(),
        event,
        pid,
        command,
        detail,
    };
    if let (Ok(mut file), Ok(json)) = (
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("events.jsonl")),
        serde_json::to_string(&line),
    ) {
        let _ = writeln!(file, "{json}");
    }
}
