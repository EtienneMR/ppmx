use std::{path::PathBuf, rc::Rc};

use log::{debug, info, trace};
use rhai::{
    module_resolvers::StaticModuleResolver,
    packages::{Package, StandardPackage},
    plugin::*,
};
use ureq::Agent;

pub struct Runtime {
    http_agent: Option<Agent>,
    working_directory: Option<PathBuf>,
    idempotence_required: bool,
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            http_agent: None,
            working_directory: None,
            idempotence_required: true,
        }
    }

    fn from_tag(tag: Option<&Dynamic>) -> Rc<Self> {
        tag.expect("engine should be tagged with a runtime")
            .clone_cast::<Rc<Runtime>>()
    }

    pub fn into_engine(self) -> Engine {
        let mut engine = Engine::new_raw();

        engine.on_print(|s| info!("{}", s));
        engine.on_debug(|s, src, pos| debug!("{} @ {:?} > {}", src.unwrap_or("unknown"), pos, s));

        engine.register_global_module(StandardPackage::new().as_shared_module());

        engine.set_default_tag(Dynamic::from(Rc::new(self)));

        let mut resolver = StaticModuleResolver::new();
        resolver.insert("std:recipe", recipe::rhai_module_generate());
        resolver.insert("std:env", env::rhai_module_generate());
        resolver.insert("std:fs", fs::rhai_module_generate());
        resolver.insert("std:github", github::rhai_module_generate());
        resolver.insert("std:http", http::rhai_module_generate());
        engine.set_module_resolver(resolver);

        engine
    }

    pub fn with_http_agent(mut self, agent: Agent) -> Self {
        self.http_agent = Some(agent);
        self
    }

    pub fn with_working_directory(mut self, path: PathBuf) -> Self {
        self.working_directory = Some(path);
        self
    }

    pub fn allow_non_idempotent(mut self) -> Self {
        self.idempotence_required = false;
        self
    }

    fn http_agent(&self) -> Result<&Agent, Box<EvalAltResult>> {
        self.http_agent
            .as_ref()
            .ok_or_else(|| "current runtime does not have access to an HTTP agent".into())
    }

    fn working_directory(&self) -> Result<&PathBuf, Box<EvalAltResult>> {
        self.working_directory
            .as_ref()
            .ok_or_else(|| "current runtime does not have access to a working directory".into())
    }

    fn ensure_idempotence_not_required(&self) -> Result<(), Box<EvalAltResult>> {
        if self.idempotence_required {
            Err("current runtime does not allow non-idempotent operations".into())
        } else {
            Ok(())
        }
    }
}

#[export_module]
mod recipe {
    use crate::backend::recipes::{
        BuildExport, BuildExportKind, BuildResult, Dependencies, PackageVersion,
    };

    pub fn version(name: String) -> PackageVersion {
        PackageVersion {
            name,
            metadata: ().into(),
        }
    }

    #[rhai_fn(pure, get = "name")]
    pub fn version_name(version: &mut PackageVersion) -> String {
        version.name.clone()
    }

    #[rhai_fn(pure, get = "metadata")]
    pub fn version_metadata(version: &mut PackageVersion) -> Dynamic {
        version.metadata.clone()
    }

    #[rhai_fn(global)]
    pub fn with_metadata(mut version: PackageVersion, metadata: Dynamic) -> PackageVersion {
        version.metadata = metadata;
        version
    }

    pub fn dependencies() -> Dependencies {
        Dependencies {
            packages: Vec::new(),
        }
    }

    #[rhai_fn(global)]
    pub fn with_package(res: Dependencies, package: &str) -> Dependencies {
        let mut clone = res.clone();
        clone.packages.push(package.to_owned());
        clone
    }

    pub fn build(root: &str) -> BuildResult {
        BuildResult {
            export_root: PathBuf::from(root),
            exports: Vec::new(),
        }
    }

