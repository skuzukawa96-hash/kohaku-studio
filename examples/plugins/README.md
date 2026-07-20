# プラグインのサンプル

Kohaku Studio のコネクタプラグイン（Plugin API Phase 1）のサンプルです。
仕様は [docs/plugin-api-draft.md](../../docs/plugin-api-draft.md) を参照してください。

> ⚠️ **プラグインはあなたの権限で動く任意のプログラムです。**
> 信頼できる作者のものだけを置いてください。既定では無効で、
> `--enable-plugins` を付けて起動したときだけ読み込まれます。

## 使ってみる

1. プラグインディレクトリへコピーします（Windows の例）:

   ```powershell
   $dst = "$env:LOCALAPPDATA\kohaku-studio\plugins\jsonl-connector"
   New-Item -ItemType Directory -Force $dst
   Copy-Item examples\plugins\jsonl-connector\* $dst
   ```

2. 認識されているか確認します（プラグインを実際に起動して応答を確かめます）:

   ```powershell
   .\target\release\kohaku-studio.exe --list-plugins
   ```

   ```
   プラグインディレクトリ: C:\Users\<user>\AppData\Local\kohaku-studio\plugins
     jsonl-connector [.jsonl .ndjson] ... OK
   ```

3. プラグインを有効にして起動します:

   ```powershell
   .\target\release\kohaku-studio.exe --enable-plugins
   ```

4. 「＋ インポート」から `.jsonl` ファイルを選ぶと、通常のCSVなどと同じように
   取り込めます。取り込んだ後は SQL・チャート・分析すべてで同じように使えます。

試すデータがなければ、次のようなファイルを作ってください（`test.jsonl`）:

```
{"id": 1, "name": "コーヒー豆", "price": 1480, "stock": true}
{"id": 2, "name": "ドリッパー", "price": 2200, "stock": false}
{"id": 3, "name": "マグカップ", "price": 980}
```

## プラグインの作り方（要点）

- ディレクトリに `plugin.json`（マニフェスト）と実行するプログラムを置きます
- 1リクエストごとにプロセスが起動され、**標準入力にJSONが1行**届きます。
  **標準出力にJSONを1行**返してください
- 失敗は `{"ok": false, "error": "理由"}` で返します。例外で落ちてもエラーとして
  扱われますが、理由を返したほうがユーザーに親切です
- 進捗・デバッグ出力は標準エラーへ。本体がエラーメッセージの末尾に含めます

対応コマンド:

| cmd | 用途 | 返すもの |
| --- | --- | --- |
| `describe` | 動作確認（`--list-plugins`） | `name` / `extensions` / `schemes` |
| `list_objects` | 取り込み対象の一覧（Excelのシート、DBのテーブル相当） | `objects`（文字列の配列） |
| `load` | データ本体の読み込み | `table`（`columns` と `rows`） |

`table` の形:

```json
{
  "columns": [{"name": "id", "type": "integer"}, {"name": "name", "type": "text"}],
  "rows": [[1, "コーヒー豆"], [2, "ドリッパー"]]
}
```

`type` は `integer` / `real` / `text` / `boolean` / `null` のいずれかです。
これは値を解釈するときのヒントで、最終的な列の型は実際の値から決まります
（宣言と値がずれていても壊れません）。

Python 以外の言語でも、標準入出力でJSONをやりとりできれば同じように書けます
（`plugin.json` の `entry` に実行コマンドを書くだけです）。
