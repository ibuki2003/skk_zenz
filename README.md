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
- [ ] zenzの組み込み、ランタイムとハードウェアごとのレイテンシを確認
  - space押下前から投機実行できるといいのですが…
- [ ] backend, frontendの実装
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