    #[rhai_fn(global)]
    pub fn with_exe(res: BuildResult, source_path: &str, system_path: &str) -> BuildResult {
        with_export(res, source_path, system_path, BuildExportKind::Executable)
    }

    #[rhai_fn(global)]
    pub fn with_share(res: BuildResult, source_path: &str, system_path: &str) -> BuildResult {
        with_export(res, source_path, system_path, BuildExportKind::Share)
    }

    #[rhai_fn(global)]
    pub fn with_config(res: BuildResult, source_path: &str, system_path: &str) -> BuildResult {
        with_export(res, source_path, system_path, BuildExportKind::Config)
    }

    fn with_export(
        res: BuildResult,
        source_path: &str,
        system_path: &str,
        kind: BuildExportKind,
    ) -> BuildResult {
        trace!("export {kind:?}: {source_path} -> {system_path}");

        let mut new_res = res.clone();
        new_res.exports.push(BuildExport {
            kind,
            source_path: PathBuf::from(source_path),
            system_path: PathBuf::from(system_path),
        });
        new_res
    }
}

#[export_module]
mod env {
    use std::env::consts;

    use rhai::Map;

    pub const OS: &str = consts::OS;
    pub const ARCH: &str = consts::ARCH;

    #[rhai_fn(return_raw)]
    pub fn map_arch(map: Map) -> Result<Dynamic, Box<EvalAltResult>> {
        if let Some(arch) = map.get(ARCH) {
            Ok(arch.clone())
        } else {
            Err(format!(
                "arch {ARCH:?} is not supported by this recipe\nsupported archs: {}",
                map.keys()
                    .into_iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<&str>>()
                    .join(", "),
            )
            .into())
        }
    }
}

#[export_module]
mod fs {
    use std::{
        fs::OpenOptions,
        io::{self, BufWriter},
        process::Command,
    };

    use anyhow::{Context, anyhow};
    use rhai::Array;

    #[rhai_fn(volatile, return_raw)]
    pub fn download(
        ctx: NativeCallContext,
        url: &str,
        destination: &str,
    ) -> Result<(), Box<EvalAltResult>> {
        trace!("downloading {url} -> {destination}");

        let runtime = Runtime::from_tag(ctx.tag());
        let working_directory = runtime.working_directory()?;
        let http_agent = runtime.http_agent()?;

        let mut response = http_agent
            .get(url)
            .call()
            .with_context(|| format!("failed to send download request for {url}"))
            .map_err(to_runtime_error)?;

        let dest_path = working_directory.join(destination);
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&dest_path)
            .with_context(|| format!("failed to open destination file {dest_path:?}"))
            .map_err(to_runtime_error)?;

        io::copy(
            &mut response.body_mut().as_reader(),
            &mut BufWriter::new(file),
        )
        .with_context(|| format!("failed to write download of {url} to {dest_path:?}"))
        .map_err(to_runtime_error)?;

