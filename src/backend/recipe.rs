use std::{
    cell::RefCell,
    env::consts,
    fs::{self, OpenOptions},
    io::{self, BufWriter},
    path::PathBuf,
    process::Command,
    rc::Rc,
};

use anyhow::{Context, Result};
use log::debug;
use rhai::{AST, Array, CustomType, Engine, EvalAltResult, Module, Scope, TypeBuilder};
use serde::Deserialize;
use ureq::Agent;

// PUBLIC API

pub struct Recipe {
    ast: AST,
}

impl Recipe {
    pub fn parse(content: String) -> anyhow::Result<Self> {
        let ast = Engine::new()
            .compile(content)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("parsing recipe")?;
        Ok(Self { ast })
    }
}

#[derive(Debug, Default)]
pub struct BuildResult {
    pub build_directory: PathBuf,
    pub export_root: PathBuf,
    pub exports: Vec<BuildExport>,
}

#[derive(Debug)]
pub struct BuildExport {
    pub kind: BuildExportKind,
    pub source_path: PathBuf,
    pub system_path: PathBuf,
}

#[derive(Debug)]
pub enum BuildExportKind {
    Executable,
    Share,
    Config,
}

pub fn eval_latest_version(recipe: &Recipe, http_client: Agent) -> Result<String> {
    debug!("evaluating latest_version");
    build_engine(http_client)
        .call_fn::<String>(&mut Scope::new(), &recipe.ast, "latest_version", ())
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("evaluating latest_version")
}

pub fn run_build(
    recipe: &Recipe,
    version: String,
    working_directory: PathBuf,
    http_client: Agent,
) -> Result<BuildResult> {
    debug!("running build in {working_directory:?}");
    let ctx = BuildContext::new(version, working_directory, http_client.clone());
    let result = ctx.result.clone();
    build_engine(http_client)
        .call_fn::<()>(&mut Scope::new(), &recipe.ast, "build", (ctx,))
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("evaluating build")?;
    Ok(result.take())
}

// PRIVATE

fn build_engine(http_client: Agent) -> Engine {
    let mut engine = Engine::new();

    let mut github_module = Module::new();
    github_module.set_native_fn(
        "latest_release",
        move |repo: &str| -> Result<String, Box<EvalAltResult>> {
            debug!("fetching latest release for {repo}");
            #[derive(Deserialize)]
            struct Release {
                tag_name: String,
            }
            let release: Release = http_client
                .get(format!(
                    "https://api.github.com/repos/{repo}/releases/latest"
                ))
                .call()
                .and_then(|mut r| r.body_mut().read_json())
                .map_err(|e| format!("fetching latest release of {repo}: {e}"))?;
            debug!("latest release for {repo} is {}", release.tag_name);
            Ok(release.tag_name)
        },
    );
    github_module.set_native_fn(
        "asset_url",
        |repo: &str, tag: &str, asset_name: &str| -> Result<String, Box<EvalAltResult>> {
            Ok(format!(
                "https://github.com/{repo}/releases/download/{tag}/{asset_name}"
            ))
        },
    );
    engine.register_static_module("github", Rc::new(github_module));

    engine.build_type::<BuildContext>();

    engine
}

#[derive(Clone, CustomType)]
#[rhai_type(name = "BuildContext", extra = Self::build_extra)]
struct BuildContext {
    #[rhai_type(readonly)]
    version: String,

    #[rhai_type(skip)]
    http_client: Agent,

    #[rhai_type(skip)]
    result: Rc<RefCell<BuildResult>>,
}

impl BuildContext {
    fn new(version: String, working_directory: PathBuf, http_client: Agent) -> Self {
        Self {
            version,
            http_client,
            result: Rc::new(RefCell::new(BuildResult {
                build_directory: working_directory,
                ..BuildResult::default()
            })),
        }
    }

    fn download(&mut self, url: &str, destination: &str) -> Result<(), Box<EvalAltResult>> {
        debug!("downloading {url} -> {destination}");
        let mut response = self
            .http_client
            .get(url)
            .call()
            .with_context(|| format!("sending download request for {url}"))
            .map_err(|e| {
                Box::new(EvalAltResult::ErrorSystem(
                    "failed to download".to_string(),
                    e.into(),
                ))
            })?;

        let dest_path = self.result.borrow().build_directory.join(destination);
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&dest_path)
            .with_context(|| format!("opening destination file {dest_path:?}"))
            .map_err(|e| {
                Box::new(EvalAltResult::ErrorSystem(
                    "failed to download".to_string(),
                    e.into(),
                ))
            })?;

        io::copy(
            &mut response.body_mut().as_reader(),
            &mut BufWriter::new(file),
        )
        .with_context(|| format!("writing download of {url} to {dest_path:?}"))
        .map_err(|e| {
            Box::new(EvalAltResult::ErrorSystem(
                "failed to download".to_string(),
                e.into(),
            ))
        })?;

        Ok(())
    }

    fn run(&mut self, program: String, args: Array) -> Result<String, Box<EvalAltResult>> {
        let string_args = args
            .into_iter()
            .map(|a| {
                a.into_string()
                    .map_err(|t| format!("run: all args must be strings, got {t}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        debug!("running command: {program} {string_args:?}");
        let build_dir = self.result.borrow().build_directory.clone();
        let output = Command::new(&program)
            .args(&string_args)
            .current_dir(&build_dir)
            .output()
            .map_err(|e| format!("failed to spawn command {program}: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "command {program} failed with status {}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        debug!(
            "command {program} succeeded ({} bytes stdout)",
            stdout.len()
        );
        Ok(stdout)
    }

    fn write(&mut self, destination: String, contents: String) -> Result<(), Box<EvalAltResult>> {
        let path = self.result.borrow().build_directory.join(&destination);
        fs::write(&path, contents).map_err(|e| format!("writing {path:?}: {e}"))?;
        Ok(())
    }

    fn export_root(&mut self, root: String) -> Result<(), Box<EvalAltResult>> {
        self.result.borrow_mut().export_root = PathBuf::from(root);
        Ok(())
    }

    fn export_exe(
        &mut self,
        source_path: String,
        system_path: String,
    ) -> Result<(), Box<EvalAltResult>> {
        self.add_export(source_path, system_path, BuildExportKind::Executable);
        Ok(())
    }

    fn export_share(
        &mut self,
        source_path: String,
        system_path: String,
    ) -> Result<(), Box<EvalAltResult>> {
        self.add_export(source_path, system_path, BuildExportKind::Share);
        Ok(())
    }

    fn export_config(
        &mut self,
        source_path: String,
        system_path: String,
    ) -> Result<(), Box<EvalAltResult>> {
        self.add_export(source_path, system_path, BuildExportKind::Config);
        Ok(())
    }

    fn add_export(&mut self, source_path: String, system_path: String, kind: BuildExportKind) {
        debug!("export {kind:?}: {source_path} -> {system_path}");
        self.result.borrow_mut().exports.push(BuildExport {
            kind,
            source_path: PathBuf::from(source_path),
            system_path: PathBuf::from(system_path),
        });
    }

    fn build_extra(builder: &mut TypeBuilder<Self>) {
        builder
            .with_get("os", |_obj: &mut Self| consts::OS)
            .with_get("arch", |_obj: &mut Self| consts::ARCH)
            .with_fn("download", Self::download)
            .with_fn("run", Self::run)
            .with_fn("write", Self::write)
            .with_fn("export_root", Self::export_root)
            .with_fn("export_exe", Self::export_exe)
            .with_fn("export_share", Self::export_share)
            .with_fn("export_config", Self::export_config);
    }
}
