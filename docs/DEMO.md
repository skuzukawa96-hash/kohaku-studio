# デモ / サンプルデータ

Kohaku Studio の機能をひと通り試すための手順です。

## サンプルデータの生成

```bash
kohaku-studio --make-samples ./samples
```

次の2ファイルが生成されます。

| ファイル | 内容 |
| --- | --- |
| `sample_sales.csv` | 地域 × 製品の売上（90日分・1,080行）。列: date, region, product, units, unit_price |
| `sample_fab.db` | 半導体工場を模したSQLite。`tools`（装置マスタ）と `measurements`（歩留まり・欠陥数） |

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

5. **DBもインポート** — `samples/sample_fab.db` → `measurements` テーブルを取り込み

6. **回帰分析**（分析タブ → 回帰分析） — 目的変数 `yield`、説明変数 `defects`
   → 高い決定係数（R² ≈ 0.99）とフィット直線付き散布図が表示される

7. **クラスタリング**（分析タブ → クラスタリング） — 特徴量 `units` と `unit_price`、k = 3
   → クラスタ中心の表と色分け散布図。「結果をデータセット保存」で `cluster` 列付きデータセットが増える

8. **ダッシュボード** — 保存したチャートが一覧表示される

9. **保存** — 「💾 保存」でプロジェクトを `.kohaku` ファイルに保存。次回は「📂 開く」で復元

## タブのディープリンク

URLのハッシュでタブを直接開けます（ブックマークや共有に便利）。

```
http://127.0.0.1:5590/#sql
http://127.0.0.1:5590/#dashboard
```

対応: `#data` `#sql` `#charts` `#analytics` `#dashboard`

## スクリーンショットの撮影（メンテナ向け）

READMEに載せる画面キャプチャは `docs/images/` に配置します。ヘッドレスブラウザで撮る例:

```bash
# データとチャートを投入したサーバーを起動しておき、ダッシュボードを撮影
msedge --headless=new --disable-gpu --hide-scrollbars \
  --window-size=1280,900 --virtual-time-budget=4000 \
  --screenshot="docs/images/dashboard.png" "http://127.0.0.1:5590/#dashboard"
```
