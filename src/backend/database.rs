use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read, Seek, SeekFrom, Write},
    ops::Deref,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use log::{debug, error, info, trace, warn};
use serde::{Deserialize, Serialize};
use ureq::Agent;

use crate::backend::{
    ResolvedPackage, ResolvedPackageWithDependencies, Scope,
    recipes::{BuildExportKind, BuildResult},
};

// PUBLIC

pub struct InstallRequest {
    pub package: ResolvedPackageWithDependencies,
    pub install_explicitly: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InstalledPackageData {
    version: String,
    dependencies: Vec<String>,

    recipe_url: String,
    owned_files: Vec<PathBuf>,
    explicitly_installed: bool,
}

impl InstalledPackageData {
    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn dependencies(&self) -> &[String] {
        &self.dependencies
    }

    pub fn recipe_url(&self) -> &str {
        &self.recipe_url
    }

    pub fn owned_files(&self) -> &[PathBuf] {
        &self.owned_files
    }

    pub fn explicitly_installed(&self) -> bool {
        self.explicitly_installed
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Database {
    #[serde(default)]
    packages: BTreeMap<String, InstalledPackageData>,

    #[serde(default)]
    owned_directories: BTreeSet<String>,
}

impl Database {
    pub fn load(scope: &Scope) -> Result<Self> {
        let database_file = scope.app_data_dir()?.join("database.json");

        trace!("loading database from {database_file:?}");
        let content = match fs::read_to_string(&database_file) {
            Ok(s) => s,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                trace!("database file not found, starting with empty database");
                "{}".to_string()
            }
            Err(e) => {
                return Err(e).context(format!("failed to read database file {database_file:?}"));
            }
        };
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse database file {database_file:?}"))
    }
}

impl Deref for Database {
    type Target = BTreeMap<String, InstalledPackageData>;

    fn deref(&self) -> &Self::Target {
        &self.packages
    }
}

pub struct LockedDatabase {
    database: Database,
    file: File,
    dirty: bool,
    dry_mode: bool,
}

impl LockedDatabase {
    pub fn load(scope: &Scope) -> Result<Self> {
        let database_dir = scope.app_data_dir()?;
        let database_file = database_dir.join("database.json");

        trace!("loading database from {:?}", database_file);

        fs::create_dir_all(&database_dir)
            .with_context(|| format!("failed to create database directory {database_dir:?}"))?;

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&database_file)
            .with_context(|| format!("failed to open database file {database_file:?}"))?;

        file.lock()
            .with_context(|| format!("failed to lock database file {database_file:?}"))?;

        let mut content = String::new();
        file.read_to_string(&mut content)
            .with_context(|| format!("failed to read database file {database_file:?}"))?;

        if content.is_empty() {
            trace!("database file is empty, starting with empty database");
            content = "{}".to_string();
        }

        let database: Database = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse database file {database_file:?}"))?;

        Ok(Self {
            database,
            file,
            dirty: false,
            dry_mode: false,
        })
    }

    pub fn save(&mut self) -> Result<()> {
        if self.dry_mode {
            debug!("database not saved: running in dry mode");
            return Ok(());
        }

        let content =
            serde_json::to_string_pretty(&self.database).context("failed to serialize database")?;

        self.file
            .seek(SeekFrom::Start(0))
            .context("failed to seek to start of database file")?;

        self.file
            .write_all(content.as_bytes())
            .context("failed to write database")?;

        // FIXME: a crash here could corrupt database and a read could see temporary corrupted state (non atomic)
        // writing to a temp file would prevent corruption but that would take longer
        // fixe once save is not called after each operation

        self.file
            .set_len(content.len() as u64)
            .context("failed to truncate database")?;

        self.file.flush().context("failed to flush database")?;

        self.dirty = false;

        debug!("saved database");

        Ok(())
    }

