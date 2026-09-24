# プラグイン API

> ステータス: 草案。末尾の「要確認」は未決。

[architecture.md](architecture.md) で決めたコアの構造を、プラグインから見た API に落とし込む。WIT の完全な定義は実装時に `api/` に置く。ここでは設計の方針と API の形を決める。

## 方針

1. **型のある API と、名前で呼ぶコマンドを使い分ける。**
   - バッファの読み書き、選択、UI のように頻繁に呼ぶものは、WIT の型つき関数にする。
   - プラグイン同士の連携や、設定・コマンドラインから呼ぶ操作は、名前つきのコマンドにする。
2. **画面の座標を出さない。** 位置はすべてバッファ上のバイトオフセットで表す。縦移動やスクロールのように画面の配置に依存する操作は、コアに依頼する。
3. **コアはプラグインを再入させない。** あるプラグインの呼び出し中に、同じプラグインをもう一度呼ぶことはない。イベントは、呼び出しが終わってから届ける。
4. **プラグインが作ったものは、プラグインと一緒に片付ける。** コマンド、装飾、UI 部品、入力スタックの層は、作ったプラグインが止まったときにコアが自動で取り除く。

## プラグインの形

プラグインは 1 つのディレクトリで、次の 2 つのファイルを持つ。

```
helix-keymap/
├── plugin.toml
└── plugin.wasm   # wasm32-wasip2 向けのコンポーネント
```

`plugin.toml` には、読み込む前に知る必要のある情報を書く。

```toml
name = "helix"            # コマンドの名前空間にもなる
version = "0.1.0"
api = "0.1"               # 対応する nib:plugin のバージョン

capabilities = []         # "fs-read" / "fs-write" / "process" / "network"
events = ["buffer-changed"]
```

権限をマニフェストで宣言させるのは、WASM の import だけでは権限を判定できないため。たとえば Rust の標準ライブラリを使うと、使っていなくても `wasi:filesystem` を import する。

## world

WIT パッケージは `nib:plugin`。プラグインは `plugin` world に対して書く。

以下の WIT は形を示すためのスケッチで、一部の型は省略している。wasm-tools での検証は、`api/` に置くときに行う。

```wit
package nib:plugin@0.1.0;

world plugin {
    import editor;
    import input;
    import commands;
    import ui;
    import events;
    import timers;
    import process;   // 宣言がなければ、呼ぶと permission-denied を返す

    export guest;
}
```

### プラグインが実装する関数（guest）

```wit
interface guest {
    use types.{key-event, event};

    enum key-result { handled, pass }

    /// 読み込み直後に 1 回呼ぶ。config は config.toml のプラグイン用の表を JSON にしたもの
    init: func(config: string) -> result<_, string>;

    /// 入力スタックに積んだ層にキーが届いたとき
    handle-key: func(ev: key-event) -> key-result;

    /// 自分が登録したコマンドが呼ばれたとき。引数と戻り値は JSON
    run-command: func(name: string, args: string) -> result<string, string>;

    /// 購読したイベント
    on-event: func(ev: event);
}
```

### コアが提供する関数（抜粋）

```wit
interface types {
    type offset = u64;

    record sel-range { anchor: offset, head: offset }
    record selection { ranges: list<sel-range>, primary: u32 }

    /// 変更前のバッファでの範囲 [start, end) を text で置き換える
    record edit { start: offset, end: offset, text: string }

    enum undo-mode { new-step, merge }
    enum cursor-shape { block, bar, underline }

    flags modifiers { ctrl, alt, shift, super }
    variant key-code {
        %char(char), enter, escape, tab, backspace, delete,
        up, down, left, right, home, end, page-up, page-down, f(u8),
    }
    record key-event { code: key-code, modifiers: modifiers }

    /// style はテーマの名前（例: "ui.statusline"）
    record span { text: string, style: string }
    type styled-line = list<span>;
}

interface editor {
    use types.{offset, selection, edit, undo-mode, cursor-shape};

    variant error { stale-version, invalid-position, closed }

    resource buffer {
        version: func() -> u64;
        len: func() -> offset;
        slice: func(start: offset, end: offset) -> string;
        line-count: func() -> u64;
        line-start: func(line: u64) -> offset;
        line-of: func(pos: offset) -> u64;
        next-grapheme: func(pos: offset) -> offset;
        prev-grapheme: func(pos: offset) -> offset;
        path: func() -> option<string>;
    }

    variant scroll-amount { lines(s32), half-page(s32), page(s32) }

    resource view {
        buffer: func() -> buffer;
        selection: func() -> selection;
        set-selection: func(sel: selection) -> result<_, error>;
        apply: func(base-version: u64, edits: list<edit>,
                    after: option<selection>, undo: undo-mode) -> result<_, error>;
        undo: func() -> bool;
        redo: func() -> bool;
        /// 表示上の行単位で縦に動かした位置を返す（折り返しとタブを考慮する）
        move-vertically: func(pos: offset, lines: s32) -> offset;
        scroll: func(amount: scroll-amount);
        /// 表示中のバッファの範囲
        visible-range: func() -> tuple<offset, offset>;
        set-cursor-shape: func(shape: cursor-shape);
    }

    active-view: func() -> view;
}
```

