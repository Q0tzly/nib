# {{name}}

A plugin for [nib](https://github.com/nib-editor/nib), written in Go with its [Go SDK](https://github.com/nib-editor/nib/tree/main/sdk/go).

## Build

You need [TinyGo](https://tinygo.org/) 0.42 or later (with Go 1.25 to 1.27), binaryen's `wasm-opt`, and [wasm-tools](https://github.com/bytecodealliance/wasm-tools).

```sh
nib plugin build
```

This writes `plugin.wasm` next to `plugin.toml`.

## Test

```sh
nib plugin test
```

runs the tests in `tests/*.toml` in an editor without a terminal. Try the plugin for real with `nib --plugin .`.

## Release

Set `version` in `plugin.toml`, then push a tag of the same version:

```sh
git tag v0.1.0 && git push origin v0.1.0
```

The [release workflow](.github/workflows/release.yml) builds the plugin and publishes `{{name}}-0.1.0.nib.tar.gz`, which `nib plugin add OWNER/REPO` installs.

## Writing plugins

The [plugin guide](https://github.com/nib-editor/nib/blob/main/sdk/README.md) explains how plugins work, and the [WIT](https://github.com/nib-editor/nib/blob/main/api/wit/plugin.wit) lists every function nib offers.
