use anyhow::Result;
use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::{CompleteEnv, CompletionCandidate};
use log::LevelFilter;
use std::{env, path::PathBuf, time::Duration};
use ureq::Agent;

use crate::{
    backend::{self, Scope},
    frontend::{package::PackagesCommand, source::SourcesCommand},
};

mod package;
mod source;

/// ppmx — personal package manager
#[derive(Parser, Debug)]
#[command(name = "ppmx", version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    #[command(flatten)]
    verbosity: Verbosity,

    #[command(flatten)]
    scope: ScopeGroup,

    #[command(subcommand)]
    command: Command,
}

/// Mutually-exclusive verbosity group: -q | -v | -vv
#[derive(Args, Debug)]
#[group(multiple = false)]
struct Verbosity {
    /// Suppress all output
    #[arg(short = 'q', long = "quiet", global = true)]
    quiet: bool,

    /// Enable verbose output (-v) or very verbose output (-vv)
    #[arg(
        short = 'v',
        long = "verbose",
        action = clap::ArgAction::Count,
        global = true
    )]
    verbose: u8,
}

/// Mutually-exclusive scope group
#[derive(Args, Debug)]
#[group(multiple = false)]
struct ScopeGroup {
    /// Operate at user level
    #[arg(short = 'u', long = "user", global = true)]
    user: bool,

    /// Operate at system level
    #[arg(short = 's', long = "system", global = true)]
    system: bool,

    /// Operate in a custom direcotry
    #[arg(long = "scope", global = true)]
    scope: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Manage packages
    #[command(flatten)]
    Packages(PackagesCommand),

    /// Manage package sources
    #[command(subcommand)]
    Sources(SourcesCommand),
}

pub fn run() -> Result<()> {
    CompleteEnv::with_factory(Cli::command).complete();

    let cli = Cli::parse();

    let level = if cli.verbosity.quiet {
        LevelFilter::Warn
    } else {
        match cli.verbosity.verbose {
            0 => LevelFilter::Info,
            1 => LevelFilter::Debug,
            _ => LevelFilter::Trace,
        }
    };
    colog::basic_builder().filter_module("ppmx", level).init();

    let scope = if cli.scope.system {
        Scope::System
    } else if let Some(s) = cli.scope.scope {
        Scope::Custom(if s.is_absolute() {
            s
        } else {
            env::current_dir()?.join(s)
        })
    } else {
        Scope::User
    };

    match cli.command {
        Command::Packages(sub) => sub.run(scope),
        Command::Sources(sub) => sub.run(scope),
    }
}

fn new_http_agent() -> Agent {
    static APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

    Agent::config_builder()
        .user_agent(APP_USER_AGENT)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
}

fn source_completion() -> Vec<CompletionCandidate> {
    let mut completions = Vec::new();

    if let Ok(mut sources) = backend::source::list(&Scope::User) {
        sources.sort();
        for source in sources.into_iter() {
            completions.push(CompletionCandidate::new(source).help(Some("(user)".into())));
        }
    }
    if let Ok(mut sources) = backend::source::list(&Scope::System) {
        sources.sort();
        for source in sources.into_iter() {
            completions.push(CompletionCandidate::new(source).help(Some("(system)".into())));
        }
    }

    completions
}