    pub fn install(
        &mut self,
        request: InstallRequest,
        http_agent: Agent,
        scope: &Scope,
    ) -> Result<()> {
        debug!("installing package {}", &request.package);

        for dep_name in &request.package.dependencies {
            if !self.database.contains_key(dep_name) {
                bail!(
                    "cannot install {}: dependency {dep_name:?} is not installed",
                    &request.package.name,
                );
            }
        }

        let package_dir = package_path(&request.package.name, scope)?;
        let (mut build, build_directory) = build_package(&request.package, http_agent, scope)?;

        for export in build.exports.iter_mut() {
            export.source_path = package_dir.join(&export.source_path);

            let base = match export.kind {
                BuildExportKind::Executable => scope.bin_path()?,
                BuildExportKind::Share => scope.share_path()?,
                BuildExportKind::Config => scope.config_path()?,
            };
            export.system_path = base.join(&export.system_path);
        }
        build.export_root = build_directory.join(build.export_root);
        build.export_root = build.export_root.canonicalize().with_context(|| {
            format!(
                "failed to canonicalize export root at {:?}",
                &build.export_root
            )
        })?;

        let ResolvedPackageWithDependencies {
            package,
            dependencies,
        } = request.package;

        let new_install = InstalledPackageData {
            version: package.version.name,
            recipe_url: package.recipe_url,
            owned_files: build
                .exports
                .iter()
                .map(|e| e.system_path.clone())
                .collect(),
            dependencies,
            explicitly_installed: request.install_explicitly,
        };

        match self.database.packages.entry(package.name) {
            Entry::Occupied(mut e) => {
                debug!("removing existing installation of {}", e.key());

                if self.dry_mode {
                    info!("would remove package {}", e.key());
                    info!("would install {} at {package_dir:?}", e.key());
                } else {
                    // FIXME: a crash here could corrupt database (entry in database without files in system)
                    // one package directory per version would be required to have a crash-safe update flow
                    // removing a package can be supposed safe as long as it is an internal tool
                    remove_package(
                        e.key(),
                        e.get(),
                        scope,
                        &mut self.database.owned_directories,
                    )?;

                    add_package(&package_dir, &build, &mut self.database.owned_directories)?;
                }
                *e.get_mut() = new_install;
            }
            Entry::Vacant(e) => {
                if self.dry_mode {
                    info!("would install {} at {package_dir:?}", e.key());
                } else {
                    add_package(&package_dir, &build, &mut self.database.owned_directories)?;
                }
                e.insert(new_install);
            }
        };

        self.dirty = true;

        if let Err(e) = fs::remove_dir_all(&build_directory)
            && e.kind() != ErrorKind::NotFound
        {
            warn!(
                "{:?}",
                anyhow!(e).context(format!(
                    "failed to clean temp build dir {build_directory:?}"
                ))
            );
        }

        Ok(())
    }

    pub fn uninstall(&mut self, name: &str, scope: &Scope) -> Result<()> {
        debug!("uninstalling package {name}");

        let Some(install) = self.database.packages.remove(name) else {
            bail!("cannot uninstall {name}: package not installed")
        };

        for (dep_name, dep_data) in self.database.iter() {
            if dep_data.dependencies().iter().any(|d| d == name) {
                bail!("cannot uninstall {name}: package is a dependency of {dep_name}");
            }
        }

        self.save()?;

        if !self.dry_mode {
            remove_package(name, &install, scope, &mut self.database.owned_directories)?;

            // remove_package may have deleted directories that were tracked
            // in owned_directories; persist that update too
            self.dirty = true;
            self.save()?;
        } else {
            info!("would remove package {name}")
        }

        Ok(())
    }

    pub fn set_install_reason(
        &mut self,
        package_name: &str,
        explicitly_installed: bool,
    ) -> Result<()> {
        let Some(package) = self.database.packages.get_mut(package_name) else {
            bail!("package not installed {package_name}");
        };
        let different = package.explicitly_installed ^ explicitly_installed;
        if different {
            package.explicitly_installed = explicitly_installed;
            self.dirty |= true;
            if self.dry_mode {
                info!(
                    "would change install reason of {package_name} to {}explicitly installed",
                    if explicitly_installed { "" } else { "not " }
                )
            } else {
                debug!(
                    "changed install reason of {package_name} to {}explicitly installed",
                    if explicitly_installed { "" } else { "not " }
                )
            }
        }
        Ok(())
    }

    pub fn remove_leftovers(&self, scope: &Scope) -> Result<()> {
        let packages_dir = scope.app_data_dir()?.join("packages");

        let entries = match fs::read_dir(&packages_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                trace!("packages directory {packages_dir:?} does not exist, nothing to clean up");
                return Ok(());
            }
            Err(e) => {
                return Err(e).context(format!(
                    "failed to list package directories in {packages_dir:?}"
                ));
            }
        };

        for package_dir in entries {
            let package = package_dir?;
            let package_name = package.file_name().to_string_lossy().to_string();
            if self.get(&package_name).is_none() {
                if self.dry_mode {
                    info!("would remove unused package directory {package_name}");
                } else {
                    info!("removing unused package directory {package_name}");
                    fs::remove_dir_all(package.path()).with_context(|| {
                        format!("failed to remove unused package directory {package_name}")
                    })?;
                }
            }
        }

        Ok(())
    }

    pub fn enable_dry_mode(&mut self) -> Result<()> {
        if self.dirty {
            self.save()
                .context("failed to save dirty database file before enabling dry mode")?;
        }
        self.dry_mode = true;
        Ok(())
    }
}

impl Drop for LockedDatabase {
    fn drop(&mut self) {
        if self.dirty {
            if let Err(e) = self
                .save()
                .context("failed to save dirty database file before drop")
            {
                error!("{}", e)
            }
        }

        if let Err(e) = self
            .file
            .unlock()
            .context("failed to unlock database file before drop")
        {
            error!("{}", e)
        }
    }
}

