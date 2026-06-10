use std::{collections::BTreeMap, fs, io::ErrorKind, path::PathBuf};

use anyhow::{Context, Result, bail};
use log::{debug, info};
use serde::{Deserialize, Serialize};
use ureq::Agent;

use crate::backend::{
    ResolvedPackage, Scope,
    recipe::{BuildExportKind, BuildResult},
    run_build,
};

// PUBLIC

#[derive(Debug)]
pub struct InstalledPackage {
    name: String,
    data: InstalledPackageData,
}

impl InstalledPackage {
    fn from_tuple((name, data): (String, InstalledPackageData)) -> Self {
        Self { name, data }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.data.version
    }

    pub fn recipe_url(&self) -> &str {
        &self.data.recipe_url
    }

    pub fn owned_files(&self) -> &[PathBuf] {
        &self.data.owned_files
    }
}

pub struct InstalledPackageList {
    scope: Scope,
}

impl InstalledPackageList {
    pub fn new(scope: Scope) -> Self {
        Self { scope }
    }

    pub fn list(&self) -> Result<Vec<InstalledPackage>> {
        debug!("listing installed packages");
        let state = self.load_state()?;

        let packages = state
            .packages
            .into_iter()
            .map(InstalledPackage::from_tuple)
            .collect();

        Ok(packages)
    }

    pub fn get(&self, name: &str) -> Result<InstalledPackage> {
        debug!("looking up installed package {name}");
        let mut state = self.load_state()?;

        if let Some(package) = state.packages.remove_entry(name) {
            Ok(InstalledPackage::from_tuple(package))
        } else {
            bail!("package {name} not installed")
        }
    }

    pub fn install(&self, package: &ResolvedPackage, http_client: Agent) -> Result<()> {
        info!(
            "installing package {} version {}",
            package.name, package.version
        );
        let package_dir = self.package_path(&package.name)?;
        let mut build = self.build_package(package, http_client)?;

        for export in build.exports.iter_mut() {
            export.source_path = package_dir.join(&export.source_path);

            let base = match export.kind {
                BuildExportKind::Executable => self.scope.bin_path()?,
                BuildExportKind::Share => self.scope.share_path()?,
                BuildExportKind::Config => self.scope.config_path()?,
            };
            export.system_path = base.join(&export.system_path);
        }
        build.export_root = build.build_directory.join(build.export_root);
        build.export_root = build
            .export_root
            .canonicalize()
            .with_context(|| format!("canonicalize export root at {:?}", &build.export_root))?;

        let new_install = InstalledPackageData {
            version: package.version.clone(),
            recipe_url: package.recipe_url.clone(),
            owned_files: build
                .exports
                .iter()
                .map(|e| e.system_path.clone())
                .collect(),
        };

        let mut state = self.load_state()?;

        if let Some(old_install) = state.packages.insert(package.name.clone(), new_install) {
            info!(
                "removing existing installation of {} {}",
                package.name(),
                package.version()
            );
            self.remove_package(&package.name, &old_install)?;
        };

        self.add_package(&package_dir, &build)?;

        self.save_state(&state)?;

        info!("package {} installed successfully", package.name);
        Ok(())
    }

    pub fn uninstall(&self, name: &str) -> Result<()> {
        info!("uninstalling package {name}");
        let mut state = self.load_state()?;

        let Some(install) = state.packages.remove(name) else {
            bail!("package not installed {name}")
        };

        self.remove_package(name, &install)?;
        self.save_state(&state)?;

        info!("package {name} uninstalled successfully");
        Ok(())
    }

    fn build_package(&self, package: &ResolvedPackage, http_client: Agent) -> Result<BuildResult> {
        let temp_path = self.package_path(&format!("{}.tmp", package.name))?;

        info!("building package {} {}", package.name, package.version);
        debug!("building in {temp_path:?}");

        if let Err(e) = fs::remove_dir_all(&temp_path)
            && e.kind() != ErrorKind::NotFound
        {
            return Err(e).context(format!("cleaning temp build dir {temp_path:?}"));
        }
        fs::create_dir_all(&temp_path)
            .with_context(|| format!("creating temp build dir {temp_path:?}"))?;

        run_build(
            &package.recipe,
            package.version.clone(),
            temp_path,
            http_client,
        )
        .with_context(|| format!("building package {} {}", &package.name, &package.version))
    }

    fn add_package(&self, package_dir: &PathBuf, build: &BuildResult) -> Result<()> {
        info!("placing package files into {:?}", package_dir);

        if let Some(parent) = package_dir.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::rename(&build.export_root, package_dir).with_context(|| {
            format!(
                "moving build output from {:?} to {package_dir:?}",
                build.export_root
            )
        })?;
        for export in build.exports.iter() {
            debug!(
                "exporting {:?} ({:?}) -> {:?}",
                export.source_path, export.kind, export.system_path
            );
            if let Some(parent) = export.system_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating parent dir for {export:?}"))?;
            }
            std::os::unix::fs::symlink(&export.source_path, &export.system_path).with_context(
                || {
                    format!(
                        "symlinking {:?} -> {:?}",
                        export.source_path, export.system_path
                    )
                },
            )?;
        }

        Ok(())
    }

    fn remove_package(&self, name: &str, package: &InstalledPackageData) -> Result<()> {
        let package_dir = self.package_path(name)?;

        debug!("removing package directory {:?}", package_dir);
        fs::remove_dir_all(&package_dir)
            .with_context(|| format!("removing package directory {package_dir:?}"))?;
        for owned_file in package.owned_files.iter() {
            debug!("removing owned file {:?}", owned_file);
            match fs::remove_file(owned_file) {
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    debug!("owned file already absent: {:?}", owned_file);
                }
                Err(e) => return Err(e).context(format!("removing {owned_file:?}")),
            }
        }
        Ok(())
    }

    fn load_state(&self) -> Result<State> {
        let state_file = self.scope.app_data_dir()?.join("state.json");

        debug!("loading state from {:?}", state_file);
        let content = match fs::read_to_string(&state_file) {
            Ok(s) => s,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                debug!("state file not found, starting with empty state");
                "{}".to_string()
            }
            Err(e) => return Err(e).context(format!("reading state file {state_file:?}")),
        };
        serde_json::from_str(&content).with_context(|| format!("parsing state file {state_file:?}"))
    }

    fn save_state(&self, state: &State) -> Result<()> {
        let state_dir = self.scope.app_data_dir()?;
        let state_file = state_dir.join("state.json");

        debug!("saving state to {:?}", state_file);
        fs::create_dir_all(&state_dir)
            .with_context(|| format!("creating state directory {state_dir:?}"))?;
        let content = serde_json::to_string_pretty(state)?;
        fs::write(&state_file, content)
            .with_context(|| format!("writing state file {state_file:?}"))
    }

    fn package_path(&self, name: &str) -> Result<PathBuf> {
        Ok(self.scope.app_data_dir()?.join("packages").join(name))
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    packages: BTreeMap<String, InstalledPackageData>,
}

#[derive(Debug, Serialize, Deserialize)]
struct InstalledPackageData {
    version: String,

    recipe_url: String,
    owned_files: Vec<PathBuf>,
}
