//! `optimus run`: admission check, then start the agent suspended inside a job, resume
//! it, watch its memory, and end anything it leaves behind.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::HANDLE;

use crate::admission::{self, Admission};
use crate::job::{Job, JobOptions, MSG_JOB_MEMORY_LIMIT, MSG_NOTIFICATION_LIMIT};
use crate::resolve::launcher;
use crate::size::format_size;
use crate::state::{self, RunRecord};
use crate::sys;

const CREATE_SUSPENDED: u32 = 0x0000_0004;
/// Exit code when admission refuses to start the agent (BSD EX_TEMPFAIL).
pub const EXIT_REFUSED: u8 = 75;
const WAIT_POLL: Duration = Duration::from_secs(2);
/// Re-arm the soft budget once the tree is back under this share of it.
const REARM_RATIO: f64 = 0.9;

pub struct RunArgs {
    pub budget: Option<u64>,
    pub hard: bool,
    pub reserve: Option<u64>,
    pub wait: bool,
    pub force: bool,
    pub keep_children: bool,
    pub command: Vec<String>,
}

fn admit(args: &RunArgs, label: &str) -> io::Result<bool> {
    let need = args.budget.unwrap_or(admission::DEFAULT_ESTIMATE);
    let mut announced = false;
    loop {
        let mem = sys::system_memory()?;
        let reserve = args
            .reserve
            .unwrap_or_else(|| admission::default_reserve(mem.total_phys));
        match admission::decide(&mem, reserve, need) {
            Admission::Admit => return Ok(true),
            Admission::Refuse { need, safe_zone } => {
                if args.force {
                    eprintln!(
                        "optimus: --force: starting {label} with only {} free of the {} it needs",
                        format_size(safe_zone),
                        format_size(need)
                    );
                    return Ok(true);
                }
                if !args.wait {
                    eprintln!(
                        "optimus: not starting {label}: it needs {}, but only {} is safe to use \
                         ({} free RAM, {} commit headroom, {} kept for Windows).\n\
                         Close an agent, or use --wait to start when memory frees up.",
                        format_size(need),
                        format_size(safe_zone),
                        format_size(mem.avail_phys),
                        format_size(mem.commit_headroom()),
                        format_size(reserve),
                    );
                    state::log_event(
                        "refused",
                        0,
                        label,
                        format!("need {need} safe_zone {safe_zone}"),
                    );
                    return Ok(false);
                }
                if !announced {
                    eprintln!(
                        "optimus: waiting for memory: {label} needs {}, {} is safe now",
                        format_size(need),
                        format_size(safe_zone)
                    );
                    announced = true;
                }
                thread::sleep(WAIT_POLL);
            }
        }
    }
}

/// Committed bytes of everything in the job right now.
fn job_private(job: &Job) -> u64 {
    job.member_pids()
        .unwrap_or_default()
        .into_iter()
        .filter_map(sys::process_memory)
        .map(|(private, _)| private)
        .sum()
}

