use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use log::debug;
use std::ffi::OsString;

use crate::{
    backend::{Scope, source},
    frontend::new_http_agent,
};

#[derive(Subcommand, Debug)]
pub enum SourcesCommand {
    /// Register a new source
    Add(SourceAddArgs),

    /// Remove a registered source
    Remove(SourceRemoveArgs),

    /// List all registered sources
    List,

    /// Find which source provides the recipe of a package
    Find(SourceFindArgs),
}

#[derive(Args, Debug)]
pub struct SourceAddArgs {
    /// Logical name for the source
    name: OsString,

    url: String,
}

#[derive(Args, Debug)]
pub struct SourceRemoveArgs {
    /// Name of the source to remove
    name: OsString,
}

#[derive(Args, Debug)]
pub struct SourceFindArgs {
    // Name of the package to find
    package_name: String,
}

impl SourcesCommand {
    pub fn run(self, scope: Scope) -> Result<()> {
        match self {
            SourcesCommand::Add(args) => {
                let Some((prefix, suffix)) = args.url.split_once("{package}") else {
                    bail!("invalid source url: missing {{package}} template");
                };

                source::add(&args.name, (prefix, suffix), &scope)?;
            }
            SourcesCommand::Remove(args) => {
                source::remove(&args.name, &scope)?;
            }
            SourcesCommand::List => {
                debug!("configured sources");
                for source in source::list(&scope)?.iter() {
                    let url = source::get_url(source, &scope)?;
                    println!(
                        "{:<10} {}{{package}}{}",
                        source.to_string_lossy(),
                        url.0,
                        url.1
                    )
                }
            }
            SourcesCommand::Find(args) => {
                let http_agent = new_http_agent();

                let (recipe, url, source) =
                    source::get_recipe(&args.package_name, &http_agent, &scope)?;
                println!("source  {}", source.display());
                println!("url     {url}");
                println!("recipe\n{recipe}");
            }
        }

        Ok(())
    }
}
