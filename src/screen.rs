//! Draws the emulated agent screen plus the status bar onto the real terminal.
//!
//! Every frame redraws all rows inside a synchronized update, so there is no flicker
//! and no state to drift. Lines that scroll off the top of the agent's screen are first
//! painted onto the top rows and scrolled into the real terminal's scrollback, so
//! scrolling up in Windows Terminal still shows the agent's history.

use std::fmt::Write as _;

use avt::{Color, Line, Pen, Vt};

const SYNC_START: &str = "\x1b[?2026h";
const SYNC_END: &str = "\x1b[?2026l";

fn push_color(out: &mut String, color: Color, base: u8) {
    match color {
        Color::Indexed(n) if n < 8 => {
            let _ = write!(out, ";{}", base + n);
        }
        Color::Indexed(n) if n < 16 => {
            let _ = write!(out, ";{}", base + 60 + n - 8);
        }
        Color::Indexed(n) => {
            let _ = write!(out, ";{};5;{n}", base + 8);
        }
        Color::RGB(c) => {
            let _ = write!(out, ";{};2;{};{};{}", base + 8, c.r, c.g, c.b);
        }
    }
}

/// A complete SGR sequence for `pen`, starting from a reset.
pub fn sgr(pen: &Pen) -> String {
    let mut out = String::from("\x1b[0");
    if pen.is_bold() {
        out.push_str(";1");
    }
    if pen.is_faint() {
        out.push_str(";2");
    }
    if pen.is_italic() {
        out.push_str(";3");
    }
    if pen.is_underline() {
        out.push_str(";4");
    }
    if pen.is_blink() {
        out.push_str(";5");
    }
    if pen.is_inverse() {
        out.push_str(";7");
    }
    if pen.is_strikethrough() {
        out.push_str(";9");
    }
    if let Some(fg) = pen.foreground() {
        push_color(&mut out, fg, 30);
    }
    if let Some(bg) = pen.background() {
        push_color(&mut out, bg, 40);
    }
    out.push('m');
    out
}

/// One line as text with SGR changes, trailing default blanks trimmed, ending with a
/// reset and erase-to-end-of-line.
pub fn render_line(line: &Line) -> String {
    let cells = line.cells();
    let end = cells
        .iter()
        .rposition(|c| !c.is_default())
        .map_or(0, |i| i + 1);
    let mut out = String::new();
    let mut pen: Option<Pen> = None;
    for cell in &cells[..end] {
        if cell.width() == 0 {
            continue; // right half of a wide character
        }
        if pen.as_ref() != Some(cell.pen()) {
            out.push_str(&sgr(cell.pen()));
            pen = Some(*cell.pen());
        }
        out.push(cell.char());
    }
    out.push_str("\x1b[0m\x1b[K");
    out
}

fn goto(out: &mut String, row: usize, col: usize) {
    let _ = write!(out, "\x1b[{};{}H", row + 1, col + 1);
}

/// Build one frame. `scrolled` are lines that left the agent's screen since the last
/// frame; `content_rows` is the agent's height; the bar goes on the row below it.
pub fn frame(vt: &Vt, scrolled: &[Line], content_rows: usize, bar: &str) -> String {
    let mut out = String::from(SYNC_START);
    out.push_str("\x1b[?25l");
    let bar_row = content_rows;

    for chunk in scrolled.chunks(content_rows.max(1)) {
        // Paint the departing lines at the top, then scroll them off into history.
        for (i, line) in chunk.iter().enumerate() {
            goto(&mut out, i, 0);
            out.push_str(&render_line(line));
        }
        goto(&mut out, bar_row, 0);
        out.push_str("\x1b[0m\x1b[K");
        for _ in chunk {
            out.push('\n');
        }
    }

    for (row, line) in vt.view().enumerate().take(content_rows) {
        goto(&mut out, row, 0);
        out.push_str(&render_line(line));
    }
    goto(&mut out, bar_row, 0);
    out.push_str(bar);

    let cursor = vt.cursor();
    goto(&mut out, cursor.row, cursor.col);
    if cursor.visible {
        out.push_str("\x1b[?25h");
    }
    out.push_str(SYNC_END);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vt(cols: usize, rows: usize) -> Vt {
        Vt::builder().size(cols, rows).scrollback_limit(0).build()
    }

    #[test]
    fn plain_line_is_trimmed() {
        let mut v = vt(10, 2);
        v.feed_str("hi");
        assert_eq!(render_line(v.line(0)), "\x1b[0mhi\x1b[0m\x1b[K");
        assert_eq!(render_line(v.line(1)), "\x1b[0m\x1b[K");
    }

    #[test]
    fn colors_and_attributes() {
        let mut v = vt(20, 1);
        v.feed_str("\x1b[1;31mA\x1b[0;38;5;200;48;2;1;2;3mB\x1b[0;7;92mC");
        assert_eq!(
            render_line(v.line(0)),
            "\x1b[0;1;31mA\x1b[0;38;5;200;48;2;1;2;3mB\x1b[0;7;92mC\x1b[0m\x1b[K"
        );
    }

    #[test]
    fn wide_characters_are_written_once() {
        let mut v = vt(10, 1);
        v.feed_str("\u{4f60}\u{597d}!");
        assert_eq!(
            render_line(v.line(0)),
            "\x1b[0m\u{4f60}\u{597d}!\x1b[0m\x1b[K"
        );
    }

    #[test]
    fn scrolled_lines_come_back_from_the_emulator() {
        let mut v = vt(10, 3);
        let scrolled: Vec<Line> = v.feed_str("a\r\nb\r\nc\r\nd\r\ne").scrollback.collect();
        let text: Vec<String> = scrolled
            .iter()
            .map(|l| l.text().trim_end().to_string())
            .collect();
        assert_eq!(text, vec!["a", "b"]);
    }

    #[test]
    fn frame_pushes_history_then_redraws() {
        let mut v = vt(10, 3);
        let scrolled: Vec<Line> = v.feed_str("a\r\nb\r\nc\r\nd").scrollback.collect();
        let f = frame(&v, &scrolled, 3, "BAR");
        assert!(f.starts_with(SYNC_START) && f.ends_with(SYNC_END));
        // "a" painted on row 1, then one newline from the bar row scrolls it away.
        let push = f.find("\x1b[1;1H\x1b[0ma").expect("history line painted");
        let scroll = f
            .find("\x1b[4;1H\x1b[0m\x1b[K\n")
            .expect("scroll from bar row");
        let redraw = f.find("\x1b[1;1H\x1b[0mb").expect("screen redrawn");
        assert!(push < scroll && scroll < redraw);
        assert!(f.contains("\x1b[4;1HBAR"));
        // Cursor ends after "d" on the last content row.
        assert!(f.contains("\x1b[3;2H\x1b[?25h"));
    }
}
