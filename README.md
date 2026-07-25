# SKK Zenz

SKKを現代的にしたいな!というモチベでPoCを作ってみます

## アピールポイント (独自の機能)

- Zenzaiを利用
  - 左文脈を活用した変換候補のreranking (・作成?) に使用
  - fcitx5のsurrounding textの実装状況がまだまだ不十分なので、非対応アプリでは直前の情報を記憶しておきfallbackとしています
- 送りがなの母音の活用
  - libskkでは参照されない、送りがなの母音情報を使って変換候補をフィルタする
  - system辞書は無改造のまま使えるようにする。user辞書には、recent usedとして母音つきのエントリを記録する
- user辞書
  - sqliteを使用します
  - 毎回並び換え&全文保存を避けるようにしています
- エントリのソートは、(from_user?: bool, ai_rank: float)をkeyとするイメージ
  - zenzとuser recencyの使い分けが使いやすさに直結するはずなので、要調整。

## roadmap (todo)

優先度順:

- abbrモード
- モデルの自動ダウンロード
- 最適化
- nixでビルドしてhome-managerから呼べるようにする
- `complete()`による見出し補完
- 数値変換、辞書内Lisp式などの特殊変換
- skkserv、CDB辞書
- system/user辞書の自動reload監視(?)
- 性能検証
  適当なテキストを用意して、オリジナル実装とのspace押下回数とか比較するんかな? 知らんけど

## 構成

### fcitx5 frontend

薄いC++ adapterがfcitx5とcommunicateし、surrounding textとともにbackendに投げて結果をもらって表示します

### backend

libskkに相当します。候補を辞書ファイルから生成し、結果を返します
configやtable、辞書の参照はすべてbackendがやります

### zenz inference benchmark

zenzの評価速度とか精度を調べるベンチマークです
操作列をscriptとして入力します

```text
context 朝起きて窓を開けました。
input きょう
input きょうはいいてんきです
eval 今日はいい天気です
eval 京は良い転機です
```

baseline/speculativeでkv cacheまわりの最適化の効果を確認する。

### backend core CLI

backendのコア機能を確認する最低限のcli

## 設定ファイル

`$XDG_CONFIG_HOME/skk-zenz/config.toml`
（通常は`~/.config/skk-zenz/config.toml`）を読みます。

```toml
model_path = "models/zenz-v3.2-xsmall-Q5_K_M.gguf"
rom_kana_rule = "azik_mod"

context_size = 1024
# threads = 8
gpu_layers = 999
```

ユーザ辞書は同じディレクトリの`user.sqlite3`
system辞書は`~/.config/skk-zenz/dictionary_list`へ1行1ファイルで指定 相対パスも使えます

このファイルがない場合は `$XDG_DATA_HOME/fcitx5/skk/dictionary_list`を代わりに使用します

rom-kana ruleは、`~/.config/skk-zenz/rules`のほか、`~/.config/libskk/rules`とコンパイル時の`LIBSKK_DATA_PATH`も探索対象とします
(コンパイル時に`LIBSKK_DATA_PATH`がなければ`/usr/share/libskk/rules`を使用します)

## build

TODO: nixで成果物をつくる

### development

開発環境は`nix-shell`またはdirenvで読み込めます。
`build.sh`が`result/`へ成果物を生成します。
`start.sh`が、fcitx5の探索パスへ`./result/`を追加してfcitx5を起動します (`-r`でreplace running instance)
設定画面で `SKK Zenz` を追加してください。

```console
direnv allow
./build.sh
./start.sh
```

backendを初期化できない場合や入力処理に失敗した場合は標準エラーへ原因を記録し、
キー入力はアプリケーションへそのまま渡します。
