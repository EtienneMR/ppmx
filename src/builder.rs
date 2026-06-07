use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;

use crate::database::Database;
use crate::executor::{ParsedRecipe, RecipeExecutor};
use crate::shared::{InstalledPackage, PackageAsset, PackageAssetKind, Scope};

pub fn install_package(
    executor: &RecipeExecutor,
    recipe: &ParsedRecipe,
    version: String,
    scope: &Scope,
    database: &Database,
) -> anyhow::Result<()> {
    let packages_path = database.packages_dir();
    let pkg_dir = packages_path.join(&recipe.name);
    let backup_dir = packages_path.join(format!("{}.bkp", recipe.name));
    let temp_dir = packages_path.join(format!("{}.tmp", recipe.name));

    let (build_dir, assets) =
        build_package(executor, recipe, version.clone(), &temp_dir).context("building package")?;

    fs::create_dir_all(&packages_path).context("creating packages directory")?;
    stage_package(&pkg_dir, &backup_dir, &build_dir).context("staging package")?;

    let assets = resolve_asset_paths(assets, &pkg_dir, scope).context("resolving asset paths")?;
    let old_package = update_state(&database, recipe, version, &assets)?;
    let old_exports = old_package.map(|p| p.exports).unwrap_or(Vec::new());
    uninstall_exports(&old_exports).context("uninstalling old exports")?;
    install_assets(&assets).context("installing new exports")?;

    fs::remove_dir_all(backup_dir).ok();
    fs::remove_dir_all(temp_dir).ok();
    Ok(())
}

fn build_package(
    executor: &RecipeExecutor,
    recipe: &ParsedRecipe,
    version: String,
    temp_dir: &Path,
) -> anyhow::Result<(PathBuf, Vec<PackageAsset>)> {
    fs::remove_dir_all(temp_dir).ok();
    fs::create_dir_all(temp_dir).context("creating build directory")?;
    let result_dir = executor.run_build(recipe, version, temp_dir.to_owned())?;
    let build_dir = temp_dir
        .join(result_dir)
        .canonicalize()
        .context("resolving build directory")?;
    let assets = executor.run_install(recipe)?;

    Ok((build_dir, assets))
}

fn resolve_asset_paths(
    mut assets: Vec<PackageAsset>,
    packages_path: &Path,
    scope: &Scope,
) -> anyhow::Result<Vec<PackageAsset>> {
    for asset in &mut assets {
        asset.source_path = packages_path.join(&asset.source_path);
        let base = match asset.kind {
            PackageAssetKind::Executable => scope.bin_path()?,
            PackageAssetKind::Share => scope.share_path()?,
            PackageAssetKind::Config => scope.config_path()?,
        };
        asset.system_path = base.join(&asset.system_path);
    }
    Ok(assets)
}

fn stage_package(pkg_dir: &Path, backup_dir: &Path, build_dir: &Path) -> anyhow::Result<()> {
    fs::remove_dir_all(&backup_dir).ok();

    match fs::rename(&pkg_dir, &backup_dir) {
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        r => r.context("backing up old version")?,
    };

    fs::rename(build_dir, &pkg_dir).context("moving package to destination")?;
    Ok(())
}

fn update_state(
    db: &Database,
    recipe: &ParsedRecipe,
    version: String,
    assets: &[PackageAsset],
) -> anyhow::Result<Option<InstalledPackage>> {
    let mut state = db.load_state()?;
    let prev = state.packages.insert(
        recipe.name.clone(),
        InstalledPackage {
            version,
            recipe_url: recipe.url.clone(),
            exports: assets.iter().map(|a| a.system_path.clone()).collect(),
        },
    );
    db.save_state(&state)?;
    Ok(prev)
}

fn install_assets(assets: &[PackageAsset]) -> anyhow::Result<()> {
    for asset in assets {
        if let Some(parent) = asset.system_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating parent dir for {asset:?}"))?;
        }
        match asset.kind {
            PackageAssetKind::Config => {
                fs::copy(&asset.source_path, &asset.system_path)
                    .with_context(|| format!("copying config asset {asset:?}"))?;
            }
            _ => std::os::unix::fs::symlink(&asset.source_path, &asset.system_path)?,
        }
    }
    Ok(())
}

fn uninstall_exports(exports: &[PathBuf]) -> anyhow::Result<()> {
    for export in exports.iter() {
        match fs::remove_file(export) {
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            r => r.with_context(|| format!("removing export {export:?}"))?,
        }
    }
    Ok(())
}
