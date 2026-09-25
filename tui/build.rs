//! Embeds the standard plugins built by `cargo xtask build-plugins`, so `nib`
//! works without any setup. Without them, the binary still builds, with a
//! warning, so checks do not need the wasm toolchain.

use std::path::PathBuf;
use std::{env, fs};

const STANDARD_PLUGINS: &[&str] = &["helix"];

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("..");
    let mut code = String::from("pub const PLUGINS: &[(&str, &str, &[u8])] = &[\n");
    for name in STANDARD_PLUGINS {
        let dir = root.join("target/plugins").join(name);
        let (manifest, wasm) = (dir.join("plugin.toml"), dir.join("plugin.wasm"));
        println!("cargo::rerun-if-changed={}", manifest.display());
        println!("cargo::rerun-if-changed={}", wasm.display());
        if manifest.is_file() && wasm.is_file() {
            code +=
                &format!("    ({name:?}, include_str!({manifest:?}), include_bytes!({wasm:?})),\n");
        } else {
            println!(
                "cargo::warning=the {name} plugin is not built into nib; run `cargo xtask build-plugins` first"
            );
        }
    }
    code += "];\n";
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("builtin_plugins.rs");
    fs::write(out, code).unwrap();
}
