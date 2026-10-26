//! `optimus run`: admission check, then start the agent suspended inside a job, resume
//! it, watch its memory, and end anything it leaves behind.
//!
//! In an interactive console the agent runs in a pseudoconsole with the status bar
//! under it (`session`); otherwise it inherits the console directly.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::Win32::Foundation::HANDLE;

use crate::admission::{self, Admission};
use crate::job::{Job, JobOptions, MSG_JOB_MEMORY_LIMIT, MSG_NOTIFICATION_LIMIT};
use crate::pty::{self, ConPty};
use crate::resolve::launcher;
use crate::size::format_size;
use crate::state::{self, RunRecord};
use crate::statusbar::BarData;
use crate::{console, session, sys, top};

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
    pub no_bar: bool,
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

/// Everything that watches one guarded agent: its job, run record, event monitor and
/// limit counters.
struct Guard {
    job: Option<Arc<Job>>,
    pid: u32,
    label: String,
    command_line: String,
    budget: Option<u64>,
    keep_children: bool,
    record_path: Option<PathBuf>,
    done: Arc<AtomicBool>,
    soft_hits: Arc<AtomicU32>,
    hard_hits: Arc<AtomicU32>,
    monitor: Option<JoinHandle<()>>,
}

impl Guard {
    fn start(job: Option<Arc<Job>>, pid: u32, label: &str, args: &RunArgs) -> Self {
        let command_line = args.command.join(" ");
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
        let soft = if args.hard { None } else { args.budget };
        let monitor = job.clone().map(|job| {
            let (done, soft_hits, hard_hits) = (done.clone(), soft_hits.clone(), hard_hits.clone());
            let command_line = command_line.clone();
            let budget = args.budget.unwrap_or(0);
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
                                format!("budget {budget}"),
                            );
                        }
                        Some((MSG_JOB_MEMORY_LIMIT, _)) => {
                            hard_hits.fetch_add(1, Ordering::Relaxed);
                            state::log_event(
                                "hard_limit",
                                pid,
                                &command_line,
                                format!("budget {budget}"),
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

        Self {
            job,
            pid,
            label: label.to_string(),
            command_line,
            budget: args.budget,
            keep_children: args.keep_children,
            record_path,
            done,
            soft_hits,
            hard_hits,
            monitor,
        }
    }

    /// Print the summary, remove the run record, and end leftovers (by dropping the job).
    fn finish(mut self, code: i32) -> ExitCode {
        self.done.store(true, Ordering::Relaxed);
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.join();
        }
        let peak = self.job.as_ref().and_then(|j| j.peak_memory().ok());
        let leftovers = self
            .job
            .as_ref()
            .and_then(|j| j.active_processes().ok())
            .unwrap_or(0);

        let mut summary = format!("optimus: {} exited with code {code}", self.label);
        if let Some(peak) = peak {
            summary += &format!(", peak {}", format_size(peak));
            if let Some(budget) = self.budget {
                summary += &format!(" of {} budget", format_size(budget));
            }
        }
        let soft_n = self.soft_hits.load(Ordering::Relaxed);
        if soft_n > 0 {
            summary += &format!(", went over budget {soft_n}x");
        }
        let hard_n = self.hard_hits.load(Ordering::Relaxed);
        if hard_n > 0 {
            summary += &format!(", hit the hard limit {hard_n}x");
        }
        if leftovers > 0 && !self.keep_children {
            summary += &format!(", ended {leftovers} leftover process(es)");
        }
        eprintln!("{summary}");
        state::log_event(
            "exit",
            self.pid,
            &self.command_line,
            format!(
                "code {code} peak {} leftovers {leftovers}",
                peak.unwrap_or(0)
            ),
        );
        if let Some(path) = self.record_path.take() {
            let _ = std::fs::remove_file(path);
        }
        // Dropping the last job handle ends leftovers when kill-on-close is set.
        self.job = None;
        ExitCode::from(code.clamp(0, 255) as u8)
    }
}

/// Assign a freshly spawned (suspended) child to the job. A failed hard budget is fatal;
/// a failed soft one degrades to running unguarded.
fn enroll(
    job: Option<Job>,
    process: HANDLE,
    args: &RunArgs,
    label: &str,
) -> io::Result<Option<Arc<Job>>> {
    let Some(job) = job else {
        return Ok(None);
    };
    match job.assign(process) {
        Ok(()) => Ok(Some(Arc::new(job))),
        Err(e) if args.hard => Err(io::Error::other(format!(
            "could not apply hard budget: {e}"
        ))),
        Err(e) => {
            eprintln!("optimus: warning: running {label} unguarded ({e})");
            Ok(None)
        }
    }
}

fn create_job(args: &RunArgs, label: &str) -> io::Result<Option<Job>> {
    let options = JobOptions {
        soft_limit: if args.hard { None } else { args.budget },
        hard_limit: if args.hard { args.budget } else { None },
        kill_on_close: !args.keep_children,
    };
    match Job::create(options) {
        Ok(job) => Ok(Some(job)),
        Err(e) if args.hard => Err(io::Error::other(format!(
            "could not apply hard budget: {e}"
        ))),
        Err(e) => {
            eprintln!("optimus: warning: running {label} unguarded ({e})");
            Ok(None)
        }
    }
}

pub fn run(args: RunArgs) -> io::Result<ExitCode> {
    let (program, rest) = args
        .command
        .split_first()
        .ok_or_else(|| io::Error::other("no command given"))?;
    let label = program.clone();

    if !admit(&args, &label)? {
        return Ok(ExitCode::from(EXIT_REFUSED));
    }

    sys::survive_ctrl_c();
    let job = create_job(&args, &label)?;
    let (exe, lead_args) = launcher(program);
    let mut argv = lead_args;
    argv.extend(rest.iter().cloned());

    if !args.no_bar && console::is_interactive() {
        return run_with_bar(&args, &label, job, exe, argv);
    }

    let mut child = Command::new(exe)
        .args(argv)
        .creation_flags(CREATE_SUSPENDED)
        .spawn()?;
    let job = match enroll(job, HANDLE(child.as_raw_handle()), &args, &label) {
        Ok(job) => job,
        Err(e) => {
            let _ = child.kill();
            return Err(e);
        }
    };
    if let Err(e) = sys::resume_process(child.id()) {
        let _ = child.kill();
        return Err(e);
    }
    let guard = Guard::start(job, child.id(), &label, &args);
    let status = child.wait()?;
    Ok(guard.finish(status.code().unwrap_or(1)))
}

fn run_with_bar(
    args: &RunArgs,
    label: &str,
    job: Option<Job>,
    exe: PathBuf,
    argv: Vec<String>,
) -> io::Result<ExitCode> {
    let (cols, rows, _) = console::size_and_cursor()?;
    let cmdline = pty::command_line(&exe, &argv);
    let pty = Arc::new(ConPty::spawn_suspended(
        &cmdline,
        cols,
        session::content_rows(rows),
    )?);
    let job = match enroll(job, pty.process(), args, label) {
        Ok(job) => job,
        Err(e) => {
            pty.terminate();
            return Err(e);
        }
    };
    if let Err(e) = pty.resume() {
        pty.terminate();
        return Err(e);
    }
    let guard = Guard::start(job.clone(), pty.pid(), label, args);

    let (budget, hard, reserve) = (args.budget, args.hard, args.reserve);
    let source = move || {
        let committed = job.as_deref().map(job_private).unwrap_or(0);
        let report = top::collect(reserve).ok();
        BarData {
            committed,
            budget,
            hard,
            safe_zone: report.as_ref().map_or(0, |r| r.safe_zone),
            agents: report.map_or(1, |r| r.agents.len().max(1)),
        }
    };
    let code = session::run(pty, Box::new(source))?;
    Ok(guard.finish(code))
}
