# SKK Zenz

SKKを現代的にしたいな!というモチベでPoCを作ってみます

https://github.com/user-attachments/assets/b3436241-f0a0-4302-b3a5-362a261e35b7

## index

README以外のドキュメントもあります

- [reranking.md](./docs/reranking.md) ソート順序
- [data-flow.md](./docs/data-flow.md) モジュール概念図
- [configuration.md](./docs/configuration.md) 設定ファイル

## アピールポイント (独自の機能)

- Zenzaiを利用
  - 左文脈を活用した変換候補のreranking (・作成?) に使用
  - fcitx5のsurrounding textの実装状況がまだまだ不十分なので、非対応アプリでは直前の情報を記憶しておきfallbackとしています
    - fallbackでは、ウインドウ切り替えは当然として、Escや矢印キーなども、追うことなくcontext flushとして、安全側に倒す実装になっています
- 送りがなの母音の活用
  - libskkでは参照されない、送りがなの母音情報を使って変換候補をフィルタする
  - system辞書は無改造のまま使えるようにする。user辞書には、recent usedとして母音つきのエントリを記録する
- user辞書
  - sqliteを使用します
  - 毎回並び換え&全文保存を避けるようにしています

### ソート順序

frequency情報とZenzによる評価値を組み合わせるハイブリッド スコアを用いて、候補をソートしている。

作者による実験では、MRR (Mean Reciprocal Rank)は、baseline(original SKK)で0.952であったものが、最大0.97まで上昇する。

詳しくは [./docs/reranking.md] を参照してください。

### 速度

NNを動かすので、どうしても時間がかかります

configのthreadsでZenz評価のスレッド数を指定できます。
CPU評価のほか、Vulkanを使用してGPUやNPUで実行することもできます。(下のほう参照。)

作者環境では、候補**1文字**あたり、
- デスクトップPC:
  - CPU: ~100ms (cpu time)
  - GPU: ~6ms
- ノートPC:
  - CPU: ~180ms
  - GPU: ~9ms

CPUだとスレッド数が結構効きます
マシン性能や消費電力と相談して設定してください。

## roadmap (todo)

- 辞書内Lisp式などの特殊変換（数値変換は実装済み）
- skkserv、CDB辞書
- system/user辞書の自動reload監視(?)

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

ユーザ辞書は同じディレクトリの`user.sqlite3`です

[サンプル](./config.sample.toml)

項目詳細: [configuration.md](./docs/configuration.md)

## build

Nix packageは`default.nix`で定義しています。

```nix
skk-zenz = pkgs.callPackage ./default.nix {};
skk-zenz-vulkan = skk-zenz.override { useVulkan = true; };
```

flakeからは、Vulkanを使用しないデフォルトのpackageをビルドできます。
`flake.lock`は利用側でnixpkgsを固定できるよう、リポジトリには含めません。

```console
nix build
```

### development

開発環境は`nix-shell`またはdirenvで読み込めます。
`build.sh`が`result/`へ成果物を生成します。
Vulkan backendを使用する場合は、`build.sh -DSKK_ZENZ_VULKAN=ON`で有効化できます。
この設定はCMakeのbuild directoryに保存され、`OFF`を指定すると無効化できます。
`start.sh`が、fcitx5の探索パスへ`./result/`を追加してfcitx5を起動します (`-r`でreplace running instance)
設定画面で `SKK Zenz` を追加してください。

```console
direnv allow
./build.sh
./start.sh
```

Vulkan backendを有効にしてビルドする場合:

```console
./build.sh -DSKK_ZENZ_VULKAN=ON
```

backendを初期化できない場合や入力処理に失敗した場合は標準エラーへ原因を記録し、
キー入力はアプリケーションへそのまま渡します。
