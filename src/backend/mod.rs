use anyhow::{Context, Result};
use log::{debug, info};
use std::path::PathBuf;

pub use installer::InstalledPackageList;
pub use recipe::RecipeExecutor;
pub use server::ServerList;

mod installer;
mod recipe;
mod server;

// PUBLIC API

#[derive(Debug, Clone)]
pub enum Scope {
    System,
    User,
    Custom(PathBuf),
}

pub struct ResolvedPackage {
    name: String,
    version: String,

    recipe_url: String,
    recipe: recipe::Recipe,
}

impl ResolvedPackage {
    pub fn resolve(
        name: &str,
        recipe_url: &str,
        http_client: &reqwest::blocking::Client,
    ) -> Result<Self> {
        info!("resolving package {name} at {recipe_url}");

        let response = http_client
            .get(recipe_url)
            .send()
            .with_context(|| format!("sending recipe request to {recipe_url}"))?
            .error_for_status()
            .with_context(|| format!("fetching recipe from {recipe_url}"))?
            .text()
            .context("reading recipe response body")?;

        debug!("recipe:\n{}", &response);

        let recipe = recipe::Recipe::from_content(recipe_url, response)?;
        let version =
            recipe::RecipeExecutor::new(http_client.clone()).eval_latest_version(&recipe)?;

        debug!("version: {}", version);

        Ok(Self {
            name: name.to_owned(),
            recipe_url: recipe_url.to_owned(),
            recipe,
            version,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

// PRIVATE

impl Scope {
    fn bin_path(&self) -> Result<PathBuf> {
        match self {
            Self::System => Ok(PathBuf::from("/usr/local/bin")),
            Self::User => xdg_dir("XDG_BIN_HOME", ".local/bin"),
            Self::Custom(base) => Ok(base.join("bin")),
        }
    }

    fn share_path(&self) -> Result<PathBuf> {
        match self {
            Self::System => Ok(PathBuf::from("/usr/local/share")),
            Self::User => xdg_dir("XDG_DATA_HOME", ".local/share"),
            Self::Custom(base) => Ok(base.join("share")),
        }
    }

    fn config_path(&self) -> Result<PathBuf> {
        match self {
            Self::System => Ok(PathBuf::from("/etc")),
            Self::User => xdg_dir("XDG_CONFIG_HOME", ".config"),
            Self::Custom(base) => Ok(base.join("config")),
        }
    }

    fn app_data_dir(&self) -> anyhow::Result<PathBuf> {
        Ok(self.share_path()?.join("ppmx"))
    }
}

fn xdg_dir(name: &str, default: &str) -> Result<PathBuf> {
    if let Some(value) = std::env::var_os(name) {
        return Ok(PathBuf::from(value));
    }

    let home = std::env::home_dir().context("failed to find home directory")?;
    Ok(home.join(default))
}
