# SKK Zenz

SKKを現代的にしたいな!というモチベでPoCを作ってみます

- 左文脈を活用した変換候補のreranking
- 送りがなの母音の活用

以下、細かい構成の構想:

- Zenzaiを利用
  - 左文脈を活用した変換候補のreranking (・作成?) に使用
  - linux desktopではサポートが不十分らしい?ので、fcitx5の情報を収集しつつ、frontendが直前の情報を記憶しておきfallbackとする
- 送りがなの母音の活用
  - libskkでは参照されない、送りがなの母音情報を使って変換候補をフィルタする
  - system辞書は無改造のまま使えるようにする。user辞書には、recent usedとして母音つきのエントリを記録する
- エントリのソートは、(from_user?: bool, ai_rank: float)をkeyとするイメージ
- user辞書
  - sqliteとかのがいい気がしてる。
  - 使用したエントリをsystem辞書からコピーする挙動は残す
  - recencyの情報はrerankのためにあまり重要ではないはず (使用しない、ではなく、persistentにしない) 使用回数くらい保存しとく?
  - 明示辞書登録も当然記録する

## roadmap

- [x] fcitx5のIMの仮実装
  - 左文脈の取得の可否を実験する
- [x] zenzの組み込み、ランタイムとハードウェアごとのレイテンシを確認
  - space押下前から投機実行できるといいのですが…
- [ ] backendの実装
  - [x] `skk-zenz-backend`を通常のCargo依存として提供する
  - [x] SKK system辞書をbackendで読み込み、送りあり・送りなし候補を検索する
  - [x] SQLite user辞書へ登録・選択・削除・使用回数を保存する
  - [x] 永続化しないrecencyを候補順位へ反映する
  - [x] zenzで候補をスコアリングし、user辞書・recency・lossで並べる
  - [x] `prepare(context, input)`で変換前にKV cacheを投機更新する
  - [x] `lookup(context, input, okuri)`を提供する
  - [x] backendのコア機能を試せるCLIを提供する
  - [x] TOML設定ファイルからモデル・辞書・推論設定を読み込む
  - [x] libskkのrule定義とincludeを読むstatelessなローマ字・かな変換API
  - [x] frontend非依存のキーイベント型とlibskk互換keymap loader
  - [x] 直接入力・preedit・候補選択の状態機械をbackendに保持
  - [ ] 後回し: 数値変換、辞書内Lisp式などの特殊変換
  - [ ] 後回し: `complete()`による見出し補完
  - [ ] 後回し: skkserv、CDB辞書
  - [ ] 後回し: system/user辞書の自動reload監視
  - [ ] 後回し: モデルの自動ダウンロード
  - [ ] 後回し: 複数候補のバッチスコアリング最適化
- [ ] frontendの実装
  - [ ] fcitx5固有のキーイベント、preedit、候補UIを実装する
  - [ ] backendの`prepare`を入力中、`lookup`を変換開始時に呼び出す
- [ ] 性能検証 (暇があればね)
  - 適当なテキストを用意して、オリジナル実装とのspace押下回数とか比較するんかな? 知らんけど

## fcitx5 IM prototype

Rust製のログ処理を薄いC++ adapterから呼び出す、入力を素通しするIMです。
UUIDごとにbest-effortな左文脈を保持し、印字可能なキーはappend、Backspaceはpop、
それ以外は安全側でflushとして扱います。fcitx5から有効なsurrounding textを取得
できた場合は、推測値をcursorより左の文脈で置き換えます。別のIMがキーを消費した
場合は生のキーを推測に使わず、そのIMがcommitした確定文字列をappendします。

開発環境は `direnv allow` で読み込めます。通常のCargo成果物は `target/` に保持されるため、
Rust部分は増分ビルドされます。

```console
direnv allow
cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release
cmake --build build
cmake --install build --prefix "$PWD/result"
```

成果物は `result/lib/fcitx5` と `result/share/fcitx5` 以下に配置されます。
試す際は、次のように成果物をfcitx5の探索パスへ追加してからfcitx5を起動し、
設定画面で `SKK Zenz (prototype)` を追加してください。

```console
export XDG_DATA_DIRS="$PWD/result/share${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"
export FCITX_ADDON_DIRS="$PWD/result/lib/fcitx5"
fcitx5 -rd
```

標準エラーには、キー入力をcyanの `[KEY]`、fcitx5から届いた周辺文脈をmagentaの
`[SURROUNDING]`、推測文脈のappend/pop/flush/replaceをgreenの `[GUESSED]` として
記録します（端末以外への出力または `NO_COLOR` 設定時は色なし）。各行の `ts_ms`
はUnix epochからのミリ秒です。ウインドウを切り替えてもUUIDごとの推測値は保持します。

## zenz inference benchmark

