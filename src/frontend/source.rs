use anyhow::Result;
use clap::{Args, Subcommand};
use log::debug;
use std::ffi::OsString;

use crate::{
    backend::{
        Scope,
        source::{self, Source},
    },
    frontend::{new_http_agent, source_completion},
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
    #[arg(add = clap_complete::engine::ArgValueCandidates::new(source_completion))]
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
                let source = Source::new(args.url);
                source.validate()?;
                source::set(&args.name, source, &scope)?;
            }
            SourcesCommand::Remove(args) => {
                source::remove(&args.name, &scope)?;
            }
            SourcesCommand::List => {
                debug!("configured sources");
                let list = source::list(&scope)?;
                let width = list.iter().map(|p| p.len()).max().unwrap_or(0);
                for source_name in list.iter() {
                    let source = source::get(source_name, &scope)?;
                    println!("{:<width$}  {source}", source_name.display());
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
