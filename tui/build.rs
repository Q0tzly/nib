//! Embeds the standard plugins built by `cargo xtask build-plugins`, so `nib`
//! works without any setup. Without them, the binary still builds, with a
//! warning, so checks do not need the wasm toolchain.

use std::path::{Path, PathBuf};
use std::{env, fs};

/// Built in, in load order: the keymap first, so it is at the bottom of the
/// input stack.
const STANDARD_PLUGINS: &[&str] = &[
    "helix", "picker", "lsp", "bash", "go", "json", "markdown", "python", "rust", "toml", "yaml",
];

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("..");
    // A static, not a const: a const's data is copied into every place that
    // uses it, which doubled the grammars in the binary.
    let mut code = String::from(
        "/// A plugin's files besides its manifest, by path.\n\
         pub type Files = &'static [(&'static str, &'static [u8])];\n\
         pub static PLUGINS: &[(&str, &str, Files)] = &[\n",
    );
    for name in STANDARD_PLUGINS {
        let dir = root.join("target/plugins").join(name);
        let manifest = dir.join("plugin.toml");
        println!("cargo::rerun-if-changed={}", dir.display());
        if !manifest.is_file() {
            println!(
                "cargo::warning=the {name} plugin is not built into nib; run `cargo xtask build-plugins` first"
            );
            continue;
        }
        let mut files = Vec::new();
        collect(&dir, &dir, &mut files);
        code += &format!("    ({name:?}, include_str!({manifest:?}), &[\n");
        for (relative, path) in files {
            println!("cargo::rerun-if-changed={}", path.display());
            code += &format!("        ({relative:?}, include_bytes!({path:?})),\n");
        }
        code += "    ]),\n";
    }
    code += "];\n";
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("builtin_plugins.rs");
    fs::write(out, code).unwrap();
}

/// The files under `dir` besides the manifest, with paths relative to `root`
/// written with `/`.
fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(root, &path, files);
        } else if path != root.join("plugin.toml") {
            let relative = path.strip_prefix(root).unwrap();
            let relative: Vec<_> = relative.iter().map(|p| p.to_string_lossy()).collect();
            files.push((relative.join("/"), path));
        }
    }
}
