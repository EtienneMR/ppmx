#[derive(Debug, allocative::Allocative)]
pub struct PackageAsset {
    pub kind: PackageAssetKind,
    pub source_path: std::path::PathBuf,
    pub system_path: std::path::PathBuf,
}

#[derive(Debug, allocative::Allocative)]
pub enum PackageAssetKind {
    Executable,
    Share,
    Config,
}
