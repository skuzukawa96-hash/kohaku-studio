# デモ / サンプルデータ

Kohaku Studio の機能をひと通り試すための手順です。

## サンプルデータの生成

```bash
kohaku-studio --make-samples ./samples
```

次の3ファイルが生成されます。

| ファイル | 内容 |
| --- | --- |
| `sample_sales.csv` | 地域 × 製品の売上（90日分・1,080行）。列: date, region, product, units, unit_price |
| `sample_wafer.csv` | ウェハーマップ用のダイ座標データ（6枚 × 421ダイ）。列: wafer_id, die_x, die_y, yield。ウェハーごとに不良パターンが異なる（局所クラスタ2種 / エッジリング劣化 / 良品 / 左右勾配 / 全体低下） |
| `sample_fab.db` | 半導体工場を模したSQLite。`tools`（装置マスタ）と `measurements`（ロット × 装置の歩留まり・欠陥数） |

## デモの流れ

1. **起動**

   ```bash
   kohaku-studio
   ```

2. **CSVをインポート** — 「＋ インポート」→ `samples/sample_sales.csv` を選択 → 型推定プレビューを確認 → 取り込み

3. **SQLで集計**（SQLタブ、`Ctrl+Enter` で実行）

   ```sql
   SELECT region, SUM(units * unit_price) AS revenue
   FROM sample_sales
   GROUP BY region
   ORDER BY revenue DESC;
   ```

4. **チャート化** — 「結果からチャート作成」→ 棒グラフ（X: region / Y: revenue / 集計: 合計）

5. **ウェハーマップ** — `samples/sample_wafer.csv` をインポート → チャートタブでタイプ「ウェハーマップ」、
   X座標 `die_x` / Y座標 `die_y` / 値 `yield` / 分割 `wafer_id` → プレビュー。
   6枚のウェハーがグリッド表示され、局所不良クラスタ・エッジリング劣化・全体低下などの
   不良パターンの違いを一括で見比べられる（カラースケールは全ウェハー共通）。
   分割を空にすると全ウェハー平均の1枚合成マップになる

6. **DBもインポート** — `samples/sample_fab.db` → `measurements` テーブルを取り込み

7. **推移 + SPC 一式**（チャートタブ → 「⚡ 推移+SPC一式」） — データセット `measurements` を選ぶと
   列は自動推測される（順序列 `lot_id` / 測定値 `yield` / 系列 `tool_id`）→ 作成
   → 歩留まり推移とSPC管理図（±3σ管理限界・ネルソンルール異常検知）がダッシュボードに追加される

8. **装置差分析**（分析タブ → 🏭 装置差分析） — グループ列 `tool_id`、測定値 `yield`
   → Welch ANOVA が装置間の有意差を検出し、最も歩留まりの低い装置（ETCH-01）が赤で強調される

9. **ロットトレース**（分析タブ → 🔎 ロットトレース） — ID列 `lot_id`、`LOT0042` で検索
   → `lot_id` 列を持つ全データセットから該当行が横断的に集まる

10. **回帰分析**（分析タブ → 回帰分析） — 目的変数 `yield`、説明変数 `defects`
    → 高い決定係数（R² ≈ 0.99）とフィット直線付き散布図が表示される

11. **クラスタリング**（分析タブ → クラスタリング） — 特徴量 `units` と `unit_price`。
    「💡 kを自動提案」でエルボー法のWCSS曲線と提案kを確認 → 実行
    → クラスタ中心の表と色分け散布図。「結果をデータセット保存」で `cluster` 列付きデータセットが増える

12. **ダッシュボード** — 保存したチャートが一覧表示される。「📷 PNG保存」で1枚の画像に、
    「📄 HTMLレポート」でオフラインで開ける自己完結HTMLにエクスポートできる

13. **保存** — 「💾 保存」でプロジェクトを `.kohaku` ファイルに保存。次回は「📂 開く」で復元
    （ファイル系ソースはParquetキャッシュにより、ソース未変更なら高速に再読み込みされる）

> 時系列分解（分析タブ → 📆 時系列分解）は日次・月次など周期性のあるデータで真価を発揮します。
> 手持ちの時系列データがあれば、時間列と値の列・周期（週=7、年=12など）を指定して試してください。

## データベース接続を試す（PostgreSQL / MySQL）

開発用の使い捨てDBを Docker で起動できます（[Docker](https://www.docker.com/) が必要）。

```bash
docker compose up -d              # PostgreSQL と MySQL を起動
docker compose up -d postgres     # 片方だけ起動する場合
```

起動後、「＋ インポート」→「🗄 データベース」タブで接続URLを入力して「接続」:

| DB | 接続URL |
| --- | --- |
| PostgreSQL | `postgres://kohaku:kohaku@localhost:5432/demo` |
| MySQL | `mysql://kohaku:kohaku@localhost:3306/demo` |

`sales` / `customers` テーブルが見えるので、選択してプレビュー → 取り込み。
以降はファイル由来のデータと同じく SQL・チャート・分析で扱えます。

停止・後片付け:

```bash
docker compose down       # 停止（データは次回も残る）
docker compose down -v    # データも削除して完全リセット
```

> 接続URLはプロジェクト（`.kohaku`）に平文で保存されます。共有する環境では
> 読み取り専用ユーザーを使ってください。クラウドの無料枠（Neon など）の
> 接続URLでも同様に接続できます。

## タブのディープリンク

URLのハッシュでタブを直接開けます（ブックマークや共有に便利）。

```
http://127.0.0.1:5590/#sql
http://127.0.0.1:5590/#dashboard
```

対応: `#data` `#sql` `#charts` `#analytics` `#dashboard`

## テーマの指定

ヘッダーの 🌙 / ☀️ ボタンで切り替えられ、選択はブラウザに保存されます。
URLで明示することもできます（保存された設定より優先。スクリーンショットの撮影に便利）。

```
http://127.0.0.1:5590/?theme=light#dashboard
http://127.0.0.1:5590/?theme=dark
```

## スクリーンショットの撮影（メンテナ向け）

READMEに載せる画面キャプチャは `docs/images/` に配置します。ヘッドレスブラウザで撮る例:

```bash
# データとチャートを投入したサーバーを起動しておき、テーマごとに撮影
msedge --headless=new --disable-gpu --hide-scrollbars \
  --window-size=1280,900 --virtual-time-budget=9000 \
  --screenshot="docs/images/dashboard.png" "http://127.0.0.1:5590/?theme=dark#dashboard"

msedge --headless=new --disable-gpu --hide-scrollbars \
  --window-size=1280,900 --virtual-time-budget=9000 \
  --screenshot="docs/images/dashboard-light.png" "http://127.0.0.1:5590/?theme=light#dashboard"
```

`?theme=` を付けないと、ヘッドレスブラウザ側の設定に左右されて意図しないテーマで
撮れることがあります。

### アイコンの作り直し

アプリのアイコンは `crates/bi-app/assets/icon.svg` が原本です。
UIのfavicon（`index.html` にインライン）とヘッダーロゴも同じ形を使っています。
`icon.ico` を作り直す場合は、SVGを256pxでラスタライズしてから
16/24/32/48/64/128/256 の多重サイズICOに変換し、`assets/icon.ico` を置き換えます
（`build.rs` がWindowsビルド時に `embed-resource` で実行ファイルへ埋め込みます）。
