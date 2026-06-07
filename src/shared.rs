use std::{env, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, allocative::Allocative)]
pub struct PackageAsset {
    pub kind: PackageAssetKind,
    pub source_path: std::path::PathBuf,
    pub system_path: std::path::PathBuf,
}

#[derive(Debug, allocative::Allocative)]
pub enum PackageAssetKind {
    Executable,
    Share,
    Config,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InstalledPackage {
    pub version: String,
    pub recipe_url: String,
    pub exports: Vec<PathBuf>,
}

pub enum Scope {
    System,
    User,
    Custom(PathBuf),
}

impl Scope {
    pub fn bin_path(&self) -> Result<PathBuf> {
        match self {
            Self::System => Ok(PathBuf::from("/usr/local/bin")),
            Self::User => xdg_dir("XDG_BIN_HOME", ".local/bin"),
            Self::Custom(base) => Ok(base.join("bin")),
        }
    }

    pub fn share_path(&self) -> Result<PathBuf> {
        match self {
            Self::System => Ok(PathBuf::from("/usr/local/share")),
            Self::User => xdg_dir("XDG_DATA_HOME", ".local/share"),
            Self::Custom(base) => Ok(base.join("share")),
        }
    }

    pub fn config_path(&self) -> Result<PathBuf> {
        match self {
            Self::System => Ok(PathBuf::from("/etc")),
            Self::User => xdg_dir("XDG_CONFIG_HOME", ".config"),
            Self::Custom(base) => Ok(base.join("config")),
        }
    }
}

fn xdg_dir(name: &str, default: &str) -> Result<PathBuf> {
    if let Some(value) = env::var_os(name) {
        return Ok(PathBuf::from(value));
    }

    let home = env::home_dir().context("failed to find home directory")?;
    Ok(home.join(default))
}
