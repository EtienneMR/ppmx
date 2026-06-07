use std::env;

use crate::builder::install_package;
use crate::database::Database;
use crate::executor::{ParsedRecipe, RecipeExecutor};
use crate::shared::Scope;

mod builder;
mod database;
mod executor;
mod shared;

fn main() -> anyhow::Result<()> {
    let scope = Scope::Custom(env::current_dir()?.join(".tmp"));
    let database = Database::from_scope(&scope)?;

    let executor = RecipeExecutor::new();
    let recipe =
        ParsedRecipe::from_filename("vscodium-web".to_string(), "recipes/vscodium-web.ppmx")?;
    let version = executor.run_latest_version(&recipe)?;

    install_package(&executor, &recipe, version, &scope, &database)
}
