# nib

WASM プラグインで全機能を構成するモーダルエディタ。標準キーマップ（Helix 風）もプラグインとして実装する。

## 構成

- `core/` — エディタ本体（crate: `nib-core`）。バッファ・選択・描画・プラグインホストだけを持つ
- `tui/` — ターミナルのフロントエンド（crate: `nib-tui`、実行ファイル `nib`）。`nib-core` は端末に依存しない
- `api/` — プラグイン API の WIT 定義。コアと全 SDK の唯一の正
- `sdk/<lang>/` — 言語別プラグイン SDK
- `plugins/` — 標準プラグイン（`plugins/test/` はテスト用）。公開 API だけで書く（コア内部に依存しない）。wasm32-wasip2 専用の別ワークスペース
- `xtask/` — cargo だけでは書けないビルド手順（`cargo xtask build-plugins`）
- `docs/` — 設計ドキュメント
- `bench/` — 既存エディタと比べる性能計測（`python3 bench/latency.py FILE`）

機能をコアに入れるかプラグインにするか迷ったら、プラグイン側に倒す。コアに入れるのは「プラグインからは実現できない」か「複数のプラグインが共有する基盤で、各プラグインに持たせると重複や性能の問題が出る」もの（例: tree-sitter の解析基盤）だけ。

## コマンド

```sh
cargo xtask build-plugins   # plugins/ を wasm32-wasip2 向けにビルドし target/plugins/ に置く。コアの統合テストが使う
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

プラグインは別ワークスペース（`plugins/Cargo.toml`）なので、fmt と clippy は `--manifest-path plugins/Cargo.toml` を付けて別に回す（clippy は `--target wasm32-wasip2`）。CI も同じ内容（test は Linux / macOS / Windows）。

`api/wit/` を変えたら `cargo xtask build-plugins` をやり直す。古いプラグインは読み込みで型が合わずに失敗する。

## ライセンス

- リポジトリ全体は `MIT OR Apache-2.0`。
- Helix（MPL-2.0）からコードを流用したファイルは MPL-2.0 のまま残す。元の著作権表示を保持し、`THIRD_PARTY.md` に upstream のパスとコミットを記録する。
- `api/` と `sdk/` には MPL コードを入れない。プラグイン作者のライセンス選択を縛らないため。

## バージョン

- エディタ本体は workspace の `version` で管理する。
- SDK はエディタとは別にバージョンを付ける。`api/` が変わらない限り SDK のバージョンは上げない。
- Go SDK は `sdk/go/` に独自の `go.mod` を置き、タグは `sdk/go/vX.Y.Z` 形式にする。

## 進め方

設計ドキュメント（`docs/`）を先に書き、それに沿って実装する。実装中に設計が変わったら、同じコミットで docs も更新する。

`docs/` は日本語で書く。README・CONTRIBUTING・コード内のコメントと識別子は英語。
