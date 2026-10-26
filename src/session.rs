//! The interactive loop behind the status bar: keys go to the agent's pseudoconsole,
//! its output goes through the emulator, and frames go to the real terminal.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use avt::{Line, Vt};

use crate::console::{self, InputReader, RawMode};
use crate::pty::ConPty;
use crate::scan::{Passthrough, Scanner};
use crate::screen;
use crate::statusbar::{self, BarData};

const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const RESIZE_POLL: Duration = Duration::from_millis(100);
const BAR_POLL: Duration = Duration::from_secs(1);
/// After the agent exits, keep reading this long for its last output.
const DRAIN: Duration = Duration::from_millis(200);
/// Modes the agent may have turned on in the real terminal; all off when we leave.
const RESET_MODES: &str =
    "\x1b[?1l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1006l\x1b[?2004l";

/// Rows the agent gets: everything but the bar.
pub fn content_rows(rows: u16) -> u16 {
    rows.saturating_sub(1).max(1)
}

enum Event {
    Output(Vec<u8>),
    OutputClosed,
    Bar(BarData),
}

/// Decode the complete UTF-8 prefix of `pending`, keeping an incomplete trailing
/// character for the next chunk. Invalid bytes become U+FFFD.
pub fn decode_utf8(pending: &mut Vec<u8>) -> String {
    let mut out = String::new();
    let mut start = 0;
    loop {
        match std::str::from_utf8(&pending[start..]) {
            Ok(s) => {
                out.push_str(s);
                pending.clear();
                return out;
            }
            Err(e) => {
                let valid = start + e.valid_up_to();
                out.push_str(std::str::from_utf8(&pending[start..valid]).unwrap_or_default());
                match e.error_len() {
                    Some(n) => {
                        out.push('\u{fffd}');
                        start = valid + n;
                    }
                    None => {
                        pending.drain(..valid);
                        return out;
                    }
                }
            }
        }
    }
}

fn new_vt(cols: u16, rows: u16) -> Vt {
    Vt::builder()
        .size(cols as usize, content_rows(rows) as usize)
        .scrollback_limit(0)
        .build()
}

