# Plugin API 設計ドラフト(v0.3)

Kohaku Studio に Connector / Transform / Chart の3種類のプラグイン機構を導入するための設計ドラフトです。

> **実装状況**: Phase 1（Connector プラグイン）は実装済みです。
> 使い方は [examples/plugins/README.md](../examples/plugins/README.md) を参照してください。
> Phase 2（Chart）は `registerChartType` として本体内で先行実装済み（ウェハーマップ / SPC管理図）。
> Phase 3（Transform / Parquet受け渡し）は未着手です。

## 1. 目的

Kohaku Studio 本体を軽量に保ったまま、ユーザーが自分の用途に合わせて機能を追加できるようにする。

| 種類 | 拡張できること | ユースケース例 |
| --- | --- | --- |
| Connector | 新しいデータソースの取り込み | 装置ログの独自フォーマット、社内API、JSON/JSONL |
| Transform | 取り込み後のデータ加工 | 列の正規化、匿名化、単位変換、横持ち→縦持ち |
| Chart | 新しいチャートタイプ | ウェハーマップ、SPC管理図、ガントチャート |

### 非目標(このドラフトで扱わないこと)

- プラグインのオンライン配布・マーケットプレイス(オフライン完結の方針を維持)
- プラグインのサンドボックス化による「信頼できないコードの安全な実行」(後述のセキュリティモデル参照)
- 分析(bi-analytics)のプラグイン化(純Rust・依存なしの方針を維持し、本体に実装する)

## 2. 前提となる制約

本体の設計目標([architecture.md](architecture.md))をプラグイン機構にもそのまま適用する:

1. **単一バイナリ・外部ランタイム不可** — Node / WebView / .NET を本体の動作要件にしない
2. **低スペックPCで軽快** — プラグインを使わないユーザーに一切のコストを課さない(ゼロコスト原則)
3. **オフライン完結** — プラグインの発見・読み込みにネットワークを使わない
4. **既存の設計原則を壊さない** — 特に「すべての入力は TableData に正規化」(Rule 2)と
   「ソース固有処理はコネクタ内に閉じ込める」(Rule 5)

## 3. 実行モデルの比較

プラグイン機構の最重要決定は「プラグインのコードを**どこで・どうやって**実行するか」。

| 方式 | 概要 | 利点 | 欠点 | 評価 |
| --- | --- | --- | --- | --- |
| A. 静的リンク | trait実装を追加して再ビルド(現状の拡張ポイント) | 最軽量・型安全 | フォークとRust環境が必須。「プラグイン」ではない | 開発者向けとして現状維持 |
| B. 動的ライブラリ | dll/so を実行時にロード | ネイティブ速度 | RustはABI不安定でコンパイラバージョン一致が必須。unsafe必須、プラグインのクラッシュが本体を巻き込む、配布が困難 | **不採用** |
| C. 外部プロセス | 実行ファイルを子プロセスとして起動し、stdin/stdout でJSONをやりとり | 言語自由(Python等)、クラッシュ隔離、追加依存ゼロ(serde_json既存)、git/kubectl等で実績ある方式 | プロセス起動コスト(数十ms)、大きな表のシリアライズコスト(→5.4で対策) | **Connector / Transform に採用** |
| D. WASM | wasmtime等でサンドボックス実行 | 安全性が最も高い、言語もある程度自由 | ランタイムがバイナリ+数十MBと重く、低スペック方針に反する。ホスト関数設計も大がかり | 不採用(将来、状況が変われば再検討) |
| E. 宣言的(コード実行なし) | マニフェスト+SQLテンプレート等の「定義」だけを読む | 最も安全・軽量 | 表現力が限定的。任意のパースや描画は書けない | Chart の骨格に採用(下記) |

**Chart だけは事情が異なる。** 描画はブラウザ側(Canvas)で行われるため、外部プロセス方式が使えない。
チャートプラグインは「マニフェスト(E)+ ローカルJSファイル(サーバーが配信し、UIが登録する)」とする。
JSの動的読み込みは「JSライブラリ追加不可」の方針と紛らわしいが、あれは**本体の依存**を増やさない
という意味であり、ユーザーが自分の意思で置いたローカルコードの読み込みとは区別する。

## 4. 全体像(推奨案)

