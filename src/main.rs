//! Optimus: a memory guard for AI coding agents on Windows.

mod admission;
mod job;
mod resolve;
mod run;
mod size;
mod state;
mod sys;
mod top;
mod tree;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "optimus",
    version,
    about = "Memory guard for AI coding agents on Windows",
    long_about = "Run each agent inside a memory budget, refuse to start one the machine \
                  cannot fit, see what every agent really commits, and leave no orphaned \
                  processes behind."
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run an agent inside a memory budget: optimus run --budget 3G -- claude
    Run {
        /// Memory budget for the agent and everything it starts (e.g. 3G, 1500M).
        /// Soft by default: you are told, nothing is killed.
        #[arg(long, value_parser = size::parse_size)]
        budget: Option<u64>,
        /// Enforce the budget: allocations past it fail inside the agent.
        #[arg(long, requires = "budget")]
        hard: bool,
        /// Memory to keep free for Windows (default: 10% of RAM, at least 2G).
        #[arg(long, value_parser = size::parse_size)]
        reserve: Option<u64>,
        /// If the agent does not fit now, wait until it does instead of refusing.
        #[arg(long, conflicts_with = "force")]
        wait: bool,
        /// Start even if the agent does not fit.
        #[arg(long)]
        force: bool,
        /// Leave processes the agent started running after it exits.
        #[arg(long)]
        keep_children: bool,
        /// The agent command and its arguments.
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Show what each agent commits and how much room is left.
    Top {
        /// Refresh every second.
        #[arg(long)]
        watch: bool,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
        /// Memory to keep free for Windows when computing the safe zone.
        #[arg(long, value_parser = size::parse_size)]
        reserve: Option<u64>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Cmd::Run {
            budget,
            hard,
            reserve,
            wait,
            force,
            keep_children,
            command,
        } => run::run(run::RunArgs {
            budget,
            hard,
            reserve,
            wait,
            force,
            keep_children,
            command,
        }),
        Cmd::Top {
            watch,
            json,
            reserve,
        } => top::top(watch, json, reserve).map(|()| ExitCode::SUCCESS),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("optimus: {e}");
            ExitCode::FAILURE
        }
    }
}