/// Run until the agent exits. Returns its exit code.
pub fn run(pty: Arc<ConPty>, bar_source: Box<dyn Fn() -> BarData + Send>) -> io::Result<i32> {
    let (mut cols, mut rows, cursor_row) = console::size_and_cursor()?;
    let raw = RawMode::enter()?;
    let mut out = io::stdout().lock();

    // Scroll what was on screen into history, so the agent starts on a clean screen.
    write!(out, "\x1b[{rows};1H")?;
    for _ in 0..=cursor_row {
        out.write_all(b"\n")?;
    }
    out.flush()?;

    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));

    let reader_pty = pty.clone();
    let reader_tx = tx.clone();
    let reader = thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match reader_pty.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if reader_tx.send(Event::Output(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = reader_tx.send(Event::OutputClosed);
    });

    // Detached: it blocks in ReadConsoleW and ends with the process.
    let input_pty = pty.clone();
    let mut input = InputReader::new()?;
    thread::spawn(move || {
        while let Ok(bytes) = input.read() {
            if !bytes.is_empty() && input_pty.write(&bytes).is_err() {
                break;
            }
        }
    });

    let bar_tx = tx.clone();
    let bar_stop = stop.clone();
    let stats = thread::spawn(move || {
        while !bar_stop.load(Ordering::Relaxed) {
            if bar_tx.send(Event::Bar(bar_source())).is_err() {
                break;
            }
            let until = Instant::now() + BAR_POLL;
            while Instant::now() < until && !bar_stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(50));
            }
        }
    });
    drop(tx);

    let mut vt = new_vt(cols, rows);
    let mut scanner = Scanner::default();
    let mut pending = Vec::new();
    let mut scrolled: Vec<Line> = Vec::new();
    let mut bar = BarData::default();
    let mut dirty = true;
    let mut last_frame = Instant::now() - FRAME_INTERVAL;
    let mut last_resize_check = Instant::now();
    let mut exited_at: Option<Instant> = None;
    let mut closed = false;

    while !closed {
        match rx.recv_timeout(FRAME_INTERVAL) {
            Ok(Event::Output(bytes)) => {
                pending.extend_from_slice(&bytes);
                let text = decode_utf8(&mut pending);
                let found = scanner.scan(&text);
                scrolled.extend(vt.feed_str(&text).scrollback);
                for p in found {
                    match p {
                        Passthrough::Forward(seq) => out.write_all(seq.as_bytes())?,
                        Passthrough::CursorQuery => {
                            let c = vt.cursor();
                            let reply = format!("\x1b[{};{}R", c.row + 1, c.col + 1);
                            let _ = pty.write(reply.as_bytes());
                        }
                    }
                }
                dirty = true;
            }
            Ok(Event::Bar(data)) => {
                dirty |= data != bar;
                bar = data;
            }
            Ok(Event::OutputClosed) | Err(RecvTimeoutError::Disconnected) => closed = true,
            Err(RecvTimeoutError::Timeout) => {}
        }

        if last_resize_check.elapsed() >= RESIZE_POLL {
            last_resize_check = Instant::now();
            if let Ok((c, r, _)) = console::size_and_cursor() {
                if (c, r) != (cols, rows) {
                    (cols, rows) = (c, r);
                    let _ = pty.resize(cols, content_rows(rows));
                    let changes = vt.resize(cols as usize, content_rows(rows) as usize);
                    scrolled.extend(changes.scrollback);
                    out.write_all(b"\x1b[2J")?;
                    dirty = true;
                }
            }
        }

        if dirty && last_frame.elapsed() >= FRAME_INTERVAL {
            let bar_line = statusbar::render(&bar, cols as usize);
            let frame = screen::frame(&vt, &scrolled, content_rows(rows) as usize, &bar_line);
            out.write_all(frame.as_bytes())?;
            out.flush()?;
            scrolled.clear();
            dirty = false;
            last_frame = Instant::now();
        }

        match exited_at {
            None if pty.has_exited() => exited_at = Some(Instant::now()),
            // Closing the pseudoconsole lets the reader reach end of stream.
            Some(t) if t.elapsed() >= DRAIN => pty.shutdown(),
            _ => {}
        }
    }

    // Final frame, then hand the terminal back: bar row cleared, cursor on it.
    let frame = screen::frame(&vt, &scrolled, content_rows(rows) as usize, "");
    out.write_all(frame.as_bytes())?;
    write!(
        out,
        "\x1b[{};1H\x1b[0m\x1b[2K\x1b[?25h{RESET_MODES}",
        content_rows(rows) + 1
    )?;
    out.flush()?;
    drop(out);

    stop.store(true, Ordering::Relaxed);
    let _ = stats.join();
    let _ = reader.join();
    drop(raw);
    Ok(pty.exit_code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_split_across_chunks() {
        let euro = "\u{20ac}".as_bytes(); // 3 bytes
        let mut pending = vec![b'a', euro[0]];
        assert_eq!(decode_utf8(&mut pending), "a");
        assert_eq!(pending, vec![euro[0]]);
        pending.extend_from_slice(&euro[1..]);
        pending.push(b'b');
        assert_eq!(decode_utf8(&mut pending), "\u{20ac}b");
        assert!(pending.is_empty());
    }

    #[test]
    fn invalid_utf8_is_replaced() {
        let mut pending = vec![b'a', 0xff, b'b'];
        assert_eq!(decode_utf8(&mut pending), "a\u{fffd}b");
    }

    #[test]
    fn bar_takes_one_row() {
        assert_eq!(content_rows(30), 29);
        assert_eq!(content_rows(1), 1);
    }
}
