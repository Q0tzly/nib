//! `nib plugin test`, run as people run it. Build the plugins first with
//! `cargo xtask build-plugins`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn built(name: &str) -> PathBuf {
    let dir = root().join("target/plugins").join(name);
    assert!(
        dir.join("plugin.toml").is_file(),
        "{} is missing; run `cargo xtask build-plugins` first",
        dir.display()
    );
    dir
}

fn nib_plugin_test(dir: &Path, file: &Path) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_nib"))
        .args(["plugin", "test"])
        .arg(dir)
        .arg(file)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    (output.status.success(), text)
}

#[test]
fn the_helix_plugin_passes_its_tests() {
    let file = root().join("plugins/helix/tests/basics.toml");
    let (ok, out) = nib_plugin_test(&built("helix"), &file);
    assert!(ok, "{out}");
    assert!(out.contains("5 passed, 0 failed"), "{out}");
}

#[test]
fn failures_show_what_was_expected() {
    let file = env::temp_dir().join(format!("nib-{}-failing.toml", std::process::id()));
    fs::write(
        &file,
        "with = []\n\
         [[test]]\nname = \"wrong keys\"\ntext = \"#[x|]#\\n\"\nkeys = \"a<nope>\"\n\
         [[test]]\nname = \"wrong text\"\ntext = \"x\\n\"\n\
         [test.expect]\ntext = \"y\\n\"\n",
    )
    .unwrap();
    let (ok, out) = nib_plugin_test(&built("test-insert"), &file);
    fs::remove_file(&file).unwrap();
    assert!(!ok, "{out}");
    assert!(out.contains("wrong keys ... FAILED"), "{out}");
    assert!(out.contains("keys: unknown key \"nope\""), "{out}");
    assert!(out.contains("expected: \"y\\n\""), "{out}");
    assert!(out.contains("actual:   \"x\\n\""), "{out}");
    assert!(out.contains("0 passed, 2 failed"), "{out}");
}
