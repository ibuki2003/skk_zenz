# 設定ファイル

## config.toml

### `model_path`, `huggingface`

使用するモデルを指定します
huggingface上のモデルか、model_pathでggufファイルを直接指定できる
デフォルト値は huggingface=`Miwa-Keita/zenz-v3.2-small-gguf/ggml-model-Q5_K_M.gguf`
xsmallに切り替えることができる

### `rom_kana_rule`

keybindとrom-kana tableを参照するrule名を指定
libskkのものも参照できます

### `recency_half_life_days`

候補の使用頻度は時間とともに指数減衰し、その半減期を日数で指定します

### `candidate_order`

`okuri_ari` `okuri_nasi` `abbrev` `prefix_suffix` のそれぞれで、候補の並べ替え順を指定できます。
`recency`、`user_first_ai_score`、`recency_then_ai_score`、`ai_score`、`mixture`のいずれかを指定できます。
詳しくは [reranking.md](./reranking.md) を参照

### `candidate_order.mixture_lambda`

候補順は`candidate_order`の各項目へ
`mixture`は候補集合内でAIスコアと使用頻度を確率へ正規化し、使用頻度をentropy confidenceで平滑化してから、両者の対数確率を`mixture_lambda`で合成します。
0.0でfrequency only, 1.0でai only

### `context_size`, `threads`, `gpu_layers`

LlamaによるZenzの評価の設定をします
スペックと相談して決めて
context_sizeは256あればほぼ足ります 左文脈の長さ制限になることもある

### `conversion_history`

開発者がコーパスデータ収集に使用。変換の履歴をcsvに保存します

### `frontend`

フロントエンドのための設定をここに書きます

#### `frontend.fcitx5`

- `page_size`: 候補表示 ページあたり件数
- `candidate_window_threshold`: 最初n件は候補をリストにしない
- `show_annotations`: 辞書のannotationを表示するかどうか
- `candidate_key`: 候補表示を選択する方法。`digit`: 数字キーを使用

## system辞書など

libskk実装と互換性を持たせています

system辞書は`~/.config/skk-zenz/dictionary_list`へ1行1ファイルで指定 相対パスも使えます

このファイルがない場合は `$XDG_DATA_HOME/fcitx5/skk/dictionary_list`を代わりに使用します

rom-kana ruleは、`~/.config/skk-zenz/rules`のほか、`~/.config/libskk/rules`とコンパイル時の`SKK_RULES_PATH`も探索対象とします
(コンパイル時に`SKK_RULES_PATH`がなければ`/usr/share/libskk/rules`を使用します)

huggingface = "Miwa-Keita/zenz-v3.2-small-gguf/ggml-model-Q5_K_M.gguf"

rom_kana_rule = "default"

[candidate_order]
okuri_ari = "user_first_ai_score"
okuri_nasi = "recency"
abbrev = "recency"
prefix_suffix = "recency"
mixture_lambda = 0.5

[frontend.fcitx5]
page_size = 8
candidate_window_threshold = 3
show_annotations = true
candidate_key = "digit"
