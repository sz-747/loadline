# Decision surfaces

## DS-001 Soft or hard budget by default

- **Question:** when an agent passes its budget, warn or enforce?
- **Why it matters:** a hard limit makes Node/Bun agents crash mid-task; a soft one lets the machine run low.
- **Status:** Decided (2026-10-26).
- **Current behavior:** soft by default (notify via `top`, event log, exit summary). `--hard` opts in.
- **Evidence:** T3 Code PR #22724 (static cap throttled agents); real agents reach 2.6-5.7 GB.
- **Reopen trigger:** users report soft warnings arrive too late to prevent freezes.

## DS-002 What happens when an agent does not fit

- **Question:** refuse, queue, or start anyway?
- **Status:** Decided (2026-10-26).
- **Current behavior:** refuse with exit code 75 and the numbers; `--wait` queues; `--force` overrides.

## DS-003 Leftover processes at agent exit

- **Question:** end processes the agent leaves running (dev servers, MCP servers)?
- **Why it matters:** ending them fixes orphan leaks (Claude Code #96299) but can stop a server the user wanted kept.
- **Status:** Decided (2026-10-26).
- **Current behavior:** end them (kill-on-close). `--keep-children` opts out.

## DS-004 Name and home

- **Question:** keep the Optimus name and repository, or start fresh?
- **Status:** Decided (2026-10-26, maintainer).
- **Current behavior:** renamed to loadline (a ship's load line marks the most it can safely carry) and moved to its own repository, sz-747/loadline. Optimus keeps the old multiplexer.
- **Evidence:** "loadline" was free on crates.io with no conflicting tool on GitHub (checked 2026-10-26).

## DS-005 How the user learns about a soft-budget crossing mid-session

- **Question:** desktop toast, terminal line, or only `top`?
- **Why it matters:** writing into the agent's terminal corrupts its TUI; a toast needs a Windows app identity.
- **Status:** Decided (2026-10-26, maintainer chose the status bar).
- **Current behavior:** the status bar under the agent turns amber at 80% and red when over; `top`, event log and exit summary as well.
- **Reopen trigger:** users run with `--no-bar` and still need alerts; then add a toast.

## DS-006 How the memory guard is shown

- **Question:** status bar inside the agent's window, Windows Terminal split pane, or a full-screen dashboard?
- **Why it matters:** the bar means loadline hosts the agent in a pseudoconsole and redraws its screen (rendering risk); a split pane is Windows Terminal only; a dashboard needs a second window.
- **Status:** Decided (2026-10-26, maintainer).
- **Current behavior:** status bar on by default in an interactive console; `--no-bar` opts out; automatic plain mode when output is redirected. Lines scrolled off the agent's screen are pushed into the real terminal's scrollback.
- **Evidence:** `tests/bar.rs`; real Claude Code and Codex render correctly under the bar (2026-10-26).
- **Reopen trigger:** rendering bugs in a major agent that `avt` cannot handle.
