use anyhow::{Context, Result};
use log::trace;
use std::{fmt::Display, ops::Deref, path::PathBuf};
use ureq::Agent;

use crate::backend::recipes::PackageVersion;

pub mod database;
pub mod planner;
pub mod recipes;
pub mod source;

// PUBLIC API

#[derive(Debug, Clone)]
pub enum Scope {
    System,
    User,
    Custom(PathBuf),
}

pub struct ResolvedPackage {
    name: String,
    version: PackageVersion,

    recipe_url: String,
    recipe: recipes::Recipe,
}

impl ResolvedPackage {
    pub fn new(
        name: String,
        version: PackageVersion,
        recipe_url: String,
        recipe: recipes::Recipe,
    ) -> Self {
        Self {
            name,
            version,
            recipe_url,
            recipe,
        }
    }

    pub fn resolve(name: String, http_agent: Agent, scope: &Scope) -> Result<Self> {
        trace!("resolving package {name:?}");

        let (recipe_source, recipe_url, _) = source::get_recipe(&name, &http_agent, scope)
            .with_context(|| format!("failed to resolve recipe url of {name}"))?;

        Self::parse(name, recipe_source, recipe_url, http_agent)
    }

    pub fn fetch(name: String, recipe_url: String, http_agent: Agent) -> Result<Self> {
        trace!("fetching package {name} at {recipe_url}");

        let recipe_content = fetch_recipe(&recipe_url, &http_agent)
            .with_context(|| format!("failed to fetch recipe of {name} at {recipe_url}"))?;

        Self::parse(name, recipe_content, recipe_url, http_agent)
    }

    pub fn parse(
        name: String,
        recipe_content: String,
        recipe_url: String,
        http_agent: Agent,
    ) -> Result<Self> {
        let recipe = recipes::Recipe::parse(recipe_content)
            .with_context(|| format!("failed to parse recipe of {name}"))?;
        let version = recipe
            .eval_latest_version(http_agent)
            .with_context(|| format!("failed to resolve version of {name}"))?;

        trace!("package {name} version {}", version.name);

        Ok(Self::new(name, version, recipe_url, recipe))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version.name
    }
}

impl Display for ResolvedPackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} v{}", self.name(), self.version())
    }
}

pub struct ResolvedPackageWithDependencies {
    package: ResolvedPackage,
    dependencies: Vec<String>,
}

impl TryFrom<ResolvedPackage> for ResolvedPackageWithDependencies {
    type Error = anyhow::Error;
    fn try_from(value: ResolvedPackage) -> Result<Self> {
        Ok(Self {
            dependencies: value
                .recipe
                .eval_dependencies()
                .with_context(|| format!("failed to resolve dependencies of {}", value.name()))?
                .packages,
            package: value,
        })
    }
}

impl Deref for ResolvedPackageWithDependencies {
    type Target = ResolvedPackage;

    fn deref(&self) -> &Self::Target {
        &self.package
    }
}

impl Display for ResolvedPackageWithDependencies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.package.fmt(f)
    }
}

pub fn fetch_recipe(recipe_url: &str, http_agent: &Agent) -> Result<String> {
    let source = http_agent
        .get(recipe_url)
        .call()
        .with_context(|| format!("failed to fetch recipe at {recipe_url}"))?
        .body_mut()
        .read_to_string()
        .with_context(|| format!("failed to read recipe response body from {recipe_url}"))?;

    trace!("fetched recipe at {recipe_url}:\n{source}");

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

    fn app_data_dir(&self) -> Result<PathBuf> {
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
