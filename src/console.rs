//! The real console Optimus runs in: raw VT input, UTF-8, size, and restoring it all.

use std::io;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Console::{
    GetConsoleCP, GetConsoleMode, GetConsoleOutputCP, GetConsoleScreenBufferInfo, GetStdHandle,
    ReadConsoleW, SetConsoleCP, SetConsoleMode, SetConsoleOutputCP, CONSOLE_MODE,
    CONSOLE_SCREEN_BUFFER_INFO, DISABLE_NEWLINE_AUTO_RETURN, ENABLE_EXTENDED_FLAGS,
    ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
    STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};

const CP_UTF8: u32 = 65001;

fn handles() -> Option<(HANDLE, HANDLE)> {
    unsafe {
        Some((
            GetStdHandle(STD_INPUT_HANDLE).ok()?,
            GetStdHandle(STD_OUTPUT_HANDLE).ok()?,
        ))
    }
}

/// True when both stdin and stdout are a real console (not redirected).
pub fn is_interactive() -> bool {
    let Some((input, output)) = handles() else {
        return false;
    };
    let mut mode = CONSOLE_MODE::default();
    unsafe { GetConsoleMode(input, &mut mode).is_ok() && GetConsoleMode(output, &mut mode).is_ok() }
}

/// Visible window size as (cols, rows), and the cursor row within the window.
pub fn size_and_cursor() -> io::Result<(u16, u16, u16)> {
    let (_, output) = handles().ok_or_else(|| io::Error::other("no console"))?;
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    unsafe { GetConsoleScreenBufferInfo(output, &mut info) }.map_err(io::Error::other)?;
    let w = info.srWindow;
    let cols = (w.Right - w.Left + 1).max(1) as u16;
    let rows = (w.Bottom - w.Top + 1).max(1) as u16;
    let cursor_row = (info.dwCursorPosition.Y - w.Top).clamp(0, rows as i16 - 1) as u16;
    Ok((cols, rows, cursor_row))
}

/// Raw mode for the lifetime of the guard: keys arrive as VT sequences (Ctrl+C as a
/// byte, not a signal), no echo or line editing, UTF-8 both ways. Restored on drop.
pub struct RawMode {
    input: HANDLE,
    output: HANDLE,
    input_mode: CONSOLE_MODE,
    output_mode: CONSOLE_MODE,
    input_cp: u32,
    output_cp: u32,
}

impl RawMode {
    pub fn enter() -> io::Result<Self> {
        let (input, output) = handles().ok_or_else(|| io::Error::other("no console"))?;
        unsafe {
            let mut input_mode = CONSOLE_MODE::default();
            let mut output_mode = CONSOLE_MODE::default();
            GetConsoleMode(input, &mut input_mode).map_err(io::Error::other)?;
            GetConsoleMode(output, &mut output_mode).map_err(io::Error::other)?;
            let guard = Self {
                input,
                output,
                input_mode,
                output_mode,
                input_cp: GetConsoleCP(),
                output_cp: GetConsoleOutputCP(),
            };
            // EXTENDED_FLAGS without QUICK_EDIT keeps mouse clicks from freezing output.
            SetConsoleMode(input, ENABLE_VIRTUAL_TERMINAL_INPUT | ENABLE_EXTENDED_FLAGS)
                .map_err(io::Error::other)?;
            SetConsoleMode(
                output,
                ENABLE_PROCESSED_OUTPUT
                    | ENABLE_VIRTUAL_TERMINAL_PROCESSING
                    | DISABLE_NEWLINE_AUTO_RETURN,
            )
            .map_err(io::Error::other)?;
            let _ = SetConsoleCP(CP_UTF8);
            let _ = SetConsoleOutputCP(CP_UTF8);
            Ok(guard)
        }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        unsafe {
            let _ = SetConsoleMode(self.input, self.input_mode);
            let _ = SetConsoleMode(self.output, self.output_mode);
            let _ = SetConsoleCP(self.input_cp);
            let _ = SetConsoleOutputCP(self.output_cp);
        }
    }
}

/// Reads keyboard input as UTF-8 bytes (VT sequences included).
pub struct InputReader {
    input: HANDLE,
    pending_high: Option<u16>,
}

unsafe impl Send for InputReader {}

impl InputReader {
    pub fn new() -> io::Result<Self> {
        let (input, _) = handles().ok_or_else(|| io::Error::other("no console"))?;
        Ok(Self {
            input,
            pending_high: None,
        })
    }

    /// Block for the next input; returns UTF-8 bytes (possibly empty).
    pub fn read(&mut self) -> io::Result<Vec<u8>> {
        let mut buf = [0u16; 512];
        let mut read = 0u32;
        unsafe {
            ReadConsoleW(
                self.input,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut read,
                None,
            )
        }
        .map_err(io::Error::other)?;
        let mut units: Vec<u16> = self.pending_high.take().into_iter().collect();
        units.extend_from_slice(&buf[..read as usize]);
        // Keep a trailing unpaired high surrogate for the next read.
        if let Some(&last) = units.last() {
            if (0xD800..0xDC00).contains(&last) {
                self.pending_high = units.pop();
            }
        }
        Ok(String::from_utf16_lossy(&units).into_bytes())
    }
}