```
%LOCALAPPDATA%\kohaku-studio\plugins\
  └─ <plugin-name>\
      ├─ plugin.json        ← マニフェスト(必須)
      ├─ main.exe / main.py ← Connector/Transform: 実行ファイル(kind による)
      └─ chart.js           ← Chart: 描画モジュール(kind による)

起動時:
  bi-app が plugins/ を走査 → plugin.json を読み、種類ごとに登録
    Connector → PluginConnector(Connector trait 実装)として ConnectorRegistry に登録
    Transform → インポートウィザードの「変換」候補に追加
    Chart     → /plugins/<name>/chart.js を配信し、UIが起動時に registerChartType()
```

- プラグインは**既定で無効**。起動フラグ `--enable-plugins` を付けたときだけ読み込む(セキュリティ参照)
- プラグインを1つも置いていなければ、走査はディレクトリ存在チェック1回で終わる(ゼロコスト原則)

### plugin.json(マニフェスト)

```json
{
  "api_version": 1,
  "name": "jsonl-connector",
  "version": "0.1.0",
  "kind": "connector",
  "description": "JSON Lines ファイルを読み込む",
  "entry": ["python", "main.py"],
  "extensions": ["jsonl", "ndjson"],
  "schemes": []
}
```

- `api_version`: プロトコルの版。本体が対応しない版は**読み込みを拒否して警告**(黙って無視しない)
- `entry`: コマンドと引数の配列。Windows でのシェバン非対応を考慮し、インタプリタを明示できる形にする
- `kind: "chart"` の場合は `entry` の代わりに `"chart_js": "chart.js"` と `"chart_type": "wafermap"` を持つ

## 5. Connector / Transform プロトコル

### 5.1 通信方式

1リクエスト=1プロセス起動とする(常駐させない)。リクエストはstdinへJSON 1行、レスポンスはstdoutのJSON 1行。
ログ・デバッグ出力はstderrへ(本体がそのままコンソールに中継する)。

**入出力のエンコーディングはUTF-8固定**。Windowsでは処理系の標準出力が既定でロケール依存
(日本語環境ではcp932)になることがあるため、プラグイン側で明示的にUTF-8を指定すること
(非UTF-8の応答は本体がその旨を伝えるエラーにする)。

常駐方式(1プロセスでリクエストを繰り返し処理)はプロセス管理(死活監視・再起動・終了)が複雑になるため
v1では見送る。起動コスト数十msは、インポートという操作の頻度なら許容できる。

### 5.2 コマンド(Connector)

既存の `Connector` trait をそのままJSONに写像する(Rule 5 を守る自然な境界)。

```
→ {"cmd": "describe", "api_version": 1}
← {"ok": true, "name": "jsonl-connector", "extensions": ["jsonl"], "schemes": []}

→ {"cmd": "list_objects", "path": "C:/data/logs.jsonl"}
← {"ok": true, "objects": ["logs"]}

→ {"cmd": "load", "path": "C:/data/logs.jsonl", "object": "logs",
   "options": {"header_row": 1, "skip_rows": 0, "max_rows": 2000000}}
← {"ok": true, "table": {  ...TableData(5.4参照)...  }}

エラー時(共通):
← {"ok": false, "error": "3行目のJSONが不正です"}
```

本体側は `PluginConnector` が trait とプロトコルの橋渡しをするだけで、レジストリ以降の既存コードは
一切変更しない(拡張子・スキームによる解決にそのまま参加する)。

### 5.3 コマンド(Transform)

```
→ {"cmd": "transform", "table": { ...TableData... }, "params": {"target_col": "name"}}
← {"ok": true, "table": { ...TableData... }}
```

適用タイミングはインポート直後(TableData → TableData)。UIはインポートウィザードに
「変換を適用」選択を追加する。SQLで書ける加工はSQLでやればよいので、
Transform は「SQLでは書きにくい加工」(正規表現置換、名寄せ、匿名化など)が主対象。

### 5.4 TableData の受け渡し形式

**小さい表はインラインJSON、大きい表はParquetファイル参照**のハイブリッドとする。

```json
// インライン(行数 <= 100,000 の目安)
{"columns": [{"name": "id", "type": "integer"}, ...], "rows": [[1, "a"], ...]}

// ファイル参照(大きい表)
{"parquet": "C:/Users/.../Temp/kohaku-plugin-xxxx.parquet"}
```

- 型名は既存の `DataType::name()`("integer" / "real" / "text" / "boolean" / "null")と一致させる
- Parquet の読み書きは v0.3 の Parquetキャッシュ(`parquet_cache.rs`)の変換コードをそのまま再利用できる
- Python 側は pandas / pyarrow で自然に読み書きできる(プラグイン作者の負担が小さい)
- v1 実装はインラインJSONのみでもよい(Parquet参照は `api_version` を上げずに追加できる後方互換拡張)

