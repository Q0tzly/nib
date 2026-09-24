# nib

A modal text editor where everything — including the default keymap — is a WebAssembly plugin.

> [!WARNING]
> nib is an early experiment. It is not usable as a daily editor yet, and the plugin API will change without notice.

## Idea

- **Small core.** The core owns buffers, selections, rendering, and the plugin host. Nothing else.
- **Plugins in any language.** Plugins are WebAssembly components built against a single API definition (WIT). Write them in Rust, Go, or anything that targets the component model.
- **Defaults are plugins too.** The standard feature set, starting with a Helix-style keymap, is built with the same public API that third-party plugins use.

## Layout

```
core/      editor core (buffers, selections, rendering, plugin host)
tui/       terminal frontend, builds the `nib` binary
api/       plugin API definitions (WIT) — the single source of truth
sdk/       plugin SDKs per language
plugins/   standard plugins
docs/      design documents
bench/     latency benchmark against other editors
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Plugin SDKs are under the same terms, so plugin authors are free to choose any license for their plugins.

See [CONTRIBUTING.md](CONTRIBUTING.md) for how contributions are licensed.
