use serde::Deserialize;
use starlark::{environment::GlobalsBuilder, eval::Evaluator, starlark_module};

use crate::executor::RecipeExecutor;

#[starlark_module]
pub fn github_module(builder: &mut GlobalsBuilder) {
    fn latest_release(repo: String, eval: &mut Evaluator) -> anyhow::Result<String> {
        let response = RecipeExecutor::from_evaluator(eval)
            .http_client
            .get(format!(
                "https://api.github.com/repos/{repo}/releases/latest"
            ))
            .send()?
            .error_for_status()?;

        #[derive(Deserialize)]
        struct _Release {
            tag_name: String,
        }

        let release: _Release = response.json()?;

        return Ok(release.tag_name);
    }

    fn asset_url(repo: &str, tag: &str, asset_name: &str) -> anyhow::Result<String> {
        Ok(format!(
            "https://github.com/{repo}/releases/download/{tag}/{asset_name}"
        ))
    }
}