### 5.5 実行管理

- タイムアウト: `describe` 5秒 / `list_objects` 30秒 / `load`・`transform` 10分(インポートの体感に合わせる)。超過時はプロセスをkillしてエラー表示
- 終了コード非0・不正JSON・タイムアウトはすべて「プラグイン名+stderr要約」付きのエラーメッセージにしてUIへ(ユーザーを止めない、原因が追える)
- 本体のインポート上限(200万行)はプラグイン経由でも同じ場所(bi-app)で適用する

## 6. Chart プラグイン

### 6.1 形式

チャートプラグインはJSモジュール1ファイル。本体UIの2つの拡張点(`buildChartQuery` / `renderChart`)を
そのまま関数として書く。

```js
// chart.js
kohaku.registerChartType({
  type: "wafermap",            // ChartSpec.chart_type に入る識別子
  label: "ウェハーマップ",      // チャート種別セレクトの表示名
  // spec からデータ取得SQLを組み立てる(既存 buildChartQuery の分岐に相当)
  buildQuery(spec, filteredBase) {
    return `SELECT x AS x, y AS y, value AS v FROM (${filteredBase})`;
  },
  // Canvas 2D コンテキストに描画する(既存 renderChart の分岐に相当)
  render(ctx, w, h, spec, result, helpers) { /* ... */ },
});
```

- `helpers` に `niceTicks` / `fmtTick` / `CHART_COLORS` 等の既存ユーティリティを渡し、見た目の統一を保つ
- **v0.4 実装で得た知見**: SQLだけでは表現できないチャート(SPC管理図の管理限界・ネルソンルール判定など)のため、`buildQuery` の代わりに非同期の `fetch(spec, base)` フック(本体の分析APIを呼んで結果オブジェクトを返す)を許可する。統計処理をUI側に書かない(設計Rule 1)ための仕組みで、SPC管理図が最初の利用者
- **v0.5 実装で得た知見(ファセット対応)**: `form` に `facet: true` を宣言すると、本体がファセット分割
  (spec.facet の列値ごとのパネル並列表示)を引き受ける。`facetMax` でパネル数の上限を指定できる
  (既定12。ウェハーマップは25=1ロット分)。ファセット時、`render` は7番目の引数
  `shared = { facetValue, allRows }` を受け取る — `allRows` は全パネル分の行で、
  スケールをパネル間で共有するための基準(ウェハーマップは座標範囲とカラースケールをここから計算)。
  `buildQuery` はファセット列を `f` として SELECT に含める(本体の分割処理は `f` 列を見る)
- **`fetch` フックを使うチャートのファセット**: `buildQuery` を使うチャートは本体が結果の `f` 列で分割するが、
  `fetch` チャートは結果が行の集合ではないため分割できない。この場合は `fetch` 自身が
  グループ別実行API(`/api/analyze/group`)を呼び、`{group, groups: [{value, result|error}], truncated, total, shown}`
  を返す。本体は結果に `groups` 配列があればそれをパネル列として描く(SPC管理図が最初の利用者)。
  パネル間で共有するスケールは `shared.allResults`(成功したグループの結果の配列)から render 側が求める。
  `error` を持つグループはそのパネルにだけエラー文言を描く(1グループの失敗で全体を消さない)
- **凡例(カラースケール等)は `renderLegend(ctx, w, h, spec, result, helpers)` + `legendWidth` で宣言する**。
  本体が格子の外側(最上段の右端パネルの右隣)に領域を確保して1回だけ呼ぶ。
  パネル内に描くと、そのパネルだけ余白が変わってマップの大きさと位置が揃わなくなる
  (v0.5 で実際に起きた。`render` 側で「代表パネルにだけ描く」方式は不可)
- サーバーは `--enable-plugins` 時のみ `/plugins/<name>/chart.js` を配信し、`index.html` 読み込み後にUIが順次ロードする
- ChartSpec(保存形式)は変更不要。`chart_type` にプラグインの識別子が入るだけ(Rule 3 を維持)
- プラグイン未導入環境でそのプロジェクトを開いた場合は「未対応のチャートタイプ(プラグイン `wafermap` が必要)」と表示する(壊さない・原因を示す)

### 6.2 制約

