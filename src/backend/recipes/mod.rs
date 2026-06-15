use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use log::trace;
use rhai::{AST, Engine, EvalAltResult, Scope};
use ureq::Agent;

use runtime::Runtime;

mod runtime;

// PUBLIC API

pub struct Recipe {
    ast: AST,
}

impl Recipe {
    pub fn parse(content: String) -> Result<Self> {
        let ast = Engine::new()
            .compile(content)
            .map_err(|e| anyhow!(e))
            .context("failed to parse recipe")?;
        Ok(Self { ast })
    }

    pub fn eval_dependencies(&self) -> Result<Dependencies> {
        trace!("evaluating dependencies");

        Runtime::new()
            .into_engine()
            .call_fn::<Dependencies>(&mut Scope::new(), &self.ast, "dependencies", ())
            .or_else(|e| match e.unwrap_inner() {
                &EvalAltResult::ErrorFunctionNotFound(..) => Ok(Dependencies::default().into()),
                _ => Err(e),
            })
            .map_err(|e: Box<EvalAltResult>| anyhow!("{e}"))
            .context("failed to evaluate dependencies")
    }

    pub fn eval_latest_version(&self, http_agent: Agent) -> Result<PackageVersion> {
        trace!("evaluating latest_version");

        Runtime::new()
            .with_http_agent(http_agent)
            .allow_non_idempotent()
            .into_engine()
            .call_fn::<PackageVersion>(&mut Scope::new(), &self.ast, "latest_version", ())
            .map_err(|e| anyhow!("{e}"))
            .context("failed to evaluate latest_version")
    }

    pub fn run_build(
        &self,
        version: PackageVersion,
        working_directory: PathBuf,
        http_agent: Agent,
    ) -> Result<BuildResult> {
        trace!("running build in {working_directory:?}");

        let result = Runtime::new()
            .with_http_agent(http_agent)
            .with_working_directory(working_directory)
            .into_engine()
            .call_fn::<BuildResult>(&mut Scope::new(), &self.ast, "build", (version,))
            .map_err(|e| anyhow!("{e}"))
            .context("failed to evaluate build")?;

        Ok(result)
    }
}

#[derive(Debug, Clone)]
pub struct PackageVersion {
    pub name: String,
    pub metadata: rhai::Dynamic,
}

#[derive(Debug, Clone)]
pub struct BuildResult {
    pub export_root: PathBuf,
    pub exports: Vec<BuildExport>,
}

#[derive(Debug, Clone)]
pub struct BuildExport {
    pub kind: BuildExportKind,
    pub source_path: PathBuf,
    pub system_path: PathBuf,
}

#[derive(Debug, Clone)]
pub enum BuildExportKind {
    Executable,
    Share,
    Config,
}

#[derive(Debug, Clone, Default)]
pub struct Dependencies {
    pub packages: Vec<String>,
}
