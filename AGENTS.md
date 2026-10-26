# Optimus (memory-guard branch)

## What this is

A Rust CLI that guards AI coding agents' memory on Windows: per-agent budgets via Job Objects, admission control on free RAM and commit headroom, kill-on-close for leftovers, and `optimus top`. Target machine: a 16 GB Windows laptop.

The old WinUI/Tauri multiplexer lives on `main` and the `backup/*` branches. Do not port its UI here.

## Layout

- `src/admission.rs`, `src/tree.rs`, `src/size.rs`: pure policy, unit-tested.
- `src/sys.rs`: Win32 memory and process facts. `src/job.rs`: Job Object wrapper.
- `src/run.rs`, `src/top.rs`: the two commands. `src/state.rs`: run records and event log.
- `tests/run.rs`: end-to-end tests that allocate real memory under real job limits.
- `docs/decision-surfaces.md`: decided and open product choices.

## Rules

- Default budgets are soft. Never make a hard kill the default.
- Measure committed (private) memory, not working set, for budgets and admission.
- Keep Windows-specific code in `sys.rs` and `job.rs`; keep policy pure and testable.
- Before committing: `cargo fmt`, `cargo clippy --all-targets` (no warnings), `cargo test`.
- Pushing this branch or replacing `main` needs the maintainer's explicit approval each time.
