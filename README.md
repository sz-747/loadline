# Optimus

**A memory guard for AI coding agents on Windows.**

Run three Claude Code or Codex sessions on a 16 GB laptop and Windows starts paging, freezing or killing things. Task Manager shows each agent using a few hundred MB, so it is hard to see why. The number that runs Windows out of memory is *committed* memory, and agents commit far more than their working set suggests.

Optimus wraps each agent in a Windows Job Object:

- **Budget per agent.** `--budget 3G` covers the agent and every process it starts (Node, ripgrep, MCP servers). Soft by default: you are told, nothing is killed. `--hard` makes the budget a wall.
- **Admission control.** Optimus refuses to start an agent the machine cannot fit, or waits for room with `--wait`.
- **No orphans.** When the agent exits, anything it left running is ended.
- **Real numbers.** `optimus top` shows committed memory per agent tree, plus how much is still safe to use.

It works with any terminal or multiplexer: Windows Terminal, VS Code, Orca, herdr, plain PowerShell.

## Quick start

```powershell
cargo install --git https://github.com/sz-747/Optimus --branch memory-guard

optimus run --budget 3G -- claude
optimus run --budget 2G --hard -- codex
optimus top --watch
```

```
RAM 16.0 GB | free 3.1 GB | commit 20.4 GB / 24.0 GB (headroom 3.6 GB) | safe for new agents 1.1 GB (keeping 2.0 GB for Windows)

AGENT              PID  PROCS  COMMITTED  WORKING SET     BUDGET  STATUS
claude           50824     17     3.4 GB       1.1 GB     3.0 GB  OVER 113% (soft)
codex            31428     18     1.2 GB     760 MB     2.0 GB  60% (hard)
claude           26828     13     749 MB     531 MB          -  unguarded
```

(Illustrative 16 GB laptop. Run `optimus top` to see your own numbers.)

## Commands

| Command | What it does |
|---|---|
| `optimus run [options] -- <agent> [args]` | Check room, start the agent inside a job, report peak memory when it exits. Exit code is the agent's. |
| `--budget SIZE` | Budget for the whole agent tree, e.g. `3G`, `1500M`. |
| `--hard` | Enforce the budget. Allocations past it fail inside the agent, which usually crashes it. The machine stays up. |
| `--reserve SIZE` | Memory kept free for Windows. Default: 10% of RAM, at least 2 GB. |
| `--wait` | If the agent does not fit, wait until it does. |
| `--force` | Start even if it does not fit. |
| `--keep-children` | Do not end processes the agent leaves running. |
| `optimus top [--watch] [--json]` | Committed memory and working set per agent tree, budgets and system headroom. `--json` for scripts and dashboards. |

Exit code 75 means admission refused to start the agent.

## How it works

- The agent starts **suspended**, joins a job, then resumes, so no child process escapes before the job exists.
- The soft budget is a job-wide [notification limit](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_notification_limit_information). Windows tells Optimus when the tree's committed memory crosses it. The hard budget is a job-wide [memory limit](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information).
- "Safe for new agents" is the tighter of free RAM and commit headroom, minus the reserve. Free RAM guards against paging; commit headroom guards against allocation failures.
- Ctrl+C goes to the agent, not Optimus. Optimus stays alive so the agent's job stays intact.
- Live runs are listed in `%LOCALAPPDATA%\optimus\runs\`. Events (start, over budget, hard limit, refused, exit) go to `%LOCALAPPDATA%\optimus\events.jsonl`.

## Limits

- Windows only.
- Soft budget warnings appear in `optimus top`, the event log and the exit summary. Desktop notifications are not built yet.
- A process that explicitly breaks away from jobs (`CREATE_BREAKAWAY_FROM_JOB`) leaves Optimus's accounting.
- `top` finds unguarded agents by executable name (`claude`, `codex`, ...). Agents that run as plain `node` only show up when started with `optimus run`.

## History

Optimus started as a native Windows terminal multiplexer with a RAM-based cap on terminal count (see the `main` branch). By late 2026 Orca, herdr and others had covered multiplexing on Windows, but none tracked or limited committed memory per agent on Windows. This branch keeps the part that still matters.

## License

MIT
