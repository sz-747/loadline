//! Win32 memory and process facts. Everything Windows-specific outside `job` lives here.

use std::io;
use std::mem::size_of;

use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE};
use windows::Win32::System::Console::{
    GetConsoleMode, GetStdHandle, SetConsoleCtrlHandler, SetConsoleMode, CTRL_BREAK_EVENT,
    CTRL_C_EVENT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_OUTPUT_HANDLE,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, Thread32First, Thread32Next,
    PROCESSENTRY32W, TH32CS_SNAPPROCESS, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::ProcessStatus::{
    GetPerformanceInfo, GetProcessMemoryInfo, PERFORMANCE_INFORMATION, PROCESS_MEMORY_COUNTERS,
    PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, OpenThread, ResumeThread, PROCESS_QUERY_LIMITED_INFORMATION,
    THREAD_SUSPEND_RESUME,
};

use crate::admission::SystemMemory;
use crate::tree::Proc;

/// Closes a raw handle on drop.
pub struct OwnedHandle(pub HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn win_err(e: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(e.code().0)
}

pub fn system_memory() -> io::Result<SystemMemory> {
    unsafe {
        let mut perf = PERFORMANCE_INFORMATION::default();
        GetPerformanceInfo(&mut perf, size_of::<PERFORMANCE_INFORMATION>() as u32)
            .map_err(win_err)?;
        let mut status = MEMORYSTATUSEX {
            dwLength: size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        GlobalMemoryStatusEx(&mut status).map_err(win_err)?;
        let page = perf.PageSize as u64;
        Ok(SystemMemory {
            total_phys: status.ullTotalPhys,
            avail_phys: status.ullAvailPhys,
            commit_total: perf.CommitTotal as u64 * page,
            commit_limit: perf.CommitLimit as u64 * page,
        })
    }
}

/// `(private bytes, working set)` of one process, or `None` if it cannot be opened.
pub fn process_memory(pid: u32) -> Option<(u64, u64)> {
    unsafe {
        let handle = OwnedHandle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut counters = PROCESS_MEMORY_COUNTERS_EX {
            cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        GetProcessMemoryInfo(
            handle.0,
            &mut counters as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
            counters.cb,
        )
        .ok()?;
        Some((counters.PrivateUsage as u64, counters.WorkingSetSize as u64))
    }
}

/// Every process with its parent and memory. Processes we cannot open report 0 bytes.
pub fn snapshot_processes() -> io::Result<Vec<Proc>> {
    let mut out = Vec::new();
    unsafe {
        let snap = OwnedHandle(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(win_err)?);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap.0, &mut entry).is_ok();
        while ok {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let exe = String::from_utf16_lossy(&entry.szExeFile[..len]).to_ascii_lowercase();
            let name = exe.strip_suffix(".exe").unwrap_or(&exe).to_string();
            let (private, working_set) = process_memory(entry.th32ProcessID).unwrap_or((0, 0));
            out.push(Proc {
                pid: entry.th32ProcessID,
                ppid: entry.th32ParentProcessID,
                name,
                private,
                working_set,
            });
            ok = Process32NextW(snap.0, &mut entry).is_ok();
        }
    }
    Ok(out)
}

/// Resume every thread of a process created with `CREATE_SUSPENDED`. Returns how many
/// threads were resumed.
pub fn resume_process(pid: u32) -> io::Result<usize> {
    let mut resumed = 0;
    unsafe {
        let snap = OwnedHandle(CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).map_err(win_err)?);
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut ok = Thread32First(snap.0, &mut entry).is_ok();
        while ok {
            if entry.th32OwnerProcessID == pid {
                let thread = OwnedHandle(
                    OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID)
                        .map_err(win_err)?,
                );
                if ResumeThread(thread.0) != u32::MAX {
                    resumed += 1;
                }
            }
            ok = Thread32Next(snap.0, &mut entry).is_ok();
        }
    }
    if resumed == 0 {
        return Err(io::Error::other(format!(
            "no threads resumed for pid {pid}"
        )));
    }
    Ok(resumed)
}

pub fn is_alive(pid: u32) -> bool {
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let handle = OwnedHandle(handle);
        let mut code = 0u32;
        GetExitCodeProcess(handle.0, &mut code).is_ok() && code == STILL_ACTIVE.0 as u32
    }
}

unsafe extern "system" fn swallow_interrupt(ctrl: u32) -> BOOL {
    // Ctrl+C belongs to the agent (it interrupts a turn). Optimus must survive it, or
    // kill-on-close would take the agent down with it. Close/logoff still end us.
    BOOL::from(ctrl == CTRL_C_EVENT || ctrl == CTRL_BREAK_EVENT)
}

/// Keep Optimus alive on Ctrl+C. A handler routine is not inherited by children,
/// unlike `SetConsoleCtrlHandler(None, TRUE)`, so the agent still receives Ctrl+C.
pub fn survive_ctrl_c() {
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(swallow_interrupt), true);
    }
}

/// Turn on ANSI escape handling for stdout (needed by `top --watch` in conhost).
pub fn enable_vt() {
    unsafe {
        if let Ok(out) = GetStdHandle(STD_OUTPUT_HANDLE) {
            let mut mode = Default::default();
            if GetConsoleMode(out, &mut mode).is_ok() {
                let _ = SetConsoleMode(out, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
}
