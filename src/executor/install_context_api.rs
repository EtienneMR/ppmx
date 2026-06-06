use std::cell::RefCell;
use std::fmt;
use std::path::PathBuf;

use allocative::Allocative;
use anyhow::Context;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::environment::MethodsStatic;
use starlark::starlark_module;
use starlark::values::NoSerialize;
use starlark::values::ProvidesStaticType;
use starlark::values::StarlarkValue;
use starlark::values::Value;
use starlark::values::ValueLike;
use starlark::values::none::NoneType;
use starlark_derive::Trace;
use starlark_derive::starlark_value;

use crate::shared::PackageAsset;
use crate::shared::PackageAssetKind;

#[derive(Debug, ProvidesStaticType, NoSerialize, Allocative, Trace)]
pub struct InstallContext {
    pub assets: RefCell<Vec<PackageAsset>>,
}

impl InstallContext {
    pub fn new() -> Self {
        Self {
            assets: RefCell::new(Vec::new()),
        }
    }
}

impl fmt::Display for InstallContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<install-context>")
    }
}

#[starlark_module]
fn install_context_methods(builder: &mut MethodsBuilder) {
    fn exe(this: Value, source_path: &str, system_path: &str) -> anyhow::Result<NoneType> {
        add_asset(this, source_path, system_path, PackageAssetKind::Executable)
    }

    fn share(this: Value, source_path: &str, system_path: &str) -> anyhow::Result<NoneType> {
        add_asset(this, source_path, system_path, PackageAssetKind::Share)
    }

    fn config(this: Value, source_path: &str, system_path: &str) -> anyhow::Result<NoneType> {
        add_asset(this, source_path, system_path, PackageAssetKind::Config)
    }
}

fn add_asset(
    this: Value,
    source_path: &str,
    system_path: &str,
    kind: PackageAssetKind,
) -> anyhow::Result<NoneType> {
    let this: &InstallContext = this.downcast_ref().context("invalid this")?;
    this.assets.borrow_mut().push(PackageAsset {
        kind,
        source_path: PathBuf::from(source_path),
        system_path: PathBuf::from(system_path),
    });
    Ok(NoneType)
}

#[starlark_value(type = "install_context")]
impl<'v> StarlarkValue<'v> for InstallContext {
    fn get_methods() -> Option<&'static Methods> {
        static RES: MethodsStatic = MethodsStatic::new();
        RES.methods_for_type::<Self::Canonical>(install_context_methods)
    }
}