## バッファと選択

- 変更は `view.apply` で、`edit` のリストとして渡す。
  - 範囲はすべて変更前のバッファで数え、互いに重なってはいけない。LSP の `TextEdit` と同じ考え方なので、どの言語でも組み立てやすい。
  - `base-version` がバッファの現在のバージョンと違えば `stale-version` を返す。
- undo の単位は `undo-mode` で指定する。
  - `new-step` は新しい 1 手を始める。
  - `merge` は直前の 1 手にまとめる。
  - 挿入モードでは、最初の打鍵を `new-step`、以降を `merge` にすれば、挿入全体が 1 手になる。
- 書記素の境界は、コアが `next-grapheme` / `prev-grapheme` として提供する。プラグインごとに Unicode の表を持たなくて済み、描画とも結果が食い違わない。
- 縦移動（`j` / `k`）、スクロール、表示範囲は、画面の配置を知っているコアが計算する。プラグインは画面の行と列を知らないまま、これらの操作を書ける。

## コマンド

- `commands.register(name, description)` で登録する。登録名の前には、マニフェストの `name` が自動で付く（`move_next_word` → `helix.move_next_word`）。
- `buffer.`、`editor.`、`view.` で始まる名前はコア用に予約する。
- `commands.call(name, args)` で呼ぶ。
  - 引数と戻り値は JSON 文字列。
  - 呼び出しは同期的で、戻り値をその場で受け取れる。
  - 呼び出し先がすでに呼び出し中のプラグイン（呼び出し元自身や、その呼び出し元）なら、再入になるのでエラーを返す。
- コアのコマンドの例: `buffer.open`、`buffer.save`、`buffer.close`、`editor.quit`、`view.split`。

JSON を選んだのは、WIT に再帰する型がなく、任意の値の木を型で表せないため。どの言語にも JSON の実装はある。

## 入力

- `input.push-layer()` で入力スタックに層を積み、`input.pop-layer()` で外す。キーは上の層から順に `handle-key` で届き、`pass` を返すと下の層に回る。
- キーマッププラグインは `init` で 1 層積み、それを外さない。
- 貼り付け（bracketed paste）は、キーではなく `paste` イベントとして届ける。

## イベント

- プラグインは、マニフェストの `events` に書いた種類のイベントだけを受け取る。全イベントを全プラグインに配ることはしない。
- 主なイベント:

| イベント | 内容 |
|----------|------|
| `buffer-opened` / `buffer-closed` / `buffer-saved` | バッファの出入り |
| `buffer-changed` | 適用された `edit` の列と、変更前後のバージョン。LSP の差分同期に使えるよう、変更前の行と列も付ける |
| `selection-changed` | 選択が変わった |
| `paste` | 貼り付けられた文字列 |
| `timer` | `timers.set` で予約した時刻になった |
| `process-output` / `process-exit` | 起動した外部プロセスの出力と終了 |
| `custom` | 他のプラグインが `events.emit(name, json)` で出したもの |

- `custom` は、コマンドと対になる仕組み。コマンドは「誰かに頼む」、`custom` イベントは「起きたことを知らせる」。たとえばキーマッププラグインがモードの変化を知らせ、ステータスラインのプラグインがそれを表示する。
- イベントは、それを起こした呼び出しが終わってから、起きた順に届ける。

## UI

