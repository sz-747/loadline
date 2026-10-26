//! The one-line memory bar under the agent.

use crate::size::format_size;

const GB: u64 = 1024 * 1024 * 1024;
const METER: usize = 16;

// Dark, quiet base with a restrained green; amber and red only when it matters.
const BASE: &str = "\x1b[0;48;5;235;38;5;250m";
const DIM: &str = "\x1b[38;5;242m";
const LABEL: &str = "\x1b[38;5;71;1m";
const GREEN: &str = "\x1b[22;38;5;71m";
const AMBER: &str = "\x1b[22;38;5;214m";
const RED: &str = "\x1b[22;38;5;203m";
const TEXT: &str = "\x1b[22;38;5;250m";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BarData {
    /// Committed memory of the guarded agent tree.
    pub committed: u64,
    pub budget: Option<u64>,
    pub hard: bool,
    /// Memory still safe for new agents on this PC.
    pub safe_zone: u64,
    /// Agents running on this PC, this one included.
    pub agents: usize,
}

fn level_color(pct: u64) -> &'static str {
    match pct {
        0..=79 => GREEN,
        80..=99 => AMBER,
        _ => RED,
    }
}

struct Seg {
    styled: String,
    width: usize,
}

fn seg(parts: &[(&str, &str)]) -> Seg {
    let mut styled = String::new();
    let mut width = 0;
    for (style, text) in parts {
        styled.push_str(style);
        styled.push_str(text);
        width += text.chars().count();
    }
    Seg { styled, width }
}

/// Render the bar for a terminal `cols` wide. The visible text never exceeds
/// `cols - 1` cells (writing the last cell of the last row can scroll some consoles);
/// the background is extended to the edge with erase-to-end-of-line.
pub fn render(data: &BarData, cols: usize) -> String {
    let max = cols.saturating_sub(1);
    let mut segments = vec![seg(&[(LABEL, " optimus ")])];

    let usage = match data.budget {
        Some(budget) => {
            let pct = data.committed.saturating_mul(100) / budget.max(1);
            let filled = ((pct as usize).min(100) * METER).div_ceil(100);
            let color = level_color(pct);
            let mode = if data.hard { "hard" } else { "soft" };
            let state = if pct >= 100 { " OVER" } else { "" };
            let figures = format!(
                " {}/{} {mode}{state}",
                format_size(data.committed),
                format_size(budget)
            );
            let meter_on = "\u{2588}".repeat(filled);
            let meter_off = "\u{2591}".repeat(METER - filled);
            vec![
                seg(&[
                    (DIM, "\u{2595}"),
                    (color, &meter_on),
                    (DIM, &meter_off),
                    (DIM, "\u{258f}"),
                ]),
                seg(&[(color, &figures)]),
            ]
        }
        None => vec![seg(&[(
            TEXT,
            &format!("{} committed", format_size(data.committed)),
        )])],
    };
    segments.extend(usage);

    let pc_color = if data.safe_zone == 0 {
        RED
    } else if data.safe_zone < GB {
        AMBER
    } else {
        TEXT
    };
    let optional = [
        seg(&[
            (DIM, " \u{2502} "),
            (
                pc_color,
                &format!("PC {} safe", format_size(data.safe_zone)),
            ),
        ]),
        seg(&[
            (DIM, " \u{2502} "),
            (TEXT, &format!("{} agents", data.agents)),
        ]),
    ];

    let mut width: usize = segments.iter().map(|s| s.width).sum();
    if width > max {
        // Too narrow even for the essentials: show only the label and figures.
        segments.retain(|s| !s.styled.contains('\u{2588}') && !s.styled.contains('\u{2591}'));
        width = segments.iter().map(|s| s.width).sum();
    }
    for s in optional {
        if width + s.width <= max {
            width += s.width;
            segments.push(s);
        }
    }

    let mut out = String::from(BASE);
    let mut used = 0;
    for s in &segments {
        if used + s.width > max {
            break;
        }
        out.push_str(&s.styled);
        used += s.width;
    }
    out.push_str(BASE);
    out.push_str("\x1b[K\x1b[0m");
    out
}

/// Visible text of a rendered bar (escape sequences removed). For tests and logs.
pub fn plain(rendered: &str) -> String {
    let mut out = String::new();
    let mut chars = rendered.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(committed: u64, budget: Option<u64>) -> BarData {
        BarData {
            committed,
            budget,
            hard: false,
            safe_zone: 3 * GB / 2,
            agents: 3,
        }
    }

    #[test]
    fn full_bar_on_wide_terminal() {
        let text = plain(&render(&data(GB, Some(2 * GB)), 120));
        assert_eq!(
            text,
            " optimus \u{2595}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{258f} 1.0 GB/2.0 GB soft \u{2502} PC 1.5 GB safe \u{2502} 3 agents"
        );
    }

    #[test]
    fn colors_follow_usage() {
        assert!(render(&data(GB, Some(2 * GB)), 120).contains(GREEN));
        assert!(render(&data(17 * GB / 10, Some(2 * GB)), 120).contains(AMBER));
        let over = render(&data(3 * GB, Some(2 * GB)), 120);
        assert!(over.contains(RED));
        assert!(plain(&over).contains("soft OVER"));
    }

    #[test]
    fn never_wider_than_terminal() {
        for cols in [10, 20, 30, 45, 60, 80, 200] {
            let text = plain(&render(&data(GB, Some(2 * GB)), cols));
            assert!(text.chars().count() < cols, "cols {cols}: {text:?}");
        }
    }

    #[test]
    fn narrow_terminal_drops_meter_first() {
        let text = plain(&render(&data(GB, Some(2 * GB)), 40));
        assert!(!text.contains('\u{2588}'));
        assert!(text.contains("1.0 GB/2.0 GB soft"));
    }

    #[test]
    fn without_budget() {
        let text = plain(&render(&data(GB, None), 120));
        assert!(text.contains("1.0 GB committed"));
    }
}