        Ok(())
    }

    #[rhai_fn(volatile, return_raw)]
    pub fn run(
        ctx: NativeCallContext,
        program: String,
        args: Array,
    ) -> Result<String, Box<EvalAltResult>> {
        let runtime = Runtime::from_tag(ctx.tag());
        let working_directory = runtime.working_directory()?;

        let string_args = args
            .into_iter()
            .map(|a| {
                a.into_string()
                    .map_err(|t| anyhow!("run: all args must be strings, got {t}"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(to_runtime_error)?;

        trace!("running command: {program} {string_args:?}");
        let output = Command::new(&program)
            .args(&string_args)
            .current_dir(working_directory)
            .output()
            .with_context(|| format!("failed to spawn command {program}"))
            .map_err(to_runtime_error)?;

        if !output.status.success() {
            return Err(anyhow!(
                "command {program} failed with status {}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ))
            .map_err(to_runtime_error);
        }
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        trace!(
            "command {program} succeeded ({} bytes stdout)",
            stdout.len()
        );
        Ok(stdout)
    }

    #[rhai_fn(volatile, return_raw)]
    pub fn write(
        ctx: NativeCallContext,
        destination: String,
        contents: String,
    ) -> Result<(), Box<EvalAltResult>> {
        let runtime = Runtime::from_tag(ctx.tag());
        let working_directory = runtime.working_directory()?;

        let path = working_directory.join(&destination);
        std::fs::write(&path, contents)
            .with_context(|| format!("failed to write into {path:?}"))
            .map_err(to_runtime_error)?;
        Ok(())
    }
}

#[export_module]
mod github {
    use anyhow::Context;
    use serde::Deserialize;

    #[rhai_fn(volatile, return_raw)]
    pub fn latest_release(
        ctx: NativeCallContext,
        repo: &str,
    ) -> Result<String, Box<EvalAltResult>> {
        trace!("fetching latest release for {repo}");

        let runtime = Runtime::from_tag(ctx.tag());
        runtime.ensure_idempotence_not_required()?;
        let http_agent = runtime.http_agent()?;

        #[derive(Deserialize)]
        struct Release {
            tag_name: String,
        }
        let url = format!("https://api.github.com/repos/{repo}/releases/latest");
        let release: Release = http_agent
            .get(&url)
            .call()
            .and_then(|mut r| r.body_mut().read_json())
            .with_context(|| format!("failed to fetch latest release of {repo} at {url}"))
            .map_err(to_runtime_error)?;
        trace!("latest release for {repo} is {}", release.tag_name);
        Ok(release.tag_name)
    }

    #[rhai_fn(volatile, return_raw)]
    pub fn repo_branch_reference(
        ctx: NativeCallContext,
        repo: &str,
        branch: &str,
    ) -> Result<String, Box<EvalAltResult>> {
        trace!("fetching latest ref for {repo}#{branch}");

        let runtime = Runtime::from_tag(ctx.tag());
        runtime.ensure_idempotence_not_required()?;
        let http_agent = runtime.http_agent()?;

        #[derive(Deserialize)]
        struct GitObject {
            sha: String,
        }

        #[derive(Deserialize)]
        struct GitRef {
            object: GitObject,
        }

        let url = format!("https://api.github.com/repos/{repo}/git/refs/heads/{branch}");
        let fetched_repo: GitRef = http_agent
            .get(&url)
            .call()
            .and_then(|mut r| r.body_mut().read_json())
            .with_context(|| format!("failed to fetch latest ref for {repo}#{branch} at {url}"))
            .map_err(to_runtime_error)?;

        trace!("repo {repo}#{branch} is at {}", fetched_repo.object.sha);
        Ok(fetched_repo.object.sha)
    }

    pub fn asset_url(repo: &str, tag: &str, asset_name: &str) -> String {
        format!("https://github.com/{repo}/releases/download/{tag}/{asset_name}")
    }

    pub fn tarball_url(repo: &str, reference: &str) -> String {
        format!("https://api.github.com/repos/{repo}/tarball/{reference}")
    }
}

#[export_module]
mod http {
    use anyhow::Context;
    use rhai::Blob;

    #[rhai_fn(volatile, return_raw)]
    pub fn get(ctx: NativeCallContext, url: &str) -> Result<Blob, Box<EvalAltResult>> {
        trace!("fetching http url {url}");

        let runtime = Runtime::from_tag(ctx.tag());
        let http_agent = runtime.http_agent()?;

        let body: Vec<u8> = http_agent
            .get(url)
            .call()
            .and_then(|mut r| r.body_mut().read_to_vec())
            .with_context(|| format!("failed to fetch http url {url}"))
            .map_err(to_runtime_error)?;

        Ok(body)
    }
}

fn to_runtime_error(err: anyhow::Error) -> Box<EvalAltResult> {
    format!("{:?}", err.context("failed to run recipe")).into()
}
