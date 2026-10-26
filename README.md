# loadline

**A memory guard for AI coding agents on Windows.**

A ship's load line marks how much it can safely carry. loadline does the same for your PC when you run several coding agents at once.

Run three Claude Code or Codex sessions on a 16 GB laptop and Windows starts paging, freezing or killing things. Task Manager shows each agent using a few hundred MB, so it is hard to see why. The number that runs Windows out of memory is *committed* memory, and agents commit far more than their working set suggests.

loadline wraps each agent in a Windows Job Object:

- **Budget per agent.** `--budget 3G` covers the agent and every process it starts (Node, ripgrep, MCP servers). Soft by default: you are told, nothing is killed. `--hard` makes the budget a wall.
- **Admission control.** loadline refuses to start an agent the machine cannot fit, or waits for room with `--wait`.
- **No orphans.** When the agent exits, anything it left running is ended.
- **A live memory bar under the agent.** One line at the bottom of the same window shows the session's name (from the title the agent sets, e.g. Claude Code's task name, shortened), its committed memory against its budget, how much room the PC has left, and how many agents are running. Green, amber at 80%, red when over.
- **Real numbers.** `loadline top` shows committed memory per agent tree, plus how much is still safe to use.

It works in any terminal or multiplexer: Windows Terminal, VS Code, Orca, herdr, plain PowerShell.

## Quick start

Needs Windows 10 1809 or later and a [Rust toolchain](https://rustup.rs).

```powershell
cargo install --git https://github.com/sz-747/loadline

loadline run --budget 3G -- claude
loadline run --budget 2G --hard -- codex
```

Claude Code runs as usual, with the bar underneath:

```
╭─ Claude Code ──────────────────────────────────────────╮
│ > fix the failing test in run.rs                       │
│ ● Editing src/run.rs (+12 -3)                          │
╰────────────────────────────────────────────────────────╯
 Fix failing test ▕██████████░░░░░░▏ 2.1 GB/3.0 GB soft │ PC 1.4 GB safe │ 3 agents
```

For every agent at once, run `loadline top` (add `--watch` to refresh):

```
RAM 16.0 GB | free 3.1 GB | commit 20.4 GB / 24.0 GB (headroom 3.6 GB) | safe for new agents 1.1 GB (keeping 2.0 GB for Windows)

AGENT              PID  PROCS  COMMITTED  WORKING SET     BUDGET  STATUS
claude           50824     17     3.4 GB       1.1 GB     3.0 GB  OVER 113% (soft)
codex            31428     18     1.2 GB     760 MB     2.0 GB  60% (hard)
claude           26828     13     749 MB     531 MB          -  unguarded
```

(Illustrative 16 GB laptop. Run `loadline top` to see your own numbers.)

## Commands

| Command | What it does |
|---|---|
| `loadline run [options] -- <agent> [args]` | Check room, start the agent inside a job, report peak memory when it exits. Exit code is the agent's. |
| `--budget SIZE` | Budget for the whole agent tree, e.g. `3G`, `1500M`. |
| `--hard` | Enforce the budget. Allocations past it fail inside the agent, which usually crashes it. The machine stays up. |
| `--reserve SIZE` | Memory kept free for Windows. Default: 10% of RAM, at least 2 GB. |
| `--wait` | If the agent does not fit, wait until it does. |
| `--force` | Start even if it does not fit. |
| `--keep-children` | Do not end processes the agent leaves running. |
| `--no-bar` | No memory bar; the agent gets the console directly. Automatic when output is redirected. |
| `loadline top [--watch] [--json]` | Committed memory and working set per agent tree, budgets and system headroom. `--json` for scripts and dashboards. |

Exit code 75 means admission refused to start the agent.

## How it works

- The agent starts **suspended**, joins a job, then resumes, so no child process escapes before the job exists.
- The soft budget is a job-wide [notification limit](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_notification_limit_information). Windows tells loadline when the tree's committed memory crosses it. The hard budget is a job-wide [memory limit](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information).
- "Safe for new agents" is the tighter of free RAM and commit headroom, minus the reserve. Free RAM guards against paging; commit headroom guards against allocation failures.
- The bar works like tmux's status line: the agent runs in a Windows pseudoconsole one row shorter than your window, loadline redraws its screen ([avt](https://crates.io/crates/avt) emulates it) and draws the bar on the last row. Lines that scroll off the agent's screen go into your terminal's normal scrollback, so scrolling up still works. Keys, Ctrl+C, paste, mouse modes and the tab title pass straight through.
- Ctrl+C goes to the agent, not loadline. loadline stays alive so the agent's job stays intact.
- Live runs are listed in `%LOCALAPPDATA%\loadline\runs\`. Events (start, over budget, hard limit, refused, exit) go to `%LOCALAPPDATA%\loadline\events.jsonl`. Set `LOADLINE_HOME` to use another folder.

## Development

```powershell
cargo test                                         # unit + end-to-end tests (no network, no accounts)
cargo test --test bar -- --ignored --nocapture     # also renders real Claude Code and Codex, if installed
```

The end-to-end tests allocate real memory under real job limits and drive loadline inside a pseudoconsole, reading back its screen.

## Limits

- Windows only.
- Going over a soft budget shows in the bar (red), `loadline top`, the event log and the exit summary. There are no desktop notifications yet.
- The bar redraws the agent's screen, so rare escape sequences the emulator does not know may render differently than in a bare terminal. Use `--no-bar` if something looks wrong, and please file an issue.
- A process that explicitly breaks away from jobs (`CREATE_BREAKAWAY_FROM_JOB`) leaves loadline's accounting.
- `top` finds unguarded agents by executable name (`claude`, `codex`, ...). Agents that run as plain `node` only show up when started with `loadline run`.

## History

loadline grew out of [Optimus](https://github.com/sz-747/Optimus), a native Windows terminal multiplexer with a RAM-based cap on terminal count. By late 2026 Orca, herdr and others had covered multiplexing on Windows, but none tracked or limited committed memory per agent on Windows. loadline keeps the part that still matters and works inside whichever terminal you already use.

## License

MIT
