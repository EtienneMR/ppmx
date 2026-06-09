use std::{
    cell::RefCell,
    env::consts,
    fmt,
    fs::{self, OpenOptions},
    io::BufWriter,
    path::PathBuf,
    process::Command,
};

use allocative::Allocative;
use anyhow::{Context, Result, bail};
use log::debug;
use serde::Deserialize;
use starlark::{
    environment::{
        Globals, GlobalsBuilder, LibraryExtension, Methods, MethodsBuilder, MethodsStatic, Module,
    },
    eval::Evaluator,
    starlark_module,
    syntax::{AstModule, Dialect},
    values::{StarlarkValue, Value, ValueLike, list_or_tuple::UnpackListOrTuple, none::NoneType},
};
use starlark_derive::{NoSerialize, ProvidesStaticType, StarlarkAttrs, Trace, starlark_value};

// PUBLIC API

pub struct Recipe {
    ast: AstModule,
}

impl Recipe {
    pub fn from_content(filename: &str, content: String) -> anyhow::Result<Self> {
        let ast = AstModule::parse(
            &filename,
            content,
            &Dialect {
                enable_f_strings: true,
                ..Dialect::Standard
            },
        )
        .map_err(|e| e.into_anyhow())
        .with_context(|| format!("failed to parse recipe file {filename}"))?;

        Ok(Self { ast })
    }
}

#[derive(Debug, Allocative, Default)]
pub struct BuildResult {
    pub build_directory: PathBuf,
    pub export_root: PathBuf,
    pub exports: Vec<BuildExport>,
}

#[derive(Debug, allocative::Allocative)]
pub struct BuildExport {
    pub kind: BuildExportKind,
    pub source_path: PathBuf,
    pub system_path: PathBuf,
}

#[derive(Debug, allocative::Allocative)]
pub enum BuildExportKind {
    Executable,
    Share,
    Config,
}

// PRIVATE

#[derive(ProvidesStaticType)]
pub struct RecipeExecutor {
    globals: Globals,
    http_client: reqwest::blocking::Client,
}
impl RecipeExecutor {
    pub fn new(http_client: reqwest::blocking::Client) -> Self {
        Self {
            globals: GlobalsBuilder::extended_by(&[LibraryExtension::Print])
                .with_namespace("github", github_module)
                .build(),
            http_client,
        }
    }

    fn from_evaluator<'a>(eval: &'a Evaluator) -> &'a Self {
        eval.extra
            .expect("extra defined in eval_recipe")
            .downcast_ref::<RecipeExecutor>()
            .expect("extra defined in eval_recipe")
    }

    pub fn eval_latest_version(&self, recipe: &Recipe) -> Result<String> {
        debug!("evaluating latest_version");
        self.eval_recipe(recipe, |mut eval| {
            let function = eval
                .module()
                .get("latest_version")
                .context("function latest_version not defined in recipe")?;

            let res = eval
                .eval_function(function, &[], &[])
                .map_err(|e| e.into_anyhow())
                .context("evaluating latest_version")?;

            res.unpack_str()
                .context("latest_version must return a string")
                .map(|s| s.to_string())
        })
    }

    pub fn run_build(
        &self,
        recipe: &Recipe,
        version: String,
        working_directory: PathBuf,
    ) -> Result<BuildResult> {
        debug!(
            "running build for version {version} in {:?}",
            working_directory
        );
        self.eval_recipe(recipe, |mut eval| {
            let function = eval
                .module()
                .get("build")
                .context("function build not defined in recipe")?;

            let ctx = eval
                .heap()
                .alloc_complex_no_freeze(BuildContext::new(version, working_directory));

            eval.eval_function(function, &[ctx], &[])
                .map_err(|e| e.into_anyhow())
                .context("evaluating build")?;

            let ctx = ctx.downcast_ref::<BuildContext>().unwrap();

            Ok(ctx.data.take())
        })
    }

    fn eval_recipe<R>(&self, recipe: &Recipe, f: impl FnOnce(Evaluator) -> Result<R>) -> Result<R> {
        debug!("evaluating recipe module");
        Module::with_temp_heap(|module| {
            let mut eval = Evaluator::new(&module);
            eval.extra = Some(self);
            eval.eval_module(recipe.ast.clone(), &self.globals)
                .map_err(|e| e.into_anyhow())?;

            f(eval)
        })
    }
}

#[starlark_module]
fn github_module(builder: &mut GlobalsBuilder) {
    fn latest_release(repo: String, eval: &mut Evaluator) -> anyhow::Result<String> {
        debug!("fetching latest release for {repo}");
        let response = RecipeExecutor::from_evaluator(eval)
            .http_client
            .get(format!(
                "https://api.github.com/repos/{repo}/releases/latest"
            ))
            .send()
            .with_context(|| format!("sending request for latest release of {repo}"))?
            .error_for_status()
            .with_context(|| format!("fetching latest release of {repo}"))?;

        #[derive(Deserialize)]
        struct _Release {
            tag_name: String,
        }

        let release: _Release = response.json()?;

        debug!("latest release for {repo}: {}", release.tag_name);
        return Ok(release.tag_name);
    }

    fn asset_url(repo: &str, tag: &str, asset_name: &str) -> anyhow::Result<String> {
        Ok(format!(
            "https://github.com/{repo}/releases/download/{tag}/{asset_name}"
        ))
    }
}

