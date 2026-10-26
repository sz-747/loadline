//! A Windows Job Object around one agent and every process it starts.
//!
//! - Kill-on-close (default): when Optimus exits, the OS ends the whole tree, so no
//!   orphaned agent or MCP processes are left holding memory.
//! - Soft budget: a job-wide notification limit. Windows posts a message to our
//!   completion port when the tree's committed memory crosses it; nothing is killed.
//! - Hard budget (opt-in): a job-wide memory limit. Allocations past it fail inside the
//!   agent, which usually crashes it - that is the point, the machine stays up.
//!
//! Limits are on committed (private) memory of the whole job, not per process.

use std::ffi::c_void;
use std::io;
use std::mem::size_of;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectAssociateCompletionPortInformation,
    JobObjectBasicAccountingInformation, JobObjectBasicProcessIdList,
    JobObjectExtendedLimitInformation, JobObjectNotificationLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, JOBOBJECTINFOCLASS,
    JOBOBJECT_ASSOCIATE_COMPLETION_PORT, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOBOBJECT_NOTIFICATION_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT, JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::IO::{CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED};

// Job completion-port message ids (winnt.h).
pub const MSG_JOB_MEMORY_LIMIT: u32 = 10;
pub const MSG_NOTIFICATION_LIMIT: u32 = 11;

#[derive(Clone, Copy, Debug, Default)]
pub struct JobOptions {
    pub soft_limit: Option<u64>,
    pub hard_limit: Option<u64>,
    pub kill_on_close: bool,
}

pub struct Job {
    handle: HANDLE,
    port: HANDLE,
}

// Both handles are kernel objects; Win32 job and port calls are thread-safe.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

fn win_err(e: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(e.code().0)
}

unsafe fn set_info<T>(job: HANDLE, class: JOBOBJECTINFOCLASS, info: &T) -> io::Result<()> {
    SetInformationJobObject(
        job,
        class,
        info as *const T as *const c_void,
        size_of::<T>() as u32,
    )
    .map_err(win_err)
}

impl Job {
    pub fn create(options: JobOptions) -> io::Result<Self> {
        unsafe {
            let handle = CreateJobObjectW(None, PCWSTR::null()).map_err(win_err)?;
            let port = match CreateIoCompletionPort(INVALID_HANDLE_VALUE, None, 0, 1) {
                Ok(port) => port,
                Err(e) => {
                    let _ = CloseHandle(handle);
                    return Err(win_err(e));
                }
            };
            let job = Self { handle, port };

            let associate = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
                CompletionKey: std::ptr::null_mut(),
                CompletionPort: port,
            };
            set_info(
                handle,
                JobObjectAssociateCompletionPortInformation,
                &associate,
            )?;

            // Explicit breakaway requests (CREATE_BREAKAWAY_FROM_JOB) are allowed so tools
            // that use them keep working; they leave our accounting, which is the lesser harm.
            let mut flags: JOB_OBJECT_LIMIT = JOB_OBJECT_LIMIT_BREAKAWAY_OK;
            let mut extended = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            if options.kill_on_close {
                flags |= JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            }
            if let Some(limit) = options.hard_limit {
                flags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
                extended.JobMemoryLimit = limit as usize;
            }
            extended.BasicLimitInformation.LimitFlags = flags;
            set_info(handle, JobObjectExtendedLimitInformation, &extended)?;

            if let Some(limit) = options.soft_limit {
                job.arm_soft_limit(limit)?;
            }
            Ok(job)
        }
    }

    /// (Re)arm the soft budget. Windows notifies once per crossing; re-arming after the
    /// tree drops back under the budget lets the next crossing notify again.
    pub fn arm_soft_limit(&self, limit: u64) -> io::Result<()> {
        let info = JOBOBJECT_NOTIFICATION_LIMIT_INFORMATION {
            JobMemoryLimit: limit,
            LimitFlags: JOB_OBJECT_LIMIT_JOB_MEMORY,
            ..Default::default()
        };
        unsafe { set_info(self.handle, JobObjectNotificationLimitInformation, &info) }
    }

    pub fn assign(&self, process: HANDLE) -> io::Result<()> {
        unsafe { AssignProcessToJobObject(self.handle, process).map_err(win_err) }
    }

    fn extended(&self) -> io::Result<JOBOBJECT_EXTENDED_LIMIT_INFORMATION> {
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        unsafe {
            QueryInformationJobObject(
                Some(self.handle),
                JobObjectExtendedLimitInformation,
                &mut info as *mut _ as *mut c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                None,
            )
            .map_err(win_err)?;
        }
        Ok(info)
    }

    /// Highest committed memory the whole job has used.
    pub fn peak_memory(&self) -> io::Result<u64> {
        Ok(self.extended()?.PeakJobMemoryUsed as u64)
    }

    /// Number of processes currently in the job.
    pub fn active_processes(&self) -> io::Result<u32> {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        unsafe {
            QueryInformationJobObject(
                Some(self.handle),
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as *mut c_void,
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                None,
            )
            .map_err(win_err)?;
        }
        Ok(info.ActiveProcesses)
    }

    /// Pids currently in the job (up to 4096).
    pub fn member_pids(&self) -> io::Result<Vec<u32>> {
        const MAX: usize = 4096;
        // JOBOBJECT_BASIC_PROCESS_ID_LIST: two u32 counts, then an array of ULONG_PTR.
        let mut buf = vec![0usize; 1 + MAX];
        unsafe {
            QueryInformationJobObject(
                Some(self.handle),
                JobObjectBasicProcessIdList,
                buf.as_mut_ptr() as *mut c_void,
                (buf.len() * size_of::<usize>()) as u32,
                None,
            )
            .map_err(win_err)?;
        }
        let listed = (buf[0] >> 32) as usize;
        Ok(buf[1..1 + listed.min(MAX)]
            .iter()
            .map(|&p| p as u32)
            .collect())
    }

    /// Block up to `timeout_ms` for the next job message. Returns `(message id, pid)`.
    pub fn next_message(&self, timeout_ms: u32) -> Option<(u32, usize)> {
        let mut message = 0u32;
        let mut key = 0usize;
        let mut overlapped: *mut OVERLAPPED = std::ptr::null_mut();
        unsafe {
            GetQueuedCompletionStatus(
                self.port,
                &mut message,
                &mut key,
                &mut overlapped,
                timeout_ms,
            )
            .ok()?;
        }
        Some((message, overlapped as usize))
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            // With kill-on-close set, closing the last job handle ends every process left.
            let _ = CloseHandle(self.handle);
            let _ = CloseHandle(self.port);
        }
    }
}
