//! Build steps that plain cargo cannot express. Run with `cargo xtask <task>`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "usage: cargo xtask build-plugins";

fn main() -> ExitCode {
    let result = match std::env::args().nth(1).as_deref() {
        Some("build-plugins") => build_plugins(),
        _ => Err(USAGE.into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("xtask: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Builds every plugin under `plugins/` for wasm32-wasip2 and lays them out
/// as `target/plugins/<name>/{plugin.toml,plugin.wasm}`.
fn build_plugins() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the repository root");
    let target = root.join("target");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args([
            "build",
            "--release",
            "--workspace",
            "--target",
            "wasm32-wasip2",
        ])
        .arg("--manifest-path")
        .arg(root.join("plugins/Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .status()
        .map_err(|err| format!("failed to run cargo: {err}"))?;
    if !status.success() {
        return Err("building plugins failed".into());
    }

    for dir in plugin_dirs(&root.join("plugins"))? {
        let package = read_toml(&dir.join("Cargo.toml"))?;
        let package = package["package"]["name"]
            .as_str()
            .ok_or_else(|| format!("{}: no package name", dir.display()))?;
        let manifest = read_toml(&dir.join("plugin.toml"))?;
        let name = manifest["name"]
            .as_str()
            .ok_or_else(|| format!("{}: plugin.toml has no name", dir.display()))?;

        let wasm = target
            .join("wasm32-wasip2/release")
            .join(format!("{}.wasm", package.replace('-', "_")));
        let out = target.join("plugins").join(name);
        fs::create_dir_all(&out).map_err(|err| format!("{}: {err}", out.display()))?;
        copy(&wasm, &out.join("plugin.wasm"))?;
        copy(&dir.join("plugin.toml"), &out.join("plugin.toml"))?;
        println!("built {name} -> {}", out.display());
    }
    Ok(())
}

/// Directories under `dir` that contain a `plugin.toml`.
fn plugin_dirs(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let entries = fs::read_dir(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|err| err.to_string())?.path();
        if !path.is_dir() || path.ends_with("target") {
            continue;
        }
        if path.join("plugin.toml").is_file() {
            found.push(path);
        } else {
            found.extend(plugin_dirs(&path)?);
        }
    }
    found.sort();
    Ok(found)
}

fn read_toml(path: &Path) -> Result<toml::Table, String> {
    fs::read_to_string(path)
        .map_err(|err| format!("{}: {err}", path.display()))?
        .parse()
        .map_err(|err| format!("{}: {err}", path.display()))
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    fs::copy(from, to)
        .map(|_| ())
        .map_err(|err| format!("copying {} failed: {err}", from.display()))
}