pub fn run(args: RunArgs) -> io::Result<ExitCode> {
    let (program, rest) = args
        .command
        .split_first()
        .ok_or_else(|| io::Error::other("no command given"))?;
    let label = program.clone();
    let command_line = args.command.join(" ");

    if !admit(&args, &label)? {
        return Ok(ExitCode::from(EXIT_REFUSED));
    }

    sys::survive_ctrl_c();
    let (exe, lead_args) = launcher(program);
    let mut child = Command::new(exe)
        .args(lead_args)
        .args(rest)
        .creation_flags(CREATE_SUSPENDED)
        .spawn()?;
    let pid = child.id();

    let soft = if args.hard { None } else { args.budget };
    let hard = if args.hard { args.budget } else { None };
    let job = Job::create(JobOptions {
        soft_limit: soft,
        hard_limit: hard,
        kill_on_close: !args.keep_children,
    })
    .and_then(|job| job.assign(HANDLE(child.as_raw_handle())).map(|()| job));

    let job = match job {
        Ok(job) => Some(Arc::new(job)),
        Err(e) if args.hard => {
            // A hard budget was asked for; never run without it.
            let _ = child.kill();
            return Err(io::Error::other(format!(
                "could not apply hard budget: {e}"
            )));
        }
        Err(e) => {
            eprintln!("optimus: warning: running {label} unguarded ({e})");
            None
        }
    };

    if let Err(e) = sys::resume_process(pid) {
        let _ = child.kill();
        return Err(e);
    }

    let record = RunRecord {
        pid,
        guard_pid: std::process::id(),
        command: command_line.clone(),
        budget: args.budget,
        hard: args.hard,
        started: state::now(),
    };
    let record_path = state::write_run(&record).ok();
    state::log_event("start", pid, &command_line, String::new());

    let done = Arc::new(AtomicBool::new(false));
    let soft_hits = Arc::new(AtomicU32::new(0));
    let hard_hits = Arc::new(AtomicU32::new(0));
    let monitor = job.clone().map(|job| {
        let (done, soft_hits, hard_hits) = (done.clone(), soft_hits.clone(), hard_hits.clone());
        let command_line = command_line.clone();
        let budget = args.budget;
        thread::spawn(move || {
            let mut over = false;
            while !done.load(Ordering::Relaxed) {
                match job.next_message(1000) {
                    Some((MSG_NOTIFICATION_LIMIT, _)) => {
                        soft_hits.fetch_add(1, Ordering::Relaxed);
                        over = true;
                        state::log_event(
                            "soft_limit",
                            pid,
                            &command_line,
                            format!("budget {}", budget.unwrap_or(0)),
                        );
                    }
                    Some((MSG_JOB_MEMORY_LIMIT, _)) => {
                        hard_hits.fetch_add(1, Ordering::Relaxed);
                        state::log_event(
                            "hard_limit",
                            pid,
                            &command_line,
                            format!("budget {}", budget.unwrap_or(0)),
                        );
                    }
                    _ => {}
                }
                if let (true, Some(limit)) = (over, soft) {
                    if (job_private(&job) as f64) < limit as f64 * REARM_RATIO
                        && job.arm_soft_limit(limit).is_ok()
                    {
                        over = false;
                    }
                }
            }
        })
    });

    let status = child.wait()?;
    done.store(true, Ordering::Relaxed);
    if let Some(monitor) = monitor {
        let _ = monitor.join();
    }

    let peak = job.as_ref().and_then(|j| j.peak_memory().ok());
    let leftovers = job
        .as_ref()
        .and_then(|j| j.active_processes().ok())
        .unwrap_or(0);
    let code = status.code().unwrap_or(1);

    let mut summary = format!("optimus: {label} exited with code {code}");
    if let Some(peak) = peak {
        summary += &format!(", peak {}", format_size(peak));
        if let Some(budget) = args.budget {
            summary += &format!(" of {} budget", format_size(budget));
        }
    }
    let soft_n = soft_hits.load(Ordering::Relaxed);
    if soft_n > 0 {
        summary += &format!(", went over budget {soft_n}x");
    }
    let hard_n = hard_hits.load(Ordering::Relaxed);
    if hard_n > 0 {
        summary += &format!(", hit the hard limit {hard_n}x");
    }
    if leftovers > 0 && !args.keep_children {
        summary += &format!(", ended {leftovers} leftover process(es)");
    }
    eprintln!("{summary}");
    state::log_event(
        "exit",
        pid,
        &command_line,
        format!(
            "code {code} peak {} leftovers {leftovers}",
            peak.unwrap_or(0)
        ),
    );

    if let Some(path) = record_path {
        let _ = std::fs::remove_file(path);
    }
    // Dropping the last job handle ends leftovers when kill-on-close is set.
    drop(job);
    Ok(ExitCode::from(code.clamp(0, 255) as u8))
}
