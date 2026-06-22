use std::io::Read;
use std::path::{Path, PathBuf};

use super::PluginManifestError;

const MANIFEST_MAX_BYTES: u64 = 1024 * 1024;

pub(super) struct ManifestSource {
    pub(super) display_path: PathBuf,
    pub(super) canonical_path: PathBuf,
    pub(super) input: String,
}

pub(super) fn load_manifest_source(path: &Path) -> Result<ManifestSource, PluginManifestError> {
    let display_path = if path.is_dir() {
        path.join("phux-plugin.toml")
    } else {
        path.to_path_buf()
    };
    let metadata = std::fs::metadata(&display_path)?;
    if !metadata.is_file() {
        return Err(PluginManifestError::Invalid(format!(
            "{} is not a regular file",
            display_path.display()
        )));
    }
    reject_oversized(metadata.len())?;
    let input = read_manifest_string(&display_path)?;
    Ok(ManifestSource {
        canonical_path: display_path.canonicalize()?,
        display_path,
        input,
    })
}

fn read_manifest_string(path: &Path) -> Result<String, PluginManifestError> {
    let file = std::fs::File::open(path)?;
    let mut reader = file.take(MANIFEST_MAX_BYTES + 1);
    let mut input = String::new();
    reader.read_to_string(&mut input)?;
    let len = u64::try_from(input.len()).map_err(|_| oversized_error())?;
    reject_oversized(len)?;
    Ok(input)
}

fn reject_oversized(len: u64) -> Result<(), PluginManifestError> {
    if len > MANIFEST_MAX_BYTES {
        return Err(oversized_error());
    }
    Ok(())
}

fn oversized_error() -> PluginManifestError {
    PluginManifestError::Invalid(format!(
        "plugin manifest exceeds {MANIFEST_MAX_BYTES} byte limit"
    ))
}
