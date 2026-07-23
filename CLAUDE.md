# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

Kohaku Studio は Rust 製のローカルファーストBIツール。単一バイナリがローカルHTTPサーバー（既定ポート 5590、`127.0.0.1` のみ）を起動し、ブラウザでUIを開く。**低スペックPCで軽快に動くことが最優先の設計目標**であり、外部ランタイム（Node / WebView / .NET）の追加は不可。重い依存も原則避けるが、v0.3以降は必要な機能（Parquet対応の `arrow` 等）に限り、実行時の軽さを損なわない形で採用してよい（オーナー決定済み）。

## コマンド

```bash
cargo build --release              # ビルド（バイナリ: target/release/kohaku-studio）
cargo test --all                   # 全テスト
cargo test -p bi-analytics         # クレート単位のテスト
cargo test -p bi-app test_limit_truncation   # 単一テスト（名前の部分一致）

# CIと同じチェック（PRの前に必ず通すこと。clippyは警告=エラー）
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings

# 実DBが必要なコネクタテスト（環境変数が無ければ自動スキップされる）
# 例: KOHAKU_TEST_PG_URL=postgres://kohaku:kohaku@localhost:5432/demo
#     KOHAKU_TEST_MYSQL_URL=mysql://kohaku:kohaku@localhost:3306/demo
cargo test -p bi-connectors test_postgres_live
cargo test -p bi-connectors test_mysql_live
```

実行:

```bash
./target/release/kohaku-studio --make-samples ./samples   # サンプルデータ生成（CSV + SQLite）
./target/release/kohaku-studio --no-browser --port 8080   # 起動（ブラウザを開かず、ポート指定）
```

PostgreSQL / MySQL コネクタの動作確認用DB（使い捨て）:

```bash
docker compose up -d               # 両方起動（接続情報は docs/DEMO.md）
docker compose down -v             # 停止してデータも削除
```

## アーキテクチャ

Cargo workspace の4クレート構成。依存方向は下から上への一方向のみ:

```
bi-app        HTTPサーバー(tiny_http) / SQLite in-memoryクエリエンジン / 分析API / 内蔵UI
  ├─ bi-analytics  記述統計・相関・OLS回帰・k-means++・統計検定（純Rust・依存はserdeのみ）
  ├─ bi-connectors CSV / Excel(calamine) / SQLite / PostgreSQL / MySQL(sqlx) コネクタ + ConnectorRegistry + Parquetキャッシュ(arrow)
  └─ bi-core       TableData / DataType / Value / Connector trait / Project モデル・型推定
```

データフロー: `Connector::load` → `TableData`（正規化済み内部表形式）→ `Engine::register` で SQLite in-memory のテーブルに登録 → SQL（ユーザー入力 / チャート生成クエリ / 分析クエリ）→ `QueryResult` → UI表示または `bi-analytics` へ。ソースの異なるデータセット同士もSQLでJOINできるのはこの正規化のおかげ。

### 設計原則（docs/architecture.md より。変更時に守ること）

1. **UIにデータ処理を書かない** — UIは Command（JSON API）を投げるだけ。処理はすべてRust側。
2. **すべての入力は `TableData` に正規化** — UI・クエリエンジンはデータの出所を意識しない。
3. **可視化は ChartSpec（JSON定義）として保存** — 描画処理ではなくグラフ定義を保存する。
4. **状態は Project に集約** — 作業状態は `.kohaku` ファイル（JSON、データ本体は含まない）へ。
5. **ソース固有処理はコネクタ内に閉じ込める** — Excelの日付シリアル値等を Core やエンジンに漏らさない。

### 主要ファイル