| 関数 | 用途 |
|------|------|
| `ui.set-status(id, side, priority, content)` | ステータスラインに項目を出す。`side` は左・中・右、同じ側では `priority` 順に並べる |
| `ui.open-popup(anchor, lines)` | バッファ上の位置に結びつけたポップアップ。`popup` リソースを返す |
| `ui.open-panel(lines)` | 画面下端のパネル。`panel` リソースを返し、`update` で中身を差し替える |
| `panel.set-cursor(line, pos)` | パネル内のカーソル。コマンドラインの入力位置に使う |

中身はすべて `styled-line` で渡し、配置と切り詰めはコアが行う。

装飾（ハイライト、下線、行内の文字列、行頭の記号）は、`ui.set-decorations(buffer, namespace, list<decoration>)` で名前空間ごとにまとめて差し替える。付けたあとの位置の追従はコアが行う。

## 権限

| 権限 | 与えるもの |
|------|------------|
| （なし） | 上記の API すべてと、プラグイン専用のデータディレクトリ（`~/.local/share/nib/plugins/<name>/`） |
| `fs-read` / `fs-write` | 作業ディレクトリ以下の読み取り / 書き込み（WASI の preopen で渡す） |
| `process` | `process.spawn` による外部プロセスの起動 |
| `network` | `wasi:sockets` / `wasi:http` |

- 宣言した権限は、確認なしですべて与える。
- 宣言していない権限は与えない。
- 読み込んでいるプラグインと権限は、`editor.plugins` コマンドで一覧できる。
- 大量のファイルを列挙するような重い I/O は、コアが非同期の仕事として提供し、結果をイベントで返す。WASI のファイル API は同期的なので、プラグインが直接やるとメインスレッドが止まる。

## ライフサイクル

1. `plugin.toml` を読み、`api` のバージョンを確かめる。合わなければ読み込まずにエラーを表示する。
2. `plugin.wasm` をコンパイルする（キャッシュがあれば使う）。
3. 宣言した権限に合わせて import を用意し、インスタンスを作る。
4. `init` を呼ぶ（時間の上限は 5 秒）。コマンドの登録や入力スタックへの層の追加は、ここで行う。
5. 以降は、キー・コマンド・イベントのたびに呼ぶ（時間の上限は 1 秒）。
6. 停止やトラップが起きたら、そのプラグインが作ったものをすべて片付け、新しいインスタンスで 3 からやり直す。1 分間に 3 回落ちたら無効にする。

再起動すると、プラグインの内部状態は失われる。残したい状態は、データディレクトリに書く。

## バージョン

- `nib:plugin` は semver で管理する。1.0 までは互換性を保証しない。
- コアが対応するのは 1 つのバージョンだけ。標準プラグインは同じリポジトリにあるので、API を変えるときは同じコミットで直す。
- SDK のバージョンは、`nib:plugin` のバージョンが変わったときだけ上げる。

## SDK

- **Rust**（`sdk/rust`、クレート `nib-plugin`）: wit-bindgen の生成コードを包み、`Plugin` トレイトと `export!` マクロを提供する。`wasm32-wasip2` 向けにビルドするだけで、コンポーネントが出来上がる。
- **Go**（`sdk/go`）: TinyGo と wit-bindgen-go を使う想定。本家 Go の wasip2 対応の状況は、着手時に確認する。

## M1 で使う範囲

Helix 風キーマップで nib 自身を編集するのに必要なものだけを、先に作る。

| 範囲 | M1 |
|------|----|
| `editor`（バッファ、選択、`apply`、undo、縦移動、スクロール、カーソルの形） | ○ |
| `input`（入力スタック） | ○ |
| `commands`（登録と呼び出し、`buffer.open` / `buffer.save` / `editor.quit`） | ○ |
| `ui.set-status`、`ui.open-panel` | ○ |
| `events`（`buffer-changed`、`custom`） | ○ |
| `ui.open-popup`、装飾 | M1 のあと |
| `timers`、`process`、権限の宣言 | M1 のあと（LSP と一緒） |
| 構文木の API | M1 のあと（tree-sitter と一緒） |
| Go SDK | M1 のあと |

## 要確認

- **宣言していない権限を拒否する**: vision.md の「当面は制限をかけない」より一歩強い。宣言したものは確認なしで与えるので手間は増えない。そのうえで、一覧の内容が常に正しくなる。
- **設定とコマンドの引数を JSON にする**: config.toml に書いた表も、コアが JSON に直して渡す。プラグインは TOML のパーサーを持たなくて済む。
- **Go SDK を M1 のあとに回す**: 想定ユーザーは作者だけなので、M1 は Rust SDK だけで足りる。