`skk-zenz-bench` は zenz v3.2 xsmall/small の Q5_K_M GGUF を Hugging Face から
ダウンロード・キャッシュし、入力中にKV cacheを更新する `speculative` と変換開始時に
初めて更新する `baseline` を比較します。既知の出力はteacher-forcingでloss
（出力トークンの負の対数尤度）を算出します。

```console
cargo run --release -p skk-zenz-bench -- --model both
```

既定の操作列は `crates/zenz-bench/cases.script` にあります。各行を最初の空白で
コマンドと値に分割し、値は残りをそのまま使用します。

```text
context 朝起きて窓を開けました。
input きょう
input きょうはいいてんきです
eval 今日はいい天気です
eval 京は良い転機です
```

`context` はKV cacheを破棄してグループを開始し、`input` は未確定入力全体を置き換え、
`eval` は現在のcontext/inputに対して出力を評価します。同じ形式のファイルを
`--script` で指定でき、`--runs` 回スクリプト全体を再生して各行と全体の統計を表示します。

```console
cargo run --release -p skk-zenz-bench -- \
  --model small \
  --script path/to/cases.script \
  --runs 10
```

CUDA または Vulkan を使う場合は、それぞれ `--features cuda` または
`--features vulkan` を追加します。ローカルの GGUF は `--model-path` で指定できます。

## backend core CLI

`skk-zenz-backend`はモデル、SKK system辞書、SQLite user辞書を読み込み、操作列で
コア機能を確認できます。

```console
cargo run --release -p skk-zenz-backend -- --script path/to/backend.script
```

既定ではcwdの`skk-zenz.toml`を読みます。別の設定ファイルは`--config`で指定できます。
設定内の相対パスは設定ファイルのあるディレクトリを基準に解決します。

```toml
model_path = "models/zenz-v3.2-xsmall-Q5_K_M.gguf"
system_dictionaries = ["data/SKK-JISYO.L"]
user_dictionary = "data/user.sqlite3"
rom_kana_rule = "azik_mod"

context_size = 1024
# threads = 8
gpu_layers = 999
```

rom-kana ruleは、libskkと同様に`$LIBSKK_DATA_PATH/*/rules`、または
`$XDG_CONFIG_HOME/libskk/rules`（通常は`~/.config/libskk/rules`）などから探します。
探索先を限定する場合は`rule_search_paths`を指定できます。

```toml
rule_search_paths = ["/path/to/libskk/rules"]
```

`--script`を省略すると標準入力から操作列を読みます。各行は最初の空白でコマンドと値に
分けます。

```text
context 食事の前に手を合わせて、
input はし
prepare
lookup
select 0
lookup

context 荷物を
input おく
prepare
lookup る
```

- `context`: KV cacheを破棄して左文脈を設定
- `input`: 現在の未確定入力全体を設定
- `prepare`: 現在のcontext/inputまで投機実行
- `lookup [okuri]`: 辞書候補を取得し、zenzでスコアリング
- `select N`: 直前の候補Nを選択し、user辞書とrecencyを更新
- `purge N`: 直前の候補Nをuser辞書から削除
- `register TEXT`: 現在のinput/okuriへ候補を明示登録
- `kana ROMAJI`: ひらがなへ変換
- `katakana ROMAJI`: カタカナへ変換
- `hankaku-katakana ROMAJI`: 半角カタカナへ変換
- `key TEXT BASE MODIFIERS`: frontendと同じ形式のキーイベントを入力
- `focus-out commit|reset`: 未確定入力を確定または破棄して状態をreset
- `reset`: 入力状態を破棄

ローマ字かな変換だけを試す場合は、モデルや辞書をロードせずに実行できます。

```console
cargo run -p skk-zenz-backend -- --romaji kanji
cargo run -p skk-zenz-backend -- --romaji kanji --kana-mode katakana
```

frontendから渡すキーイベントは、キーボードレイアウトとShift適用後の`text`と、
修飾前の論理`key`を分けます。backendはキーボードレイアウトを推測しません。

```rust
KeyEvent {
    text: Some("!".to_owned()),
    key: Key::Character('1'),
    modifiers: Modifiers::SHIFT,
}
```

libskk keymapの修飾なし印字キー（`!`や`X`）は`text`に対して照合し、`S-1`や
`C-q`などの明示的なChordと名前付きキーは`key + modifiers`に対して照合します。
両方に一致する定義がある場合は、明示的なChordを優先します。

preedit状態もbackendが単一で保持します。`Backend::handle_event`へキーやfocus-outを
渡すと、`InputUpdate`として`consumed`、preedit、候補、commit文字列を返します。

```text
context これは
key K k shift
key a a -
key n n -
key a a -
key SPC space -
key - Return -
```

この操作列では`▽かな`から辞書lookupを行い、`▼候補`を経て確定します。
