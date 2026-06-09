use anyhow::{Context, Result};
use log::{debug, info};
use std::path::PathBuf;

pub use installer::{InstalledPackage, InstalledPackageList};
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
    pub fn new(name: String, version: String, recipe_url: String, recipe: recipe::Recipe) -> Self {
        Self {
            name,
            version,
            recipe_url,
            recipe,
        }
    }

    pub fn resolve(
        name: &str,
        recipe_url: &str,
        http_client: &reqwest::blocking::Client,
        recipe_executor: &RecipeExecutor,
    ) -> Result<Self> {
        info!("resolving package {name} at {recipe_url}");

        let source_recipe = fetch_recipe(recipe_url, http_client)
            .with_context(|| format!("fetching recipe of {name}"))?;
        let recipe = recipe::Recipe::from_content(recipe_url, source_recipe)
            .with_context(|| format!("parsing recipe of {name}"))?;
        let version = recipe_executor.eval_latest_version(&recipe)?;

        debug!("version: {}", version);

        Ok(Self::new(
            name.to_owned(),
            version,
            recipe_url.to_owned(),
            recipe,
        ))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

pub fn fetch_recipe(recipe_url: &str, http_client: &reqwest::blocking::Client) -> Result<String> {
    let source = http_client
        .get(recipe_url)
        .send()
        .with_context(|| format!("sending recipe request to {recipe_url}"))?
        .error_for_status()
        .with_context(|| format!("fetching recipe at {recipe_url}"))?
        .text()
        .with_context(|| format!("reading recipe response body from {recipe_url}"))?;

    debug!("fetched recipe at {recipe_url}:\n{source}");

    Ok(source)
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
