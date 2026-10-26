//! `loadline top`: what each agent tree really commits, against system headroom.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io;
use std::thread;
use std::time::Duration;

use serde::Serialize;

use crate::admission::{self, SystemMemory};
use crate::size::format_size;
use crate::state::{self, RunRecord};
use crate::sys;
use crate::tree::{Snapshot, TreeUsage, AGENT_NAMES};

#[derive(Serialize)]
pub struct Row {
    #[serde(flatten)]
    pub usage: TreeUsage,
    pub guarded: bool,
    pub budget: Option<u64>,
    pub hard: bool,
    pub command: Option<String>,
}

#[derive(Serialize)]
pub struct Report {
    pub system: SystemMemory,
    pub commit_headroom: u64,
    pub reserve: u64,
    pub safe_zone: u64,
    pub agents: Vec<Row>,
}

pub fn collect(reserve: Option<u64>) -> io::Result<Report> {
    let system = sys::system_memory()?;
    let procs = sys::snapshot_processes()?;
    let runs: Vec<RunRecord> = state::list_runs(sys::is_alive);
    let snap = Snapshot::new(&procs);

    let mut agents = Vec::new();
    let mut guarded_pids = HashSet::new();
    for run in &runs {
        guarded_pids.extend(snap.descendants(run.pid));
        if let Some(usage) = snap.usage(run.pid) {
            agents.push(Row {
                usage,
                guarded: true,
                budget: run.budget,
                hard: run.hard,
                command: Some(run.command.clone()),
            });
        }
    }
    for root in snap.agent_roots(AGENT_NAMES, &guarded_pids) {
        if let Some(usage) = snap.usage(root) {
            agents.push(Row {
                usage,
                guarded: false,
                budget: None,
                hard: false,
                command: None,
            });
        }
    }
    agents.sort_by_key(|r| std::cmp::Reverse(r.usage.private));

    let reserve = reserve.unwrap_or_else(|| admission::default_reserve(system.total_phys));
    Ok(Report {
        commit_headroom: system.commit_headroom(),
        safe_zone: admission::safe_zone(&system, reserve),
        reserve,
        system,
        agents,
    })
}

fn status(row: &Row) -> String {
    match (row.guarded, row.budget) {
        (false, _) => "unguarded".into(),
        (true, None) => "guarded".into(),
        (true, Some(budget)) => {
            let pct = row.usage.private.saturating_mul(100) / budget.max(1);
            let mode = if row.hard { "hard" } else { "soft" };
            if pct >= 100 {
                format!("OVER {pct}% ({mode})")
            } else {
                format!("{pct}% ({mode})")
            }
        }
    }
}

pub fn render(report: &Report) -> String {
    let s = &report.system;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "RAM {} | free {} | commit {} / {} (headroom {}) | safe for new agents {} (keeping {} for Windows)",
        format_size(s.total_phys),
        format_size(s.avail_phys),
        format_size(s.commit_total),
        format_size(s.commit_limit),
        format_size(report.commit_headroom),
        format_size(report.safe_zone),
        format_size(report.reserve),
    );
    let _ = writeln!(out);
    if report.agents.is_empty() {
        let _ = writeln!(out, "No agents running.");
        return out;
    }
    let _ = writeln!(
        out,
        "{:<14} {:>7} {:>6} {:>10} {:>12} {:>10}  STATUS",
        "AGENT", "PID", "PROCS", "COMMITTED", "WORKING SET", "BUDGET"
    );
    for row in &report.agents {
        let _ = writeln!(
            out,
            "{:<14} {:>7} {:>6} {:>10} {:>12} {:>10}  {}",
            row.usage.name,
            row.usage.root,
            row.usage.processes,
            format_size(row.usage.private),
            format_size(row.usage.working_set),
            row.budget.map(format_size).unwrap_or_else(|| "-".into()),
            status(row),
        );
    }
    let total: u64 = report.agents.iter().map(|r| r.usage.private).sum();
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Agents commit {} in total. COMMITTED is what runs Windows out of memory; \
         Task Manager's default column shows WORKING SET.",
        format_size(total)
    );
    out
}

pub fn top(watch: bool, json: bool, reserve: Option<u64>) -> io::Result<()> {
    if !watch {
        let report = collect(reserve)?;
        if json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print!("{}", render(&report));
        }
        return Ok(());
    }
    sys::enable_vt();
    loop {
        let report = collect(reserve)?;
        let body = if json {
            serde_json::to_string(&report)? + "\n"
        } else {
            render(&report)
        };
        // Clear screen and home the cursor, then redraw.
        print!("\x1b[2J\x1b[H{body}");
        thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    fn row(private: u64, guarded: bool, budget: Option<u64>, hard: bool) -> Row {
        Row {
            usage: TreeUsage {
                root: 1,
                name: "claude".into(),
                processes: 3,
                private,
                working_set: private / 2,
            },
            guarded,
            budget,
            hard,
            command: None,
        }
    }

    #[test]
    fn status_labels() {
        assert_eq!(status(&row(GB, false, None, false)), "unguarded");
        assert_eq!(status(&row(GB, true, None, false)), "guarded");
        assert_eq!(status(&row(GB, true, Some(2 * GB), false)), "50% (soft)");
        assert_eq!(
            status(&row(3 * GB, true, Some(2 * GB), true)),
            "OVER 150% (hard)"
        );
    }

    #[test]
    fn renders_16gb_laptop() {
        let report = Report {
            system: SystemMemory {
                total_phys: 16 * GB,
                avail_phys: 3 * GB,
                commit_total: 20 * GB,
                commit_limit: 24 * GB,
            },
            commit_headroom: 4 * GB,
            reserve: 2 * GB,
            safe_zone: GB,
            agents: vec![
                row(5 * GB, true, Some(3 * GB), false),
                row(2 * GB, false, None, false),
            ],
        };
        let text = render(&report);
        assert!(text.contains("RAM 16.0 GB"));
        assert!(text.contains("safe for new agents 1.0 GB"));
        assert!(text.contains("OVER 166% (soft)"));
        assert!(text.contains("Agents commit 7.0 GB in total"));
    }
}
