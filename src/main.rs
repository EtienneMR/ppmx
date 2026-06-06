use std::path::PathBuf;

use crate::executor::ParsedRecipe;
use crate::executor::RecipeExecutor;

mod executor;
mod shared;

fn main() -> anyhow::Result<()> {
    let executor = RecipeExecutor::new();
    let recipe = ParsedRecipe::from_filename("recipes/tbx.ppmx")?;
    let version = executor.run_latest_version(recipe.clone())?;
    println!("version: {version}");
    let build = executor.run_build(recipe.clone(), version, PathBuf::from(".tmp/tbx"))?;
    println!("build: {build:?}");
    let install = executor.run_install(recipe)?;
    println!("install: {install:?}");
    Ok(())
}