impl Deref for LockedDatabase {
    type Target = Database;

    fn deref(&self) -> &Self::Target {
        &self.database
    }
}

fn build_package(
    package: &ResolvedPackage,
    http_agent: Agent,
    scope: &Scope,
) -> Result<(BuildResult, PathBuf)> {
    let temp_path = package_path(&format!("{}.tmp", package.name), scope)?;

    debug!("building package {package}");
    trace!("building in {temp_path:?}");

    if let Err(e) = fs::remove_dir_all(&temp_path)
        && e.kind() != ErrorKind::NotFound
    {
        return Err(e).context(format!("failed to remove old temp build dir {temp_path:?}"));
    }
    fs::create_dir_all(&temp_path)
        .with_context(|| format!("failed to create temp build dir {temp_path:?}"))?;

    let result = package
        .recipe
        .run_build(package.version.clone(), temp_path.clone(), http_agent)
        .with_context(|| format!("failed to build package {package}"))?;

    Ok((result, temp_path))
}

fn add_package(
    package_dir: &PathBuf,
    build: &BuildResult,
    owned_directories: &mut BTreeSet<String>,
) -> Result<()> {
    debug!("placing package files into {:?}", package_dir);

    if let Some(parent) = package_dir.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory for {package_dir:?}"))?;
    }

    match fs::remove_dir_all(package_dir) {
        Ok(_) => {
            warn!("removed dangling package directory at {package_dir:?}");
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => {
            warn!(
                "{:?}",
                anyhow!(e).context(format!(
                    "failed to remove dangling package directory at {package_dir:?}"
                ))
            );
        }
    };

    fs::rename(&build.export_root, package_dir).with_context(|| {
        format!(
            "failed to move build output from {:?} to {package_dir:?}",
            build.export_root
        )
    })?;
    for export in build.exports.iter() {
        trace!(
            "exporting {:?} ({:?}) -> {:?}",
            export.source_path, export.kind, export.system_path
        );
        if let Some(parent) = export.system_path.parent() {
            create_dir_all_tracked(parent, owned_directories)
                .with_context(|| format!("failed to create parent dir for {export:?}"))?;
        }
        std::os::unix::fs::symlink(&export.source_path, &export.system_path).with_context(
            || {
                format!(
                    "failed to symlink {:?} -> {:?}",
                    export.source_path, export.system_path
                )
            },
        )?;
    }

    Ok(())
}

fn remove_package(
    name: &str,
    package: &InstalledPackageData,
    scope: &Scope,
    owned_directories: &mut BTreeSet<String>,
) -> Result<()> {
    let package_dir = package_path(name, scope)?;

    for owned_file in package.owned_files.iter() {
        trace!("removing owned file {:?}", owned_file);
        match fs::remove_file(owned_file) {
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {
                trace!("owned file already absent: {:?}", owned_file);
            }
            Err(e) => return Err(e).context(format!("failed to remove {owned_file:?}")),
        }

        if let Some(parent) = owned_file.parent() {
            cleanup_empty_dirs(parent, owned_directories);
        }
    }
    trace!("removing package directory {:?}", package_dir);
    fs::remove_dir_all(&package_dir)
        .with_context(|| format!("failed to remove package directory {package_dir:?}"))?;

    if let Some(parent) = package_dir.parent() {
        cleanup_empty_dirs(parent, owned_directories);
    }

    Ok(())
}

fn package_path(name: &str, scope: &Scope) -> Result<PathBuf> {
    Ok(scope.app_data_dir()?.join("packages").join(name))
}

fn create_dir_all_tracked(
    path: &Path,
    owned_directories: &mut BTreeSet<String>,
) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        create_dir_all_tracked(parent, owned_directories)?;
    }

    fs::create_dir(path)?;

    trace!("tracking owned directory {path:?}");
    owned_directories.insert(path.to_string_lossy().into_owned());

    Ok(())
}

fn cleanup_empty_dirs(dir: &Path, owned_directories: &mut BTreeSet<String>) {
    let key = dir.to_string_lossy().into_owned();

    if !owned_directories.contains(&key) {
        return;
    }

    match fs::remove_dir(dir) {
        Err(e) if e.kind() == ErrorKind::DirectoryNotEmpty => {}
        Err(e) if e.kind() != ErrorKind::NotFound => {
            warn!(
                "{:?}",
                anyhow!(e).context(format!("failed to remove empty owned directory {dir:?}"))
            );
        }
        _ => {
            debug!("removed empty owned directory {dir:?}");
            owned_directories.remove(&key);
            if let Some(parent) = dir.parent() {
                return cleanup_empty_dirs(parent, owned_directories);
            }
        }
    }
}