- 描画はCanvas 2Dのみ(DOM操作・外部リソース読み込みはしない規約とする。技術的強制はv1ではしない)
- ダッシュボード・PNG/HTMLエクスポートは Canvas を経由するため、プラグインチャートも追加対応なしで動く

## 7. セキュリティモデル

**プラグイン=ローカルでの任意コード実行**である。これを隠さず、明示的な同意で有効化する。

1. 既定は無効。`--enable-plugins` を付けたときのみ読み込む
2. 起動時に読み込んだプラグインの一覧(名前・種類・パス)をコンソールに表示する
3. ドキュメントに「信頼できる作者のプラグインだけを置くこと。プラグインはあなたの権限で動く」と明記
4. プラグインディレクトリは自分のユーザープロファイル配下のみ(他所からの自動読み込みはしない)

サンドボックスを提供しない理由: WASM等による隔離は本体の軽量方針と両立しない(3章)。
「ローカルツールがローカルユーザーの明示的な指示でローカルのコードを動かす」のはエディタのプラグインや
シェルスクリプトと同じ信頼モデルであり、それを超える保証はv1では約束しない。

## 8. 段階的実装計画

| フェーズ | 内容 | 状況 |
| --- | --- | --- |
| Phase 1 | マニフェスト読み込み+Connector プラグイン(インラインJSONのみ)+サンプル(Python製JSONLコネクタ) | **実装済み** |
| Phase 2 | Chart プラグイン(registerChartType)+サンプル | API は本体内で実装済み(ウェハーマップ / SPC管理図)。外部ファイルからの読み込みは未実装 |
| Phase 3 | Transform プラグイン+Parquet受け渡し | 未着手 |

**論点**: v0.4 のウェハーマップ・SPC管理図を「最初のチャートプラグイン」として実装するか(ドッグフーディング)、
本体組み込みにするか。プラグインとして作ればAPIの実用性が検証できるが、半導体向け機能を使う
ユーザー全員に `--enable-plugins` を要求することになる。→ 本体組み込みにしつつ、内部実装を
`registerChartType` と同じ形にする(=APIの検証だけ行う)折衷案を推奨。

## 9. 論点と決定

1. ✅ **有効化の方式** → **起動フラグ `--enable-plugins`**(既定は無効)。
   診断用に `--list-plugins`(実際にプラグインを起動して疎通を確認する)も用意した
2. ✅ **Python前提の是非** → **サンプルはPythonで用意する**。本体はPythonに依存せず、
   サンプルを使わなければ不要。他言語でも標準入出力でJSONを扱えれば同じように書ける
3. ✅ **プラグインの置き場所** → **`%LOCALAPPDATA%\kohaku-studio\plugins\`**
   (Parquetキャッシュと同居。必ず書き込み可能)。`KOHAKU_PLUGIN_DIR` で変更可
4. ⏳ **Transform の適用タイミング**: インポート時のみ(推奨)か、SQL実行後にも適用できるようにするか
   → Phase 3 で判断する
5. ⏳ **api_version の運用**: 破壊的変更時にどこまで旧版を並行サポートするか
   → 現在は完全一致のみ受け付け、違う版は理由を表示して読み込まない

### Phase 1 の実装で分かったこと

- **入出力はUTF-8必須**と明記が要る。Windowsでは Python の標準出力が既定でロケール依存
  (cp932など)になり、日本語を含む応答が壊れる。実装では非UTF-8を専用のエラーメッセージで
  知らせ、サンプルは `sys.stdout.buffer` へUTF-8で書き出している
- **型宣言は「値を解釈するヒント」**として扱い、最終的な列型は実際の値から決めるのが安全
  (宣言と値がずれていても壊れない。他のコネクタと同じ挙動)
- タイムアウト時は子プロセスを確実に kill する必要がある(孤児プロセスを残さない)

## 10. 参考: 既存コードとの対応

| プラグイン概念 | 既存コード |
| --- | --- |
| Connector プロトコル | `bi_core::Connector` trait([bi-core/src/lib.rs](../crates/bi-core/src/lib.rs)) |
| レジストリ登録 | `ConnectorRegistry::new()`([bi-connectors/src/lib.rs](../crates/bi-connectors/src/lib.rs)) |
| TableData JSON | `DataType::name()` / 分析APIの `table_json()`(bi-app/src/server.rs) |
| Parquet 受け渡し | `parquet_cache.rs` の TableData ⇄ Parquet 変換 |
| Chart 拡張点 | `buildChartQuery()` / `renderChart()`(bi-app/ui/app.js) |
