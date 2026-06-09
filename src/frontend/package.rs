use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use log::{error, info};

use crate::{
    backend::{
        InstalledPackage, InstalledPackageList, RecipeExecutor, ResolvedPackage, Scope, ServerList,
    },
    frontend::new_http_client,
};

#[derive(Subcommand, Debug)]
pub enum PackagesCommand {
    /// Install one or more packages
    Install(PackageInstallArgs),

    /// Uninstall one or more packages
    Uninstall(PackageUninstallArgs),

    /// List installed packages
    List,

    /// Show information about a package name
    Info(PackageInfoArgs),

    /// Update all installed packages to their latest version
    Update(PackageUpdateArgs),
}

#[derive(Args, Debug)]
pub struct PackageInstallArgs {
    /// One or more package names to install
    #[arg(required = true, num_args = 1..)]
    packages: Vec<String>,
}

#[derive(Args, Debug)]
pub struct PackageUninstallArgs {
    /// One or more package names to uninstall
    #[arg(required = true, num_args = 1..)]
    packages: Vec<String>,
}

#[derive(Args, Debug)]
pub struct PackageInfoArgs {
    /// Package name to query
    package: String,
}

#[derive(Args, Debug)]
#[group(multiple = false)]
pub struct PackageUpdateArgs {
    /// One or more package names to update
    #[arg(required = true, num_args = 1..)]
    packages: Vec<String>,

    #[arg(short = 'a', long = "all")]
    all: bool,
}

impl PackagesCommand {
    pub fn run(self, scope: Scope) -> Result<()> {
        match self {
            PackagesCommand::Install(args) => {
                let http_client = new_http_client();
                let package_list = InstalledPackageList::new(scope.clone());
                let server_list = ServerList::new(scope);
                let executor = RecipeExecutor::new(http_client.clone());

                for package_name in args.packages.iter() {
                    install(
                        package_name,
                        &http_client,
                        &package_list,
                        &server_list,
                        &executor,
                    )?;
                }
            }
            PackagesCommand::Uninstall(args) => {
                let package_list = InstalledPackageList::new(scope);
                for package_name in args.packages.iter() {
                    package_list.uninstall(&package_name)?;
                }
            }
            PackagesCommand::List => {
                let package_list = InstalledPackageList::new(scope);
                info!("installed packages");

                let list = package_list.list()?;
                let width = list.iter().map(|p| p.name().len()).max().unwrap_or(0);
                for package in list.iter() {
                    println!("  {:width$} {}", package.name(), package.version());
                }
            }
            PackagesCommand::Info(args) => {
                let package_list = InstalledPackageList::new(scope);
                let package = package_list.get(&args.package)?;
                println!("name        {}", package.name());
                println!("version     {}", package.version());
                println!("recipe url  {}", package.recipe_url());
                println!("exports");
                for file in package.owned_files().iter() {
                    println!("  {}", file.display());
                }
            }
            PackagesCommand::Update(args) => {
                let http_client = new_http_client();
                let package_list = InstalledPackageList::new(scope);
                let executor = RecipeExecutor::new(http_client.clone());

                let mut packages = package_list.list()?;

                if !args.all {
                    packages.retain(|i| args.packages.iter().any(|p| p == i.name()));

                    if packages.is_empty() {
                        bail!("no package found, requested {}", args.packages.join(", "));
                    }
                }

                let mut has_errors = false;
                for install in packages.into_iter() {
                    if let Err(e) = update(&install, &http_client, &package_list, &executor) {
                        has_errors = true;
                        error!("{:?}", e);
                    }
                }

                if has_errors {
                    bail!("at least one package could not be updated");
                }
            }
        }

        Ok(())
    }
}

fn install(
    package_name: &str,
    http_client: &reqwest::blocking::Client,
    package_list: &InstalledPackageList,
    server_list: &ServerList,
    executor: &RecipeExecutor,
) -> Result<()> {
    info!("installing package {package_name}");
    let (recipe_url, _server_name) = server_list.find_url(package_name, &http_client)?;

    let package = ResolvedPackage::resolve(package_name, &recipe_url, &http_client, &executor)?;
    package_list.install(&package, &executor)?;

    Ok(())
}

fn update(
    install: &InstalledPackage,
    http_client: &reqwest::blocking::Client,
    package_list: &InstalledPackageList,
    executor: &RecipeExecutor,
) -> Result<()> {
    let package = ResolvedPackage::resolve(
        install.name(),
        install.recipe_url(),
        &http_client,
        &executor,
    )
    .with_context(|| {
        format!(
            "resolving package {} at {}",
            install.name(),
            install.recipe_url()
        )
    })?;

    if package.version() != install.version() {
        info!(
            "updating {} {} to {}",
            install.name(),
            install.version(),
            package.version()
        );
        package_list.install(&package, &executor).with_context(|| {
            format!(
                "installing package {} {}",
                package.name(),
                package.version()
            )
        })?;
    } else {
        info!(
            "package {} {} is up-to-date",
            install.name(),
            install.version()
        );
    }

    Ok(())
}
