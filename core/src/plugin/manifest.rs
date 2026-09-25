use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::Error;

/// `plugin.toml`: what the editor needs to know before loading a plugin.
#[derive(Debug, Deserialize)]
pub(crate) struct Manifest {
    /// Also the namespace of the plugin's commands.
    pub name: String,
    pub version: String,
    /// The `nib:plugin` version the plugin was built against.
    pub api: String,
}

pub(crate) fn read(path: &Path) -> Result<Manifest, Error> {
    let fail = |message: String| Error::Plugin(format!("{}: {message}", path.display()));
    let text = fs::read_to_string(path).map_err(|err| fail(err.to_string()))?;
    let manifest: Manifest = toml::from_str(&text).map_err(|err| fail(err.to_string()))?;
    let valid_name = !manifest.name.is_empty()
        && manifest
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid_name {
        return Err(fail(format!(
            "name {:?} must be lowercase letters, digits, and '-'",
            manifest.name
        )));
    }
    Ok(manifest)
}
