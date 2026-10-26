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

## DS-004 Branch, name and `main`

- **Question:** does the memory guard replace `main` of sz-747/Optimus, and does it keep the name "Optimus"?
- **Status:** Deferred. **Owner:** maintainer.
- **Current behavior:** lives on the `memory-guard` branch; `main` untouched; local commits only.
- **Reopen trigger:** v0.1 used on the 16 GB laptop for a few days, before any public push or launch.

## DS-005 How the user learns about a soft-budget crossing mid-session

- **Question:** desktop toast, terminal line, or only `top`?
- **Why it matters:** writing into the agent's terminal corrupts its TUI; a toast needs a Windows app identity.
- **Status:** Deferred.
- **Current behavior:** `optimus top`, event log, exit summary.
- **Reopen trigger:** next slice; implement a toast once DS-004 settles the app identity.
