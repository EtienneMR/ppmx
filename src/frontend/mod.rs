use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;
use log::LevelFilter;
use std::{path::PathBuf, time::Duration};
use ureq::Agent;

use crate::{
    backend::Scope,
    frontend::{package::PackagesCommand, server::ServersCommand},
};

mod package;
mod server;

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

    /// Manage package servers
    #[command(subcommand)]
    Servers(ServersCommand),

    /// Print a shell completion script to stdout and exit
    Completions {
        /// Target shell
        shell: Shell,
    },
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    if let Command::Completions { shell } = cli.command {
        clap_complete::generate(
            shell,
            &mut <Cli as clap::CommandFactory>::command(),
            "ppmx",
            &mut std::io::stdout(),
        );
        return Ok(());
    }

    let level = if cli.verbosity.quiet {
        LevelFilter::Warn
    } else {
        match cli.verbosity.verbose {
            0 => LevelFilter::Info,
            1 => LevelFilter::Debug,
            _ => LevelFilter::Trace,
        }
    };
    colog::basic_builder().filter_level(level).init();

    let scope = if cli.scope.system {
        Scope::System
    } else if let Some(s) = cli.scope.scope {
        Scope::Custom(s)
    } else {
        Scope::User
    };

    match cli.command {
        Command::Packages(sub) => sub.run(scope),
        Command::Servers(sub) => sub.run(scope),
        Command::Completions { .. } => unreachable!(),
    }
}

fn new_http_client() -> Agent {
    static APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

    Agent::config_builder()
        .user_agent(APP_USER_AGENT)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
}
