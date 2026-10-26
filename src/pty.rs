//! A Windows pseudoconsole (ConPTY) hosting the agent, so Loadline can draw the agent's
//! screen itself and keep the bottom line for the status bar.
//!
//! The child is created suspended: the caller assigns it to the job, then calls
//! [`ConPty::resume`]. Adapted from the multiplexer engine on the `backup/*` branches.

use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{
    ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, ResumeThread, UpdateProcThreadAttribute,
    WaitForSingleObject, CREATE_SUSPENDED, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;

fn win_err(e: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(e.code().0)
}

/// Quote one argument for `CommandLineToArgvW` / the MSVC runtime.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// The full command line for `program` + `args`. Batch files go through `cmd.exe`.
pub fn command_line(program: &Path, args: &[String]) -> String {
    let exe = program.to_string_lossy();
    let rest: String = args.iter().map(|a| format!(" {}", quote_arg(a))).collect();
    let is_batch = program
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
    if is_batch {
        // /s strips the outer quotes and keeps the rest verbatim; the script path is
        // always quoted so cmd never splits it.
        format!("cmd.exe /d /s /c \"\"{exe}\"{rest}\"")
    } else {
        format!("{}{rest}", quote_arg(&exe))
    }
}

pub struct ConPty {
    hpc: HPCON,
    input_write: HANDLE,
    output_read: HANDLE,
    proc_info: PROCESS_INFORMATION,
    attr_list: Vec<u8>,
    closed: AtomicBool,
}

unsafe impl Send for ConPty {}
unsafe impl Sync for ConPty {}

impl ConPty {
    /// Create a `cols` x `rows` pseudoconsole and start `cmdline` in it, suspended.
    pub fn spawn_suspended(cmdline: &str, cols: u16, rows: u16) -> io::Result<Self> {
        unsafe {
            let mut input_read = HANDLE::default();
            let mut input_write = HANDLE::default();
            CreatePipe(&mut input_read, &mut input_write, None, 0).map_err(win_err)?;
            let mut output_read = HANDLE::default();
            let mut output_write = HANDLE::default();
            CreatePipe(&mut output_read, &mut output_write, None, 0).map_err(win_err)?;

            let size = COORD {
                X: cols as i16,
                Y: rows as i16,
            };
            let hpc = CreatePseudoConsole(size, input_read, output_write, 0).map_err(win_err)?;

            let mut attr_size: usize = 0;
            let _ = InitializeProcThreadAttributeList(None, 1, None, &mut attr_size);
            let mut attr_list = vec![0u8; attr_size];
            let attr_ptr = LPPROC_THREAD_ATTRIBUTE_LIST(attr_list.as_mut_ptr() as *mut c_void);
            InitializeProcThreadAttributeList(Some(attr_ptr), 1, None, &mut attr_size)
                .map_err(win_err)?;
            // lpValue is the HPCON value itself, not its address.
            UpdateProcThreadAttribute(
                attr_ptr,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                Some(hpc.0 as *const c_void),
                size_of::<HPCON>(),
                None,
                None,
            )
            .map_err(win_err)?;

            let mut si = STARTUPINFOEXW::default();
            si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            si.lpAttributeList = attr_ptr;
            // Keep the child off our own console: INVALID std handles force the pseudoconsole.
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
            si.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
            si.StartupInfo.hStdError = INVALID_HANDLE_VALUE;

            let mut cmdline_w: Vec<u16> = cmdline.encode_utf16().chain([0]).collect();
            let mut proc_info = PROCESS_INFORMATION::default();
            let created = CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(cmdline_w.as_mut_ptr())),
                None,
                None,
                false,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED,
                None,
                PCWSTR::null(),
                &si.StartupInfo,
                &mut proc_info,
            );
            // conhost holds its own duplicates of the child-facing ends now.
            let _ = CloseHandle(input_read);
            let _ = CloseHandle(output_write);
            if let Err(e) = created {
                ClosePseudoConsole(hpc);
                let _ = CloseHandle(input_write);
                let _ = CloseHandle(output_read);
                DeleteProcThreadAttributeList(attr_ptr);
                return Err(win_err(e));
            }

            Ok(Self {
                hpc,
                input_write,
                output_read,
                proc_info,
                attr_list,
                closed: AtomicBool::new(false),
            })
        }
    }

    pub fn pid(&self) -> u32 {
        self.proc_info.dwProcessId
    }

    /// Borrowed process handle (closed when the `ConPty` drops).
    pub fn process(&self) -> HANDLE {
        self.proc_info.hProcess
    }

    pub fn resume(&self) -> io::Result<()> {
        if unsafe { ResumeThread(self.proc_info.hThread) } == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Kill the child (used when setup fails before it ever ran).
    pub fn terminate(&self) {
        unsafe {
            let _ = windows::Win32::System::Threading::TerminateProcess(self.proc_info.hProcess, 1);
        }
    }

    pub fn has_exited(&self) -> bool {
        unsafe { WaitForSingleObject(self.proc_info.hProcess, 0) == WAIT_OBJECT_0 }
    }

    pub fn exit_code(&self) -> Option<i32> {
        let mut code = 0u32;
        unsafe { GetExitCodeProcess(self.proc_info.hProcess, &mut code).ok()? };
        (code != 259).then_some(code as i32)
    }

    pub fn write(&self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let mut written = 0u32;
            unsafe { WriteFile(self.input_write, Some(data), Some(&mut written), None) }
                .map_err(win_err)?;
            data = &data[written as usize..];
        }
        Ok(())
    }

    /// Blocking read of the child's VT output. `Ok(0)` at end of stream.
    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut read = 0u32;
        match unsafe { ReadFile(self.output_read, Some(buf), Some(&mut read), None) } {
            Ok(()) => Ok(read as usize),
            Err(e) if e.code() == ERROR_BROKEN_PIPE.to_hresult() => Ok(0),
            Err(e) => Err(win_err(e)),
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let size = COORD {
            X: cols as i16,
            Y: rows as i16,
        };
        unsafe { ResizePseudoConsole(self.hpc, size) }.map_err(win_err)
    }

    /// Close the pseudoconsole so a blocked reader sees end of stream. Idempotent.
    pub fn shutdown(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            unsafe { ClosePseudoConsole(self.hpc) };
        }
    }
}

impl Drop for ConPty {
    fn drop(&mut self) {
        self.shutdown();
        unsafe {
            let _ = CloseHandle(self.input_write);
            let _ = CloseHandle(self.output_read);
            let _ = CloseHandle(self.proc_info.hProcess);
            let _ = CloseHandle(self.proc_info.hThread);
            DeleteProcThreadAttributeList(LPPROC_THREAD_ATTRIBUTE_LIST(
                self.attr_list.as_mut_ptr() as *mut c_void,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn quotes_like_msvcrt() {
        assert_eq!(quote_arg("plain"), "plain");
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg("two words"), "\"two words\"");
        assert_eq!(quote_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote_arg(r"C:\dir with space\"), r#""C:\dir with space\\""#);
    }

    #[test]
    fn batch_files_go_through_cmd() {
        let line = command_line(
            &PathBuf::from(r"C:\npm\claude.cmd"),
            &["--model".into(), "a b".into()],
        );
        assert_eq!(
            line,
            r#"cmd.exe /d /s /c ""C:\npm\claude.cmd" --model "a b"""#
        );
        let line = command_line(&PathBuf::from(r"C:\bin\codex.exe"), &["x".into()]);
        assert_eq!(line, r"C:\bin\codex.exe x");
    }
}
