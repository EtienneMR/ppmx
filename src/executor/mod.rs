use std::path::PathBuf;

use anyhow::Context;
use starlark::{
    environment::{Globals, GlobalsBuilder, LibraryExtension, Module},
    eval::Evaluator,
    syntax::{AstModule, Dialect},
    values::ValueLike,
};
use starlark_derive::ProvidesStaticType;

use crate::{
    executor::{
        build_context_api::BuildContext, github_api::github_module,
        install_context_api::InstallContext,
    },
    shared::PackageAsset,
};

mod build_context_api;
mod github_api;
mod install_context_api;

#[derive(ProvidesStaticType)]
pub struct RecipeExecutor {
    globals: Globals,
    http_client: reqwest::blocking::Client,
}

impl RecipeExecutor {
    pub fn new() -> Self {
        static APP_USER_AGENT: &str =
            concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

        Self {
            globals: GlobalsBuilder::extended_by(&[LibraryExtension::Print])
                .with_namespace("github", github_module)
                .build(),
            http_client: reqwest::blocking::Client::builder()
                .user_agent(APP_USER_AGENT)
                .build()
                .expect("builder should be valid"),
        }
    }

    pub fn from_evaluator<'a>(eval: &'a Evaluator) -> &'a Self {
        eval.extra
            .expect("extra defined in eval_recipe")
            .downcast_ref::<RecipeExecutor>()
            .expect("extra defined in eval_recipe")
    }

    pub fn run_latest_version(&self, recipe: ParsedRecipe) -> anyhow::Result<String> {
        self.eval_recipe(recipe, |mut eval| {
            let function = eval
                .module()
                .get("latest_version")
                .context(format!("function latest_version not defined"))?;

            let res = eval
                .eval_function(function, &[], &[])
                .map_err(|e| e.into_anyhow())?;

            res.unpack_str()
                .context("latest_version result is not a string")
                .map(|s| s.to_string())
        })
    }

    pub fn run_build(
        &self,
        recipe: ParsedRecipe,
        version: String,
        working_directory: PathBuf,
    ) -> anyhow::Result<PathBuf> {
        self.eval_recipe(recipe, |mut eval| {
            let function = eval
                .module()
                .get("build")
                .context(format!("function build not defined"))?;

            let ctx = eval
                .heap()
                .alloc_complex_no_freeze(BuildContext::new(version, working_directory));

            let res = eval
                .eval_function(function, &[ctx], &[])
                .map_err(|e| e.into_anyhow())?;

            res.unpack_str()
                .context("build result is not a string")
                .map(|s| s.into())
        })
    }

    pub fn run_install(&self, recipe: ParsedRecipe) -> anyhow::Result<Vec<PackageAsset>> {
        self.eval_recipe(recipe, |mut eval| {
            let function = eval
                .module()
                .get("install")
                .context(format!("function install not defined"))?;

            let ctx = eval.heap().alloc_complex_no_freeze(InstallContext::new());

            eval.eval_function(function, &[ctx], &[])
                .map_err(|e| e.into_anyhow())?;

            Ok(ctx.downcast_ref::<InstallContext>().unwrap().assets.take())
        })
    }

    fn eval_recipe<R>(
        &self,
        recipe: ParsedRecipe,
        f: impl FnOnce(Evaluator) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        Module::with_temp_heap(|module| {
            let mut eval = Evaluator::new(&module);
            eval.extra = Some(self);
            eval.eval_module(recipe.ast, &self.globals)
                .map_err(|e| e.into_anyhow())?;

            f(eval)
        })
    }
}

#[derive(Debug, Clone)]
pub struct ParsedRecipe {
    ast: AstModule,
}

impl ParsedRecipe {
    pub fn from_content(filename: &str, content: String) -> anyhow::Result<Self> {
        let ast = AstModule::parse(
            filename,
            content,
            &Dialect {
                enable_f_strings: true,
                ..Dialect::Standard
            },
        )
        .map_err(|e| e.into_anyhow())
        .context("failled to parse content")?;

        Ok(Self { ast })
    }

    pub fn from_filename(filename: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(filename).context("failled to read file")?;
        Self::from_content(filename, content)
    }
}
