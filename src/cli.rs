use std::{ffi::OsString, path::PathBuf};

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;
use log::{LevelFilter, info};

use crate::backend::{InstalledPackageList, RecipeExecutor, ResolvedPackage, Scope, ServerList};

/// ppmx — personal package manager
#[derive(Parser, Debug)]
#[command(name = "ppmx", version, about, long_about = None)]
#[command(propagate_version = true)]
pub struct Cli {
    #[command(flatten)]
    pub verbosity: Verbosity,

    #[command(flatten)]
    pub scope: ScopeGroup,

    #[command(subcommand)]
    pub command: Command,
}

/// Mutually-exclusive verbosity group: -q | -v | -vv
#[derive(Args, Debug)]
#[group(multiple = false)]
pub struct Verbosity {
    /// Suppress all output
    #[arg(short = 'q', long = "quiet", global = true)]
    pub quiet: bool,

    /// Enable verbose output (-v) or very verbose output (-vv)
    #[arg(
        short = 'v',
        long = "verbose",
        action = clap::ArgAction::Count,
        global = true
    )]
    pub verbose: u8,
}

/// Mutually-exclusive scope group
#[derive(Args, Debug)]
#[group(multiple = false)]
pub struct ScopeGroup {
    /// Operate at user level
    #[arg(short = 'u', long = "user", global = true)]
    pub user: bool,

    /// Operate at system level
    #[arg(short = 's', long = "system", global = true)]
    pub system: bool,

    /// Operate in a custom direcotry
    #[arg(long = "scope", global = true)]
    pub scope: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Install one or more packages
    Install(InstallArgs),

    /// Uninstall one or more packages
    Uninstall(UninstallArgs),

    /// List installed packages
    List,

    /// Manage package servers
    #[command(subcommand)]
    Servers(ServersCommand),

    /// Print a shell completion script to stdout and exit
    Completions {
        /// Target shell
        shell: Shell,
    },
}

#[derive(Args, Debug)]
pub struct InstallArgs {
    /// One or more package names to install
    #[arg(required = true, num_args = 1..)]
    pub packages: Vec<String>,
}

#[derive(Args, Debug)]
pub struct UninstallArgs {
    /// One or more package names to uninstall
    #[arg(required = true, num_args = 1..)]
    pub packages: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum ServersCommand {
    /// Register a new server
    Add(ServerAddArgs),

    /// Remove a registered server
    Remove(ServerRemoveArgs),

    /// List all registered servers
    List,
}

#[derive(Args, Debug)]
pub struct ServerAddArgs {
    /// Logical name for the server
    pub name: OsString,

    pub url: String,
}

#[derive(Args, Debug)]
pub struct ServerRemoveArgs {
    /// Name of the server to remove
    pub name: OsString,
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

    match &cli.command {
        Command::Install(args) => {
            let http_client = new_http_client();
            let package_list = InstalledPackageList::new(scope.clone());
            let server_list = ServerList::new(scope);
            let executor = &RecipeExecutor::new(http_client.clone());

            for package_name in args.packages.iter() {
                info!("installing package {package_name}");
                let recipe_url = server_list.find_url(package_name, &http_client)?;
                let package = ResolvedPackage::resolve(package_name, &recipe_url, &http_client)?;
                package_list.install(&package, executor)?;
            }
        }
        Command::Uninstall(args) => {
            let package_list = InstalledPackageList::new(scope);
            for package_name in args.packages.iter() {
                package_list.uninstall(&package_name)?;
            }
        }
        Command::List => {
            let package_list = InstalledPackageList::new(scope);
            info!("installed packages");

            let list = package_list.list()?;
            let width = list.iter().map(|p| p.name().len()).max().unwrap_or(0);
            for package in list.iter() {
                println!("  {:width$} {}", package.name(), package.version());
            }
        }
        Command::Servers(sub) => match sub {
            ServersCommand::Add(args) => {
                let server_list = ServerList::new(scope);
                let Some((prefix, suffix)) = args.url.split_once("{package}") else {
                    bail!("invalid server url: missing {{package}} template");
                };

                server_list.add(&args.name, (prefix, suffix))?;
            }
            ServersCommand::Remove(args) => {
                let server_list = ServerList::new(scope);
                server_list.remove(&args.name)?;
            }
            ServersCommand::List => {
                let server_list = ServerList::new(scope);
                info!("configured servers");
                for server in server_list.list()?.iter() {
                    let url = server_list.get_url(server)?;
                    println!(
                        "{:<10} {}{{package}}{}",
                        server.to_string_lossy(),
                        url.0,
                        url.1
                    )
                }
            }
        },
        Command::Completions { .. } => unreachable!(),
    };

    Ok(())
}

fn new_http_client() -> reqwest::blocking::Client {
    static APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

    reqwest::blocking::Client::builder()
        .user_agent(APP_USER_AGENT)
        .build()
        .expect("builder should be valid")
}
