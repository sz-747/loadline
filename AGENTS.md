# loadline

## What this is

A Rust CLI that guards AI coding agents' memory on Windows: per-agent budgets via Job Objects, admission control on free RAM and commit headroom, kill-on-close for leftovers, a live memory bar under the agent, and `loadline top`. Target machine: a 16 GB Windows laptop.

It grew out of the Optimus multiplexer (github.com/sz-747/Optimus). Do not port that project's UI here.

## Layout

- `src/admission.rs`, `src/tree.rs`, `src/size.rs`: pure policy, unit-tested.
- `src/sys.rs`: Win32 memory and process facts. `src/job.rs`: Job Object wrapper.
- `src/run.rs`, `src/top.rs`: the two commands. `src/state.rs`: run records and event log.
- Status bar: `src/pty.rs` (ConPTY), `src/console.rs` (raw console), `src/session.rs` (loop), `src/screen.rs` (frame drawing), `src/scan.rs` (sequences forwarded to the real terminal), `src/statusbar.rs` (the bar line).
- `tests/run.rs`: end-to-end tests that allocate real memory under real job limits.
- `tests/bar.rs`: runs loadline inside a pseudoconsole and reads its screen; typing, Ctrl+C, scrolling. `--ignored` tests run real Claude Code and Codex (no model calls).
- `docs/decision-surfaces.md`: decided and open product choices.

## Rules

- Default budgets are soft. Never make a hard kill the default.
- Measure committed (private) memory, not working set, for budgets and admission.
- Keep Windows-specific code in `sys.rs` and `job.rs`; keep policy pure and testable.
- Before committing: `cargo fmt`, `cargo clippy --all-targets` (no warnings), `cargo test`.
- The repository is public: no machine paths, personal names or private notes in files or commit messages.
- Pushing needs the maintainer's explicit approval.