#[derive(Debug, ProvidesStaticType, NoSerialize, Allocative, Trace, StarlarkAttrs)]
struct BuildContext {
    version: String,
    os: &'static str,
    arch: &'static str,

    #[starlark(skip)]
    data: RefCell<BuildResult>,
}

impl BuildContext {
    fn new(version: String, working_directory: PathBuf) -> Self {
        Self {
            version,
            os: consts::OS,
            arch: consts::ARCH,
            data: RefCell::new(BuildResult {
                build_directory: working_directory,
                ..BuildResult::default()
            }),
        }
    }
}

impl fmt::Display for BuildContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "<build-context at {:?}>",
            self.data.borrow().build_directory
        )
    }
}

#[starlark_module]
fn build_context_methods(builder: &mut MethodsBuilder) {
    fn download(
        this: Value,
        url: &str,
        destination: &str,
        eval: &mut Evaluator,
    ) -> anyhow::Result<NoneType> {
        let this: &BuildContext = this.downcast_ref().context("invalid this")?;
        let exec = RecipeExecutor::from_evaluator(eval);

        debug!("downloading {url} -> {destination}");
        let mut response = exec
            .http_client
            .get(url)
            .send()
            .with_context(|| format!("sending download request for {url}"))?
            .error_for_status()
            .with_context(|| format!("downloading {url}"))?;
        let dest_path = this.data.borrow().build_directory.join(destination);
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&dest_path)
            .with_context(|| format!("opening destination file {dest_path:?}"))?;

        response
            .copy_to(&mut BufWriter::new(file))
            .with_context(|| format!("writing download of {url} to {dest_path:?}"))?;

        Ok(NoneType)
    }

    fn run(
        this: Value,
        program: &str,
        #[starlark(args)] args: UnpackListOrTuple<&str>,
    ) -> anyhow::Result<String> {
        let this: &BuildContext = this.downcast_ref().context("invalid this")?;

        debug!("running command: {program} {:?}", args.items);
        let output = Command::new(program)
            .args(args.items)
            .current_dir(&this.data.borrow().build_directory)
            .output()
            .with_context(|| format!("failed to spawn command {program}"))?;

        if !output.status.success() {
            bail!(
                "command {program} failed with status {}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        debug!(
            "command {program} succeeded ({} bytes stdout)",
            stdout.len()
        );
        Ok(stdout)
    }

    fn write(this: Value, destination: &str, contents: &str) -> anyhow::Result<NoneType> {
        let this: &BuildContext = this.downcast_ref().context("invalid this")?;

        fs::write(
            this.data.borrow().build_directory.join(destination),
            contents,
        )?;
        Ok(NoneType)
    }

    fn export_root(this: Value, root: &str) -> anyhow::Result<NoneType> {
        let this: &BuildContext = this.downcast_ref().context("invalid this")?;

        this.data.borrow_mut().export_root = PathBuf::from(root);

        Ok(NoneType)
    }

    fn export_exe(this: Value, source_path: &str, system_path: &str) -> anyhow::Result<NoneType> {
        debug!("export_exe: {source_path} -> {system_path}");
        add_asset(this, source_path, system_path, BuildExportKind::Executable)
    }

    fn export_share(this: Value, source_path: &str, system_path: &str) -> anyhow::Result<NoneType> {
        debug!("export_share: {source_path} -> {system_path}");
        add_asset(this, source_path, system_path, BuildExportKind::Share)
    }

    fn export_config(
        this: Value,
        source_path: &str,
        system_path: &str,
    ) -> anyhow::Result<NoneType> {
        debug!("export_config: {source_path} -> {system_path}");
        add_asset(this, source_path, system_path, BuildExportKind::Config)
    }
}
#[starlark_value(type = "build_context")]
impl<'v> StarlarkValue<'v> for BuildContext {
    fn get_methods() -> Option<&'static Methods> {
        static RES: MethodsStatic = MethodsStatic::new();
        RES.methods_for_type::<Self::Canonical>(build_context_methods)
    }

    starlark::values::starlark_attrs!();
}

fn add_asset(
    this: Value,
    source_path: &str,
    system_path: &str,
    kind: BuildExportKind,
) -> anyhow::Result<NoneType> {
    let this: &BuildContext = this.downcast_ref().context("invalid this")?;
    this.data.borrow_mut().exports.push(BuildExport {
        kind,
        source_path: PathBuf::from(source_path),
        system_path: PathBuf::from(system_path),
    });
    Ok(NoneType)
}
