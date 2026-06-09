use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use log::info;
use std::ffi::OsString;

use crate::{
    backend::{Scope, ServerList},
    frontend::new_http_client,
};

#[derive(Subcommand, Debug)]
pub enum ServersCommand {
    /// Register a new server
    Add(ServerAddArgs),

    /// Remove a registered server
    Remove(ServerRemoveArgs),

    /// List all registered servers
    List,

    /// Find which server provides the recipe of a package
    Find(ServerFindArgs),
}

#[derive(Args, Debug)]
pub struct ServerAddArgs {
    /// Logical name for the server
    name: OsString,

    url: String,
}

#[derive(Args, Debug)]
pub struct ServerRemoveArgs {
    /// Name of the server to remove
    name: OsString,
}

#[derive(Args, Debug)]
pub struct ServerFindArgs {
    // Name of the package to find
    package_name: String,
}

impl ServersCommand {
    pub fn run(self, scope: Scope) -> Result<()> {
        match self {
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
            ServersCommand::Find(args) => {
                let server_list = ServerList::new(scope);
                let http_client = new_http_client();

                let (url, server) = server_list.find_url(&args.package_name, &http_client)?;
                println!("server  {}", server.display());
                println!("url     {url}");
            }
        }

        Ok(())
    }
}
