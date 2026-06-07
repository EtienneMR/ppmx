use std::fmt;
use std::fs;
use std::fs::OpenOptions;
use std::io::BufWriter;
use std::path::PathBuf;
use std::process::Command;

use allocative::Allocative;
use anyhow::bail;
use anyhow::Context;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::environment::MethodsStatic;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::list_or_tuple::UnpackListOrTuple;
use starlark::values::none::NoneType;
use starlark::values::NoSerialize;
use starlark::values::ProvidesStaticType;
use starlark::values::StarlarkValue;
use starlark::values::Value;
use starlark::values::ValueLike;
use starlark_derive::starlark_value;
use starlark_derive::StarlarkAttrs;
use starlark_derive::Trace;

use crate::executor::RecipeExecutor;

#[derive(Debug, ProvidesStaticType, NoSerialize, Allocative, Trace, StarlarkAttrs)]
pub struct BuildContext {
    version: String,

    #[starlark(skip)]
    working_directory: PathBuf,
}

impl BuildContext {
    pub fn new(version: String, working_directory: PathBuf) -> Self {
        Self {
            version,
            working_directory,
        }
    }
}

impl fmt::Display for BuildContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<build-context at {:?}>", self.working_directory)
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

        let mut response = exec.http_client.get(url).send()?.error_for_status()?;
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(this.working_directory.join(destination))
            .context("failed to open file")?;

        response
            .copy_to(&mut BufWriter::new(file))
            .context("failled to write into file")?;

        Ok(NoneType)
    }

    fn run(
        this: Value,
        program: &str,
        #[starlark(args)] args: UnpackListOrTuple<&str>,
    ) -> anyhow::Result<String> {
        let this: &BuildContext = this.downcast_ref().context("invalid this")?;

        let output = Command::new(program)
            .args(args.items)
            .current_dir(&this.working_directory)
            .output()
            .context("command failled")?;

        if !output.status.success() {
            bail!(
                "command failled with status {}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn write(this: Value, destination: &str, contents: &str) -> anyhow::Result<NoneType> {
        let this: &BuildContext = this.downcast_ref().context("invalid this")?;

        fs::write(this.working_directory.join(destination), contents)?;
        Ok(NoneType)
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
