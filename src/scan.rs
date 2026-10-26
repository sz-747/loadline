//! Picks out the few escape sequences in the agent's output that must reach the real
//! terminal (or need an answer), while all output still goes to the emulator.
//!
//! - Input modes the agent turns on or off (arrow-key mode, mouse, bracketed paste,
//!   focus events), because the real terminal produces the keys Optimus forwards.
//! - Window title and progress (OSC 0, 2, 9;4), so the tab title keeps working.
//! - Cursor position queries (CSI 6n), answered from the emulator.

/// Private modes forwarded to the real terminal.
const FORWARDED_MODES: &[u32] = &[1, 1000, 1002, 1003, 1004, 1006, 2004];
const MAX_SEQ: usize = 4096;

#[derive(Debug, PartialEq, Eq)]
pub enum Passthrough {
    /// A complete sequence to write to the real terminal as-is.
    Forward(String),
    CursorQuery,
}

#[derive(Default)]
enum State {
    #[default]
    Ground,
    Esc,
    Csi(String),
    Osc(String),
    OscEsc(String),
}

#[derive(Default)]
pub struct Scanner {
    state: State,
}

impl Scanner {
    /// Scan a chunk. Sequences split across chunks are completed on later calls.
    pub fn scan(&mut self, text: &str) -> Vec<Passthrough> {
        let mut out = Vec::new();
        for c in text.chars() {
            self.state = match std::mem::take(&mut self.state) {
                State::Ground if c == '\x1b' => State::Esc,
                State::Ground => State::Ground,
                State::Esc => match c {
                    '[' => State::Csi(String::new()),
                    ']' => State::Osc(String::new()),
                    '\x1b' => State::Esc,
                    _ => State::Ground,
                },
                State::Csi(mut body) => {
                    if ('\x40'..='\x7e').contains(&c) {
                        if let Some(p) = csi(&body, c) {
                            out.push(p);
                        }
                        State::Ground
                    } else if c == '\x1b' {
                        State::Esc
                    } else if body.len() < MAX_SEQ {
                        body.push(c);
                        State::Csi(body)
                    } else {
                        State::Ground
                    }
                }
                State::Osc(mut body) => match c {
                    '\x07' => {
                        osc(&body, "\x07", &mut out);
                        State::Ground
                    }
                    '\x1b' => State::OscEsc(body),
                    _ if body.len() < MAX_SEQ => {
                        body.push(c);
                        State::Osc(body)
                    }
                    _ => State::Ground,
                },
                State::OscEsc(body) => {
                    if c == '\\' {
                        osc(&body, "\x1b\\", &mut out);
                    }
                    State::Ground
                }
            };
        }
        out
    }
}

fn csi(body: &str, final_char: char) -> Option<Passthrough> {
    if body == "6" && final_char == 'n' {
        return Some(Passthrough::CursorQuery);
    }
    let params = body.strip_prefix('?')?;
    if final_char != 'h' && final_char != 'l' {
        return None;
    }
    let kept: Vec<&str> = params
        .split(';')
        .filter(|p| p.parse().is_ok_and(|n| FORWARDED_MODES.contains(&n)))
        .collect();
    (!kept.is_empty())
        .then(|| Passthrough::Forward(format!("\x1b[?{}{final_char}", kept.join(";"))))
}

fn osc(body: &str, terminator: &str, out: &mut Vec<Passthrough>) {
    if body.starts_with("0;") || body.starts_with("2;") || body.starts_with("9;4;") {
        out.push(Passthrough::Forward(format!("\x1b]{body}{terminator}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fwd(s: &str) -> Passthrough {
        Passthrough::Forward(s.into())
    }

    #[test]
    fn forwards_input_modes_only() {
        let mut s = Scanner::default();
        assert_eq!(
            s.scan("hi\x1b[?2004h\x1b[?25l\x1b[?1;25;1006h\x1b[2J"),
            vec![fwd("\x1b[?2004h"), fwd("\x1b[?1;1006h")]
        );
        // Win32 input mode stays between Optimus and the agent's pseudoconsole.
        assert!(s.scan("\x1b[?9001h").is_empty());
    }

    #[test]
    fn titles_and_progress() {
        let mut s = Scanner::default();
        assert_eq!(
            s.scan("\x1b]0;Claude Code\x07\x1b]9;4;1;50\x1b\\\x1b]8;;http://x\x07"),
            vec![fwd("\x1b]0;Claude Code\x07"), fwd("\x1b]9;4;1;50\x1b\\")]
        );
    }

    #[test]
    fn cursor_query() {
        let mut s = Scanner::default();
        assert_eq!(s.scan("\x1b[6n"), vec![Passthrough::CursorQuery]);
    }

    #[test]
    fn sequences_split_across_chunks() {
        let mut s = Scanner::default();
        assert!(s.scan("abc\x1b").is_empty());
        assert!(s.scan("[?20").is_empty());
        assert_eq!(s.scan("04l rest"), vec![fwd("\x1b[?2004l")]);
        assert!(s.scan("\x1b]2;ti").is_empty());
        assert_eq!(s.scan("tle\x07"), vec![fwd("\x1b]2;title\x07")]);
    }
}