- `crates/bi-app/src/server.rs` — HTTPサーバー。全APIは `POST /api/*`（JSON）で、`handle_api()` がディスパッチする。新しいAPIはここに追加。ローカルCSRF対策（POSTのみ / `Content-Type: application/json` 必須 / `Origin` は localhost のみ許可）を壊さないこと。
- `crates/bi-app/src/engine.rs` — SQLite in-memory クエリエンジン。省メモリ用PRAGMA設定あり。
- `crates/bi-app/src/analysis.rs` — 分析API（profile / regression / cluster / advise / test）。**グループ別実行（`api_group`）は既存の分析関数を再利用する**（グループ値ごとに `source` を絞り込んだ SQL に差し替えて呼ぶだけ）。対応分析を増やすときは `api_group` の match に足す。個別の分析関数にグループ対応を書かないこと。1グループの失敗で全体を止めず、そのグループだけ `error` を返す。
- `crates/bi-app/ui/` — 内蔵UI（vanilla JS + 自前Canvasレンダラ、約3,000行の `app.js`）。`include_str!` でバイナリに埋め込まれるため、**UIの変更も再ビルドが必要**。JSライブラリの追加は不可（オフライン完結・依存ゼロ）。注意: incrementalビルドが `ui/*.js` の変更を拾わないことがある。UI変更が反映されない時は `ui/` 配下のファイルを touch してからビルドするか `cargo clean -p bi-app` する。
- **配色は `ui/style.css` のCSS変数に一元化する**（ダーク既定 / `<html data-theme="light">` でライト）。JSに色をハードコードしないこと。Canvasの色は `refreshThemeColors()` がCSS変数から読んで `CHART_COLORS` / `SERIES_COLORS` に入れる。新しい描画関数は先頭で `registerRedraw(canvas, () => 同じ引数で再描画)` を呼ぶ（テーマ切替時に描き直すため）。**ブランド色（`--accent`＝琥珀）はUI用、データの色は `--chart-primary` / `--series-N` を使う**（混ぜない）。
- `crates/bi-analytics/src/` — `lib.rs`（統計・回帰・k-means++）、`htest.rs`（統計検定・効果量・多重比較）、`distributions.rs`（p値計算）、`advisor.rs`（検定の自動提案）。**このクレートは外部依存なし（serdeのみ）を維持する。**
- `crates/bi-core/src/lib.rs` — `TableData` / `DataType` / `Value` / `Connector` trait / `BiResult`。
- `crates/bi-connectors/src/parquet_cache.rs` — Parquetキャッシュ（v0.3）。ファイル系ソースの取り込み結果を `%LOCALAPPDATA%\kohaku-studio\cache\` に保存し、ソース未変更（サイズ+更新時刻一致）なら再パースせず復元する。キャッシュ失敗は常にソース読み込みへフォールバックし、ユーザーを止めない。DB接続は対象外。`--no-cache` で無効化。

### 拡張ポイント

- **新しいデータソース**: `bi_core::Connector` trait を実装し、`bi-connectors/src/lib.rs` の `ConnectorRegistry::new()` に登録するだけ。UI・エンジン側の変更は不要（ファイルは拡張子、接続URLはスキームで解決される）。
- **コネクタプラグイン（再ビルド不要の拡張）**: `bi-connectors/src/plugin.rs`。`%LOCALAPPDATA%\kohaku-studio\plugins\<name>\plugin.json` を読み、外部プロセスと stdin/stdout のJSON 1往復で通信する（`--enable-plugins` 時のみ有効。既定は無効=任意コード実行のため）。**入出力はUTF-8固定**（Windowsのロケール依存出力で壊れるため、非UTF-8は専用エラーにしている）。診断は `--list-plugins`。サンプルは `examples/plugins/`。
- **新しいチャートタイプ**: `bi-app/ui/app.js` の `kohaku.registerChartType()` で登録する（`docs/plugin-api-draft.md` 6章と同一のAPI。実装例はウェハーマップ）。既存5種（棒/折れ線/散布図/ヒストグラム/テーブル）のみ従来どおり `buildChartQuery()` / `renderChart()` 内の分岐。
  - `form` で使うフォーム行を宣言する（`{x, y, value, series, agg, yrange, facet, facetMax}`。文字列を渡すとラベルを差し替え）。`facet: true` で本体のファセット分割に乗る（`buildQuery` はファセット列を `f` で返し、`render` は第7引数 `shared = {facetValue, allRows}` を受け取ってスケールを全パネルで共有する。実装例はウェハーマップ）。**凡例・カラースケールは `renderLegend()` + `legendWidth` で宣言し、本体に格子の外側へ描かせる**（パネル内に描くとそのパネルだけ余白が変わり、大きさと位置が揃わなくなる）。
  - **軸のレンジは `niceTicks()`（自動。データ範囲を必ず覆う）→ `H.applyManualRange(spec, ticks, count)`（手動指定を重ねる）の順で求め、データの描画は必ずプロット領域でクリップする**。この3点を守らないとプロットが枠外に出て軸ラベルと重なる。範囲外に出る注釈線（管理限界など）は描画せず、その旨を画面に明示すること。
  - **`fetch` チャート（SPC管理図）のファセット**は結果を `f` 列で分割できないため、`fetch` 自身が `/api/analyze/group` を呼んで `{group, groups: [{value, result|error}]}` を返す。本体は `groups` があればそれをパネル列として描く（`renderRegistryFacets`）。パネル間のスケール共有は `shared.allResults` から求める。
  - **Canvasの文字状態は描く直前に必ず設定する**（`textAlign` / `textBaseline`）。ファセットは見出しを描いた同じ ctx を各パネルに渡すため、前の設定が残っていると文字が枠外へずれる（v0.7 で SPC の情報行が下段の見出しと重なった）。本体側は `resetTextState()` で既定に戻してから `render` を呼ぶ。
  - **ファセットの格子は外周の余白（`FACET_PAD`）と段の間隔（`FACET_ROW_GAP`）を引いてからパネル高さを決める**。これがないと最上段・最下段の文字がCanvas端で切れ、段の境目でも文字が接触する。
  - 組み込みチャートの描画は `renderChart`（入口）→ `renderFacets`（ファセット分割）→ `drawChartArea`（1枚を描く）の3層。ファセット間では **Y軸レンジ・ヒストグラムのビン・系列の色順を必ず共有する**（`shared` 引数）。揃えないとパネル同士を見比べられない。
  - **2次元ファセット（行×列）**は `spec.facet`＝列（横）/ `spec.facet2`＝行（縦）。SQL派生（`f`/`f2` 列）は `renderSqlFacetGrid`、`fetch` 派生（SPCの group2 ペア）は `renderFetchFacetGrid` が担当し、どちらも `renderFacetGrid`（列見出し上・行見出し左の格子）に委譲する。スケール共有の計算は 1D/2D 共通の `computeFacetShared()` に集約（個別に書かない）。各軸 `FACET2_DIM_MAX`（=6）まで、SPCのペア列挙はサーバ `GROUP2_MAX`（=36）まで。

## 開発フロー

- feature ブランチ → push 前に上記の fmt / clippy / test を通す → PR 作成 → CI green を確認 → **マージとタグの強制pushはオーナーが行う**（Claude は自分の PR をマージしない）。
- CI の Rust stable はローカルより新しいことがある。ローカルで通るのに CI の clippy だけ落ちる場合は `rustup update stable` でローカルを揃えてから再現する。
- コミット前に、変更が UI で確認できるものは必ず実際に動かして検証する（テストが通る ≠ 動く）。

### リリース手順

バージョン更新PR（`Cargo.toml` / `Cargo.lock` / `ROADMAP.md` / `CLAUDE.md`）をマージしてから、以下を実行する。
**既定はオーナーが実行する**が、依頼があれば Claude が代行してよい（その場合も実行したコマンドを提示する）。

```bash
gh pr merge <N> --squash --delete-branch
git checkout main; git pull origin main
# タグは必ず注釈付き(-a)で作る。v0.2.0 以降すべて注釈付きで統一している
git tag -a v0.6.0 -m "v0.6.0: per-column visualization (histogram hue, facet grids, multi-wafer maps)"
git push origin v0.6.0
gh release create v0.6.0 --title "Kohaku Studio v0.6.0" --generate-notes
```

- `--generate-notes` はPR一覧とFull Changelogを自動生成する。日本語の概要を冒頭に足す場合は `--notes-file` を併用する。
- **公開済みのタグを作り直すと、そのタグに紐づく GitHub Release は下書きに戻る**（本文は残る）。
  `gh release edit <tag> --draft=false` で公開し直すこと（v0.6.0 で実際に発生）。

## 規約

- エラー型は `BiResult<T> = Result<T, String>`。コード内のコメント・エラーメッセージ・ドキュメントは**日本語**で書く（既存コードに合わせる）。
- ユニットテストは各モジュール内の `#[cfg(test)] mod tests` に置く（コネクタ・エンジン・分析に既存例あり）。
- 実行時の制限値（インポート200万行、SQL結果表示2,000行など）は README に明記されている。挙動を変える場合は README も更新する。
- ロードマップは `ROADMAP.md`（現在 **v1.0 完了 = 安定版**。ドキュメント整合・分析の打ち切り警告・主要機能の実データ検証をまとめた仕上げリリース）。v1.x 以降の候補は ROADMAP に記載（Plugin API Phase 2/3、時系列予測）。次の計画はオーナーと相談して決める。
