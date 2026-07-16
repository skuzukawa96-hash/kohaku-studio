# アーキテクチャ

Kohaku Studio の内部構造と設計方針をまとめます。

## 全体像

```
ブラウザUI (vanilla JS + Canvas)
    │  JSON API (Command)  ← すべて POST /api/*
    ▼
┌─────────────────────────────────────────────┐
│ bi-app        HTTPサーバー(tiny_http)          │
│               クエリエンジン(SQLite in-memory) │
│               分析API                          │
├─────────────────────────────────────────────┤
│ bi-analytics  記述統計 / 相関 / OLS回帰 /       │
│               k-means++（純Rust・依存なし）     │
├─────────────────────────────────────────────┤
│ bi-connectors CSV / Excel(calamine) / SQLite   │
│               コネクタ + レジストリ             │
├─────────────────────────────────────────────┤
│ bi-core       TableData / DataType /           │
│               Connector trait / Project モデル  │
└─────────────────────────────────────────────┘
```

データは次のように流れます。

```
CSV / Excel / SQLite
    │  Connector::load
    ▼
TableData（正規化済みの内部表形式）
    │  Engine::register
    ▼
SQLite in-memory テーブル
    │  SQL（ユーザー入力 / チャート生成クエリ / 分析クエリ）
    ▼
QueryResult
    ├─▶ テーブル表示 / チャート描画（UI）
    └─▶ bi-analytics（回帰・クラスタリング・統計）
```

## クレートの責務

| クレート | 責務 | 主な依存 |
| --- | --- | --- |
| `bi-core` | 内部データモデル（`TableData` / `DataType` / `Value`）、`Connector` trait、`Project` モデル、型推定・列名正規化などの共通ユーティリティ | serde |
| `bi-connectors` | 各データソースを `TableData` へ正規化するコネクタ群と、拡張子からコネクタを解決する `ConnectorRegistry` | csv, calamine, rusqlite, encoding_rs |
| `bi-analytics` | 記述統計・ピアソン相関・OLS回帰・k-means++。外部依存なしの純Rust実装 | serde |
| `bi-app` | ローカルHTTPサーバー、SQLite in-memory クエリエンジン、分析API、内蔵UI（HTML/CSS/JSをバイナリに埋め込み） | tiny_http, rusqlite, serde_json |

## 設計原則

1. **UIにデータ処理を書かない** — UIはCommand（JSON API）を投げるだけで、データ処理はすべてRust側で行う。
2. **すべての入力はDatasetに正規化** — CSV / Excel / DB を別々に扱わず、内部表現 `TableData` に統一する。UI・クエリエンジンはデータの出所を意識しない。
3. **可視化はChartSpecとして保存** — 描画処理ではなくグラフ定義（JSON）を保存する。
4. **状態はProjectに集約** — ユーザーの作業状態を `.kohaku` プロジェクトファイルにまとめる。
5. **ソース固有処理はコネクタ内に閉じ込める** — Excelのシート名・セル型・日付シリアル値などは Excel コネクタの内側で完結させ、Coreやエンジンに漏らさない。

## 主要な型（bi-core）

```rust
pub enum DataType { Null, Boolean, Int64, Float64, Utf8 }

pub enum Value { Null, Bool(bool), Int(i64), Float(f64), Text(String) }

pub struct TableData {
    pub schema: TableSchema,
    pub rows: Vec<Vec<Value>>,
}

pub trait Connector: Send + Sync {
    fn connector_type(&self) -> &'static str;
    fn extensions(&self) -> &'static [&'static str];
    fn list_objects(&self, path: &Path) -> BiResult<Vec<String>>;
    fn load(&self, path: &Path, object: &str, opts: &ImportOptions) -> BiResult<TableData>;
}
```

## クエリエンジン

SQLite の in-memory データベースを採用しています。各データセットはテーブルとして登録され、
ソースの異なるデータ同士もSQLでJOINできます。低スペックPC向けに、以下のPRAGMAで
速度と省メモリのバランスを取っています。

- `temp_store = MEMORY` — GROUP BY等のソート用一時領域をメモリに
- `cache_size = -16000` — ページキャッシュを約16MBに制限
- `threads = 4` — ソートをワーカースレッドで並列化

## セキュリティ（ローカルCSRF対策）

APIエンドポイント（`/api/*`）には次のガードがあります。

1. POSTメソッドのみ許可（それ以外は 405）
2. `Content-Type: application/json` を必須（それ以外は 415）。プリフライトを回避できる
   単純リクエスト（`text/plain` 等）を排除する。
3. ブラウザ由来（`Origin` ヘッダあり）のリクエストは `localhost` / `127.0.0.1` のみ許可（それ以外は 403）。
   curl等の非ブラウザクライアントは `Origin` を送らないため影響しない。

これにより、ツール起動中に別のWebページをブラウザで開いても、そのページからローカルAPIを
勝手に叩かれる（ファイル書き込み・SQL実行など）ことを防ぎます。

## 拡張ポイント

将来のプラグイン機構(Connector / Transform / Chart)の設計は
[plugin-api-draft.md](plugin-api-draft.md) を参照してください。以下は現在の(再ビルドを伴う)拡張方法です。

### 新しいデータソース

`bi_core::Connector` trait を実装し、`bi-connectors/src/lib.rs` の
`ConnectorRegistry::new()` に登録するだけです。UI・エンジン側の変更は不要です。

### 新しいチャートタイプ

`bi-app/ui/app.js` の `buildChartQuery()`（データ取得SQLの組み立て）と
`renderChart()`（Canvas描画）に分岐を追加します。

## 設計方針からの主な変更点（軽量化）

低スペックPCでの動作を最優先に、当初の設計方針から次の点を変更しています。

| 設計方針 | 実装 | 理由 |
| --- | --- | --- |
| Tauri 2 + React | 内蔵HTTPサーバー + ブラウザ | WebView2 / Node への依存を排除し、単一バイナリ（数MB）に |
| DataFusion + Arrow | SQLite in-memory | 省メモリ・起動即応。SQL集計機能は同等にカバー |
| ECharts等のJSライブラリ | 自前Canvasレンダラ | オフライン完結、依存ゼロ |
