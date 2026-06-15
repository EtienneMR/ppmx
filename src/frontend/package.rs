use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use log::{info, warn};
use ureq::Agent;

use crate::{
    backend::{
        Scope,
        database::{Database, InstallRequest, InstalledPackageData, LockedDatabase},
        planner::{InstallPlan, plan_uninstall},
    },
    frontend::new_http_agent,
};

#[derive(Subcommand, Debug)]
pub enum PackagesCommand {
    /// Install one or more packages
    Install(PackageInstallArgs),

    /// Uninstall one or more packages and remove unneeded ones
    Uninstall(PackageUninstallArgs),

    /// List installed packages
    List,

    /// Show information about a package name
    Info(PackageInfoArgs),

    /// Update all installed packages to their latest version
    Update(PackageUpdateArgs),

    /// Clean database
    Clean,
}

#[derive(Args, Debug)]
pub struct PackageInstallArgs {
    /// One or more package names to install and mark as explicitely installed
    #[arg(required = true, num_args = 1..)]
    packages: Vec<String>,

    /// Install even if already installed
    #[arg(long = "reinstall")]
    reinstall: bool,

    /// Compute install plan without running it
    #[arg(long = "dry-run")]
    dry_run: bool,
}

#[derive(Args, Debug)]
pub struct PackageUninstallArgs {
    /// One or more package names to mark as not explicitely installed
    packages: Vec<String>,

    /// Compute uninstall plan without running it
    #[arg(long = "dry-run")]
    dry_run: bool,
}

#[derive(Args, Debug)]
pub struct PackageInfoArgs {
    /// Package name to query
    package: String,
}

#[derive(Args, Debug)]
pub struct PackageUpdateArgs {
    /// Package to update
    #[command(flatten)]
    packages: PackageSelection,

    /// Compute install plan without running it
    #[arg(long = "dry-run")]
    dry_run: bool,
}

#[derive(Args, Debug)]
#[group(multiple = false)]
pub struct PackageSelection {
    /// One or more package names
    #[arg(required = true, num_args = 1..)]
    packages: Vec<String>,

    /// Select all packages
    #[arg(short = 'a', long = "all")]
    all: bool,
}

impl PackageSelection {
    fn get_packages<'a>(
        &'a self,
        database: &'a Database,
    ) -> Result<Vec<(&'a String, &'a InstalledPackageData)>> {
        if self.all {
            Ok(database.iter().collect())
        } else {
            self.packages
                .iter()
                .map(|p| {
                    Ok((
                        p,
                        database
                            .get(p)
                            .with_context(|| format!("package {p} not installed"))?,
                    ))
                })
                .collect::<Result<Vec<_>>>()
        }
    }
}

impl PackagesCommand {
    pub fn run(self, scope: Scope) -> Result<()> {
        match self {
            PackagesCommand::Install(args) => {
                let http_agent = new_http_agent();

                let mut database = LockedDatabase::load(&scope)?;

                if args.dry_run {
                    database.enable_dry_mode()?;
                }

                let mut plan = InstallPlan::new(&scope, &database, &http_agent);
                let mut to_mark_explicit = Vec::new();

                for package_name in args.packages.into_iter() {
                    if !args.reinstall && database.contains_key(&package_name) {
                        to_mark_explicit.push(package_name);
                    } else {
                        info!("planning installation of {package_name}");
                        plan.add_install(package_name, args.reinstall)?;
                    }
                }

                let plan = plan.to_plan();

                for package_name in to_mark_explicit.into_iter() {
                    info!("marking package {package_name} as explicitly installed");
                    database.set_install_reason(&package_name, true)?;
                }

                apply_plan(plan, &http_agent, &scope, &mut database)?;
            }
            PackagesCommand::Uninstall(args) => {
                let mut database = LockedDatabase::load(&scope)?;
                if args.dry_run {
                    database.enable_dry_mode()?;
                }
                for package_name in args.packages.iter() {
                    database.set_install_reason(package_name, false)?;
                }
                let plan = plan_uninstall(&database)
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>();
                for (idx, package_name) in plan.iter().enumerate() {
                    info!("[{}/{}] uninstalling {package_name}", idx + 1, plan.len());
                    database.uninstall(package_name, &scope)?;
                }
            }
            PackagesCommand::List => {
                info!("installed packages");

                let database = Database::load(&scope)?;
                let width = database.keys().map(|p| p.len()).max().unwrap_or(0);
                for (name, data) in database.iter() {
                    println!(
                        "  {name:width$}  {}  {}",
                        if data.explicitly_installed() {
                            " "
                        } else {
                            "D"
                        },
                        data.version()
                    );
                }
            }
            PackagesCommand::Info(args) => {
                if let Some(data) = Database::load(&scope)?.get(&args.package) {
                    println!("name            {}", args.package);
                    println!("version         {}", data.version());
                    println!(
                        "install reason  {}",
                        if data.explicitly_installed() {
                            "explicit"
                        } else {
                            "dependency"
                        }
                    );
                    println!("recipe url      {}", data.recipe_url());
                    println!("exports");
                    for file in data.owned_files().iter() {
                        println!("  {}", file.display());
                    }
                    println!("dependencies");
                    for file in data.dependencies().iter() {
                        println!("  {}", file);
                    }
                } else {
                    bail!("package {} not installed", args.package);
                }
            }
            PackagesCommand::Update(args) => {
                let http_agent = new_http_agent();

                let mut database = LockedDatabase::load(&scope)?;
                if args.dry_run {
                    database.enable_dry_mode()?;
                }
                let mut plan = InstallPlan::new(&scope, &database, &http_agent);

                let packages = args.packages.get_packages(&database)?;

                for (package_name, package_data) in packages.into_iter() {
                    info!("planning update of {package_name}");
                    plan.add_update(
                        package_name.clone(),
                        package_data.version(),
                        package_data.recipe_url().to_string(),
                        package_data.explicitly_installed(),
                    )?;
                }

                apply_plan(plan.to_plan(), &http_agent, &scope, &mut database)?;
            }
            PackagesCommand::Clean => {
                let mut database = LockedDatabase::load(&scope)?;

                info!("uninstalling unneeded packages");
                let plan = plan_uninstall(&database)
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>();

                for (idx, package_name) in plan.iter().enumerate() {
                    info!("[{}/{}] uninstalling {package_name}", idx + 1, plan.len());
                    database.uninstall(package_name, &scope)?;
                }

                info!("removing leftovers");
                database.remove_leftovers(&scope)?;
            }
        }

        Ok(())
    }
}

fn apply_plan(
    plan: Vec<InstallRequest>,
    http_agent: &Agent,
    scope: &Scope,
    database: &mut LockedDatabase,
) -> Result<()> {
    if plan.is_empty() {
        warn!("all packages are already installed");
    } else {
        let len = plan.len();
        info!("installing {} packages", len);
        for (idx, request) in plan.into_iter().enumerate() {
            info!("[{}/{len}] installing {}", idx + 1, request.package);
            database
                .install(request, http_agent.clone(), &scope)
                .with_context(|| format!("failed to install package"))?;
        }
    }

    Ok(())
}
