# Contributing

nib is at an early, experimental stage. Design discussion in issues is more useful than feature requests right now.

## Development

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same checks.

## Licensing of contributions

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in nib by you, as defined in the Apache-2.0 license, shall be dual licensed as in [README](README.md#license), without any additional terms or conditions.

## Code from other projects

Some code may be adapted from [Helix](https://github.com/helix-editor/helix), which is licensed under MPL-2.0. MPL-2.0 is file-level copyleft, so:

- Files containing code adapted from Helix stay under MPL-2.0. Keep the original copyright notice and add an MPL-2.0 header to the file.
- List every such file in [THIRD_PARTY.md](THIRD_PARTY.md) with the upstream path and commit it came from.
- Do not mix MPL-2.0 code into the plugin SDKs (`sdk/`) or the API definitions (`api/`). These must stay MIT OR Apache-2.0 so plugin authors are not constrained.
