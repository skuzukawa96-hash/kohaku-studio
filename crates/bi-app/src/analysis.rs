//! 分析API: データプロファイル / 回帰分析 / クラスタリング。
//! データ取得はクエリエンジン経由(データセットまたは任意SQLをソースにできる)。

use crate::engine::QueryResult;
use crate::server::AppState;
use bi_analytics::htest::{self, Correction};
use bi_core::*;
use serde_json::{json, Value as Json};
use std::collections::{HashMap, HashSet};

/// 分析に読み込む最大行数
const ANALYZE_LIMIT: usize = 1_000_000;
/// 相関行列の対象にする数値列の上限(O(p²·n)の暴走防止)
const MAX_CORR_COLS: usize = 20;
/// 個別値カウントの上限
const DISTINCT_CAP: usize = 10_000;

fn s(v: &Json, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn str_list(v: &Json, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// source: {kind: "dataset"|"sql", dataset?/sql?} をクエリ結果に解決する
fn resolve_source(state: &AppState, req: &Json) -> BiResult<QueryResult> {
    let src = req.get("source").ok_or("sourceが指定されていません")?;
    let kind = src
        .get("kind")
        .and_then(|x| x.as_str())
        .unwrap_or("dataset");
    let sql = if kind == "sql" {
        let q = src
            .get("sql")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .trim_end_matches(';')
            .to_string();
        if q.is_empty() {
            return Err("SQLが空です".to_string());
        }
        q
    } else {
        let ds = src.get("dataset").and_then(|x| x.as_str()).unwrap_or("");
        if ds.is_empty() {
            return Err("データセットを指定してください".to_string());
        }
        format!("SELECT * FROM \"{}\"", ds.replace('"', "\"\""))
    };
    state.engine.query(&sql, ANALYZE_LIMIT)
}

/// 列を f64 ベクトルに変換する(数値以外・NULLは NaN)
fn col_f64(result: &QueryResult, idx: usize) -> Vec<f64> {
    result
        .rows
        .iter()
        .map(|r| match &r[idx] {
            Value::Int(i) => *i as f64,
            Value::Float(f) => *f,
            Value::Bool(b) => *b as i64 as f64,
            _ => f64::NAN,
        })
        .collect()
}

/// 列が数値列か(非NULL値がすべて数値)
fn is_numeric_col(result: &QueryResult, idx: usize) -> bool {
    let mut any = false;
    for r in &result.rows {
        match &r[idx] {
            Value::Int(_) | Value::Float(_) => any = true,
            Value::Null => {}
            _ => return false,
        }
    }
    any
}

fn col_index(result: &QueryResult, name: &str) -> BiResult<usize> {
    result
        .columns
        .iter()
        .position(|c| c == name)
        .ok_or_else(|| format!("列「{name}」が見つかりません"))
}

fn round4(x: f64) -> Json {
    if x.is_finite() {
        json!((x * 10000.0).round() / 10000.0)
    } else {
        Json::Null
    }
}

// ---------- データプロファイル ----------

pub fn api_profile(state: &mut AppState, req: &Json) -> BiResult<Json> {
    let result = resolve_source(state, req)?;
    let ncols = result.columns.len();
    let nrows = result.rows.len();
    if ncols == 0 {
        return Err("結果に列がありません".to_string());
    }

    let mut columns_out = Vec::with_capacity(ncols);
    let mut numeric_cols: Vec<(usize, Vec<f64>)> = Vec::new();

    for c in 0..ncols {
        let mut nulls = 0usize;
        let mut distinct: HashSet<String> = HashSet::new();
        let mut capped = false;
        for r in &result.rows {
            match &r[c] {
                Value::Null => nulls += 1,
                v => {
                    if distinct.len() < DISTINCT_CAP {
                        distinct.insert(match v {
                            Value::Text(t) => t.clone(),
                            Value::Int(i) => i.to_string(),
                            Value::Float(f) => f.to_string(),
                            Value::Bool(b) => b.to_string(),
                            Value::Null => unreachable!(),
                        });
                    } else {
                        capped = true;
                    }
                }
            }
        }
        let numeric = is_numeric_col(&result, c);
        let stats_json = if numeric {
            let vals = col_f64(&result, c);
            let st = bi_analytics::numeric_stats(&vals);
            if numeric_cols.len() < MAX_CORR_COLS {
                numeric_cols.push((c, vals));
            }
            st.map(|st| {
                json!({
                    "mean": round4(st.mean), "std": round4(st.std),
                    "min": round4(st.min), "q25": round4(st.q25),
                    "median": round4(st.median), "q75": round4(st.q75),
                    "max": round4(st.max),
                })
            })
        } else {
            None
        };
        columns_out.push(json!({
            "name": result.columns[c],
            "kind": if numeric { "numeric" } else { "text" },
            "count": nrows - nulls,
            "nulls": nulls,
            "distinct": distinct.len(),
            "distinct_capped": capped,
            "stats": stats_json,
        }));
    }

    // 相関行列と強相関ペア
    let corr_names: Vec<&String> = numeric_cols
        .iter()
        .map(|(i, _)| &result.columns[*i])
        .collect();
    let mut matrix: Vec<Vec<Json>> = Vec::new();
    let mut pairs: Vec<(String, String, f64)> = Vec::new();
    for (i, (_, xi)) in numeric_cols.iter().enumerate() {
        let mut row = Vec::new();
        for (j, (_, xj)) in numeric_cols.iter().enumerate() {
            if i == j {
                row.push(json!(1.0));
            } else {
                match bi_analytics::pearson(xi, xj) {
                    Some(r) => {
                        row.push(round4(r));
                        if j > i {
                            pairs.push((corr_names[i].clone(), corr_names[j].clone(), r));
                        }
                    }
                    None => row.push(Json::Null),
                }
            }
        }
        matrix.push(row);
    }
    pairs.sort_by(|a, b| b.2.abs().partial_cmp(&a.2.abs()).unwrap());
    let top_pairs: Vec<Json> = pairs
        .iter()
        .take(10)
        .map(|(a, b, r)| json!({"a": a, "b": b, "r": round4(*r)}))
        .collect();

    Ok(json!({
        "n_rows": nrows,
        "truncated": result.truncated,
        "columns": columns_out,
        "correlation": {"columns": corr_names, "matrix": matrix},
        "top_pairs": top_pairs,
    }))
}

// ---------- 回帰分析 ----------

pub fn api_regression(state: &mut AppState, req: &Json) -> BiResult<Json> {
    let target = s(req, "target");
    let features = str_list(req, "features");
    if target.is_empty() {
        return Err("目的変数を指定してください".to_string());
    }
    if features.is_empty() {
        return Err("説明変数を1つ以上指定してください".to_string());
    }
    if features.contains(&target) {
        return Err("目的変数と説明変数が重複しています".to_string());
    }
    let result = resolve_source(state, req)?;
    let ti = col_index(&result, &target)?;
    let fis: Vec<usize> = features
        .iter()
        .map(|f| col_index(&result, f))
        .collect::<BiResult<Vec<_>>>()?;

    let y_all = col_f64(&result, ti);
    let x_all: Vec<Vec<f64>> = fis.iter().map(|&i| col_f64(&result, i)).collect();

    // 欠損行を除外して行列を構築
    let mut rows: Vec<Vec<f64>> = Vec::new();
    let mut y: Vec<f64> = Vec::new();
    for i in 0..y_all.len() {
        if !y_all[i].is_finite() || x_all.iter().any(|c| !c[i].is_finite()) {
            continue;
        }
        rows.push(x_all.iter().map(|c| c[i]).collect());
        y.push(y_all[i]);
    }
    let dropped = y_all.len() - y.len();
    let r = bi_analytics::ols(&rows, &y)?;

    // チャート用サンプル(最大2000点)
    let step = (r.n / 2000).max(1);
    let single = features.len() == 1;
    let points: Vec<Json> = (0..r.n)
        .step_by(step)
        .map(|i| {
            if single {
                json!([round4(rows[i][0]), round4(y[i]), round4(r.predicted[i])])
            } else {
                json!([round4(y[i]), round4(r.predicted[i])])
            }
        })
        .collect();

    let mut names = vec!["(切片)".to_string()];
    names.extend(features.iter().cloned());
    Ok(json!({
        "names": names,
        "coef": r.coef.iter().map(|v| round4(*v)).collect::<Vec<_>>(),
        "stderr": r.stderr.iter().map(|v| round4(*v)).collect::<Vec<_>>(),
        "tvalues": r.tvalues.iter().map(|v| round4(*v)).collect::<Vec<_>>(),
        "r2": round4(r.r2),
        "adj_r2": round4(r.adj_r2),
        "rmse": round4(r.rmse),
        "n": r.n,
        "dropped": dropped,
        "single_feature": single,
        "target": target,
        "feature": features.first(),
        "points": points,
    }))
}

// ---------- クラスタリング ----------

pub fn api_cluster(state: &mut AppState, req: &Json) -> BiResult<Json> {
    let features = str_list(req, "features");
    if features.is_empty() {
        return Err("特徴量を1つ以上指定してください".to_string());
    }
    let k = req
        .get("k")
        .and_then(|x| x.as_u64())
        .unwrap_or(3)
        .clamp(2, 50) as usize;
    let save_as = s(req, "save_as");

    let result = resolve_source(state, req)?;
    let fis: Vec<usize> = features
        .iter()
        .map(|f| col_index(&result, f))
        .collect::<BiResult<Vec<_>>>()?;
    let x_all: Vec<Vec<f64>> = fis.iter().map(|&i| col_f64(&result, i)).collect();

    let mut rows: Vec<Vec<f64>> = Vec::new();
    let mut kept: Vec<usize> = Vec::new();
    for i in 0..result.rows.len() {
        if x_all.iter().any(|c| !c[i].is_finite()) {
            continue;
        }
        rows.push(x_all.iter().map(|c| c[i]).collect());
        kept.push(i);
    }
    let dropped = result.rows.len() - rows.len();
    let km = bi_analytics::kmeans(&rows, k, 42)?;

    // チャート用サンプル(最大5000点): [f1, f2, ..., cluster]
    let step = (rows.len() / 5000).max(1);
    let points: Vec<Json> = (0..rows.len())
        .step_by(step)
        .map(|i| {
            let mut p: Vec<Json> = rows[i].iter().map(|v| round4(*v)).collect();
            p.push(json!(km.assignments[i]));
            json!(p)
        })
        .collect();

    // 結果を新データセットとして登録(元の全列 + cluster列)
    let mut saved = Json::Null;
    if !save_as.trim().is_empty() {
        // データセット名の扱いをapi_importと統一する
        let name = crate::server::sanitize_dataset_name(save_as.trim());
        let mut names = result.columns.clone();
        names.push("cluster".to_string());
        let names = normalize_names(names);
        let ncols = names.len();
        let mut data_rows: Vec<Vec<Value>> = kept
            .iter()
            .enumerate()
            .map(|(j, &i)| {
                let mut row = result.rows[i].clone();
                row.push(Value::Int(km.assignments[j] as i64));
                row
            })
            .collect();
        let types = unify_columns(&mut data_rows, ncols);
        let schema = TableSchema {
            columns: names
                .into_iter()
                .zip(types)
                .map(|(name, data_type)| ColumnSchema { name, data_type })
                .collect(),
        };
        let td = TableData {
            schema: schema.clone(),
            rows: data_rows,
        };
        let row_count = td.rows.len();
        state.engine.register(&name, &td)?;
        state.datasets.retain(|d| d.name != name);
        state.datasets.push(DatasetDef {
            name: name.clone(),
            path: "(クラスタリング結果)".to_string(),
            object: String::new(),
            options: ImportOptions::default(),
            row_count,
            schema: Some(schema),
        });
        saved = json!({"name": name, "rows": row_count});
    }

    let centroids: Vec<Vec<Json>> = km
        .centroids
        .iter()
        .map(|c| c.iter().map(|v| round4(*v)).collect())
        .collect();
    Ok(json!({
        "k": k,
        "features": features,
        "sizes": km.sizes,
        "centroids": centroids,
        "inertia": round4(km.inertia),
        "iterations": km.iterations,
        "n_used": rows.len(),
        "dropped": dropped,
        "points": points,
        "saved": saved,
    }))
}

// ---------- Kohaku Test Advisor(統計検定) ----------

fn value_label(v: &Value) -> String {
    match v {
        Value::Null => "(null)".to_string(),
        Value::Text(t) => t.clone(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
    }
}

/// カテゴリ群の上限(UI・計算コスト保護)
const MAX_GROUPS: usize = 50;

/// 数値目的変数をカテゴリ列で群分けする(出現順を保持、NaN/NULLは除外)。
fn build_groups(result: &QueryResult, ti: usize, gi: usize) -> BiResult<Vec<(String, Vec<f64>)>> {
    let tcol = col_f64(result, ti);
    let mut order: Vec<String> = vec![];
    let mut map: HashMap<String, Vec<f64>> = HashMap::new();
    for (row, &tv) in result.rows.iter().zip(tcol.iter()) {
        if !tv.is_finite() || matches!(row[gi], Value::Null) {
            continue;
        }
        let label = value_label(&row[gi]);
        if !map.contains_key(&label) {
            if order.len() >= MAX_GROUPS {
                return Err(format!(
                    "群が多すぎます(最大{MAX_GROUPS}群)。群列を見直してください。"
                ));
            }
            order.push(label.clone());
        }
        map.entry(label).or_default().push(tv);
    }
    Ok(order
        .into_iter()
        .map(|k| {
            let v = map.remove(&k).unwrap();
            (k, v)
        })
        .collect())
}

/// 2数値列を対応づけ(両方が有限の行のみ)。
fn two_numeric_pairs(result: &QueryResult, xi: usize, yi: usize) -> (Vec<f64>, Vec<f64>) {
    let xc = col_f64(result, xi);
    let yc = col_f64(result, yi);
    let mut xs = vec![];
    let mut ys = vec![];
    for i in 0..result.rows.len() {
        if xc[i].is_finite() && yc[i].is_finite() {
            xs.push(xc[i]);
            ys.push(yc[i]);
        }
    }
    (xs, ys)
}

/// クロス集計の結果: (行ラベル, 列ラベル, 度数表)
type Contingency = (Vec<String>, Vec<String>, Vec<Vec<f64>>);

/// 2カテゴリ列のクロス集計(度数表)を作る。
fn contingency(result: &QueryResult, ri: usize, ci: usize) -> BiResult<Contingency> {
    let mut rlabels: Vec<String> = vec![];
    let mut clabels: Vec<String> = vec![];
    let mut counts: HashMap<(String, String), f64> = HashMap::new();
    for row in &result.rows {
        if matches!(row[ri], Value::Null) || matches!(row[ci], Value::Null) {
            continue;
        }
        let rl = value_label(&row[ri]);
        let cl = value_label(&row[ci]);
        if !rlabels.contains(&rl) {
            if rlabels.len() >= MAX_GROUPS {
                return Err("行カテゴリが多すぎます".to_string());
            }
            rlabels.push(rl.clone());
        }
        if !clabels.contains(&cl) {
            if clabels.len() >= MAX_GROUPS {
                return Err("列カテゴリが多すぎます".to_string());
            }
            clabels.push(cl.clone());
        }
        *counts.entry((rl, cl)).or_insert(0.0) += 1.0;
    }
    let table: Vec<Vec<f64>> = rlabels
        .iter()
        .map(|rl| {
            clabels
                .iter()
                .map(|cl| *counts.get(&(rl.clone(), cl.clone())).unwrap_or(&0.0))
                .collect()
        })
        .collect();
    Ok((rlabels, clabels, table))
}

fn alpha_of(req: &Json) -> f64 {
    req.get("alpha")
        .and_then(|x| x.as_f64())
        .filter(|a| *a > 0.0 && *a < 0.5)
        .unwrap_or(0.05)
}

/// 数値列の有限値のみを取り出す。
fn col_finite(result: &QueryResult, idx: usize) -> Vec<f64> {
    col_f64(result, idx)
        .into_iter()
        .filter(|v| v.is_finite())
        .collect()
}

/// カテゴリ列の値別件数(出現順、NULL除外)。
fn category_counts(result: &QueryResult, ci: usize) -> BiResult<Vec<(String, usize)>> {
    let mut order: Vec<String> = vec![];
    let mut map: HashMap<String, usize> = HashMap::new();
    for row in &result.rows {
        if matches!(row[ci], Value::Null) {
            continue;
        }
        let label = value_label(&row[ci]);
        if !map.contains_key(&label) {
            if order.len() >= MAX_GROUPS {
                return Err(format!(
                    "カテゴリが多すぎます(最大{MAX_GROUPS}種類)。列を見直してください。"
                ));
            }
            order.push(label.clone());
        }
        *map.entry(label).or_insert(0) += 1;
    }
    Ok(order
        .into_iter()
        .map(|k| {
            let c = map[&k];
            (k, c)
        })
        .collect())
}

/// 基準比率 p0 (0〜1) を取得。
fn p0_of(req: &Json) -> BiResult<f64> {
    let p0 = req.get("p0").and_then(|x| x.as_f64()).unwrap_or(0.5);
    if !(p0 > 0.0 && p0 < 1.0) {
        return Err("基準比率は0より大きく1未満で指定してください".to_string());
    }
    Ok(p0)
}

/// 検定候補の提案 (/api/analyze/advise)
pub fn api_advise(state: &mut AppState, req: &Json) -> BiResult<Json> {
    let result = resolve_source(state, req)?;
    let mode = s(req, "mode");
    let rec =
        match mode.as_str() {
            "groups" => {
                let ti = col_index(&result, &s(req, "target"))?;
                let gi = col_index(&result, &s(req, "group"))?;
                if !is_numeric_col(&result, ti) {
                    return Err("目的変数は数値列を選んでください".to_string());
                }
                let groups = build_groups(&result, ti, gi)?;
                bi_analytics::advisor::advise_numeric_groups(&groups, false)?
            }
            "two_numeric" => {
                let xi = col_index(&result, &s(req, "x"))?;
                let yi = col_index(&result, &s(req, "y"))?;
                if !is_numeric_col(&result, xi) || !is_numeric_col(&result, yi) {
                    return Err("2つとも数値列を選んでください".to_string());
                }
                let (xs, ys) = two_numeric_pairs(&result, xi, yi);
                let paired = req.get("paired").and_then(|x| x.as_bool()).unwrap_or(false);
                if paired {
                    let groups = vec![("測定1".to_string(), xs), ("測定2".to_string(), ys)];
                    bi_analytics::advisor::advise_numeric_groups(&groups, true)?
                } else {
                    bi_analytics::advisor::advise_two_numeric(&xs, &ys)?
                }
            }
            "categorical" => {
                let ri = col_index(&result, &s(req, "row"))?;
                let ci = col_index(&result, &s(req, "col"))?;
                let (_rl, _cl, table) = contingency(&result, ri, ci)?;
                bi_analytics::advisor::advise_categorical(&table)?
            }
            "one_sample" => {
                let ti = col_index(&result, &s(req, "target"))?;
                if !is_numeric_col(&result, ti) {
                    return Err("対象は数値列を選んでください".to_string());
                }
                let xs = col_finite(&result, ti);
                let mu0 = req.get("mu0").and_then(|x| x.as_f64()).unwrap_or(0.0);
                bi_analytics::advisor::advise_one_sample(&xs, mu0)?
            }
            "proportion" => {
                let ci = col_index(&result, &s(req, "column"))?;
                let counts = category_counts(&result, ci)?;
                bi_analytics::advisor::advise_proportion(&counts)?
            }
            _ => return Err(
                "mode は groups / two_numeric / categorical / one_sample / proportion のいずれか"
                    .to_string(),
            ),
        };
    serde_json::to_value(&rec).map_err(|e| e.to_string())
}

/// 単一検定を実行して結果を返す。
fn run_named_test(
    id: &str,
    result: &QueryResult,
    req: &Json,
    alpha: f64,
) -> BiResult<htest::TestResult> {
    let mode = s(req, "mode");
    match mode.as_str() {
        "groups" => {
            let ti = col_index(result, &s(req, "target"))?;
            let gi = col_index(result, &s(req, "group"))?;
            let groups = build_groups(result, ti, gi)?;
            let gv: Vec<Vec<f64>> = groups.iter().map(|(_, g)| g.clone()).collect();
            let mut r = match id {
                "welch_t" if gv.len() == 2 => htest::welch_t(&gv[0], &gv[1], alpha),
                "student_t" if gv.len() == 2 => htest::student_t(&gv[0], &gv[1], alpha),
                "mann_whitney" if gv.len() == 2 => htest::mann_whitney(&gv[0], &gv[1], alpha),
                "f_var" if gv.len() == 2 => htest::f_var_test(&gv[0], &gv[1], alpha),
                "anova" => htest::one_way_anova(&gv, alpha),
                "welch_anova" => htest::welch_anova(&gv, alpha),
                "kruskal" => htest::kruskal_wallis(&gv, alpha),
                "levene" => htest::levene_test(&gv, alpha),
                _ => Err(format!("この群構成では検定「{id}」を実行できません")),
            }?;
            // 汎用ラベル(群1,群2,...)を実際のカテゴリ名に置き換える
            for (gs, (label, _)) in r.groups.iter_mut().zip(groups.iter()) {
                gs.label = label.clone();
            }
            Ok(r)
        }
        "two_numeric" => {
            let xi = col_index(result, &s(req, "x"))?;
            let yi = col_index(result, &s(req, "y"))?;
            let (xs, ys) = two_numeric_pairs(result, xi, yi);
            match id {
                "pearson" => htest::pearson_test(&xs, &ys, alpha),
                "spearman" => htest::spearman_test(&xs, &ys, alpha),
                "kendall" => htest::kendall_test(&xs, &ys, alpha),
                "paired_t" => htest::paired_t(&xs, &ys, alpha),
                "wilcoxon" => htest::wilcoxon_signed_rank(&xs, &ys, alpha),
                _ => Err(format!("検定「{id}」を実行できません")),
            }
        }
        "categorical" => {
            let ri = col_index(result, &s(req, "row"))?;
            let ci = col_index(result, &s(req, "col"))?;
            let (_rl, _cl, table) = contingency(result, ri, ci)?;
            match id {
                "chi_square" => htest::chi_square_independence(&table, alpha),
                "fisher" if table.len() == 2 && table[0].len() == 2 => htest::fisher_exact_2x2(
                    table[0][0],
                    table[0][1],
                    table[1][0],
                    table[1][1],
                    alpha,
                ),
                _ => Err("Fisher検定は2×2表のみ対応です".to_string()),
            }
        }
        "one_sample" => {
            let ti = col_index(result, &s(req, "target"))?;
            let xs = col_finite(result, ti);
            let mu0 = req.get("mu0").and_then(|x| x.as_f64()).unwrap_or(0.0);
            match id {
                "one_sample_t" => htest::one_sample_t(&xs, mu0, alpha),
                "wilcoxon_1s" => htest::wilcoxon_one_sample(&xs, mu0, alpha),
                _ => Err(format!("検定「{id}」を実行できません")),
            }
        }
        "proportion" => {
            let ci = col_index(result, &s(req, "column"))?;
            let success = s(req, "success");
            if success.is_empty() {
                return Err("「成功」とみなすカテゴリを選択してください".to_string());
            }
            let counts = category_counts(result, ci)?;
            let n: usize = counts.iter().map(|(_, c)| c).sum();
            let k = counts
                .iter()
                .find(|(l, _)| *l == success)
                .map(|(_, c)| *c)
                .ok_or_else(|| format!("カテゴリ「{success}」が見つかりません"))?;
            let p0 = p0_of(req)?;
            match id {
                "binomial" => htest::binomial_test(k as u64, n as u64, p0, alpha),
                _ => Err(format!("検定「{id}」を実行できません")),
            }
        }
        _ => Err("不明なmodeです".to_string()),
    }
}

/// t系検定の事後検出力(観測効果量に基づく正規近似, 参考値)。
fn approx_power(id: &str, r: &htest::TestResult, alpha: f64) -> Option<f64> {
    use bi_analytics::distributions as dist;
    let d = r.effect.as_ref()?.value.abs();
    let delta = match id {
        "welch_t" | "student_t" => {
            let n1 = r.groups.first()?.n as f64;
            let n2 = r.groups.get(1)?.n as f64;
            d * (n1 * n2 / (n1 + n2)).sqrt()
        }
        "paired_t" | "one_sample_t" => d * (r.n as f64).sqrt(),
        _ => return None,
    };
    let z = dist::normal_ppf(1.0 - alpha / 2.0);
    Some(dist::normal_cdf(delta - z) + dist::normal_cdf(-delta - z))
}

/// 3群以上のペアワイズ事後検定(多重比較補正付き)。
fn posthoc_pairs(
    result: &QueryResult,
    req: &Json,
    parametric: bool,
    alpha: f64,
    correction: Correction,
) -> BiResult<Json> {
    let ti = col_index(result, &s(req, "target"))?;
    let gi = col_index(result, &s(req, "group"))?;
    let groups = build_groups(result, ti, gi)?;
    if groups.len() < 3 {
        return Ok(Json::Null);
    }
    let mut pairs: Vec<(String, String)> = vec![];
    let mut raw_p: Vec<f64> = vec![];
    let mut stats: Vec<(f64, Option<f64>)> = vec![]; // (統計量, 効果量)
    for i in 0..groups.len() {
        for j in (i + 1)..groups.len() {
            let r = if parametric {
                htest::welch_t(&groups[i].1, &groups[j].1, alpha)
            } else {
                htest::mann_whitney(&groups[i].1, &groups[j].1, alpha)
            }?;
            pairs.push((groups[i].0.clone(), groups[j].0.clone()));
            raw_p.push(r.p_value);
            stats.push((r.statistic, r.effect.map(|e| e.value)));
        }
    }
    let adj = htest::adjust_pvalues(&raw_p, correction);
    let method = if parametric {
        "Welchのt検定"
    } else {
        "Mann-Whitney U検定"
    };
    let items: Vec<Json> = pairs
        .iter()
        .enumerate()
        .map(|(k, (a, b))| {
            json!({
                "a": a, "b": b,
                "statistic": round4(stats[k].0),
                "effect": stats[k].1.map(round4).unwrap_or(Json::Null),
                // p値は丸めず生値で返す(UI側で「<0.0001」表示に整形)
                "p": raw_p[k],
                "p_adjusted": adj[k],
                "significant": adj[k] < alpha,
            })
        })
        .collect();
    Ok(json!({ "method": method, "pairs": items }))
}

/// 検定実行 (/api/analyze/test)
pub fn api_test(state: &mut AppState, req: &Json) -> BiResult<Json> {
    let result = resolve_source(state, req)?;
    let alpha = alpha_of(req);
    let id = s(req, "test");
    if id.is_empty() {
        return Err("検定を選択してください".to_string());
    }
    let res = run_named_test(&id, &result, req, alpha)?;
    let correction = Correction::from_name(&s(req, "correction"));

    // 3群以上で分散分析/Kruskalが有意なら、事後のペアワイズ比較を付ける
    let posthoc = if matches!(id.as_str(), "anova" | "welch_anova" | "kruskal") {
        let parametric = id != "kruskal";
        posthoc_pairs(&result, req, parametric, alpha, correction)?
    } else {
        Json::Null
    };

    let correction_label = match correction {
        Correction::None => "なし(未補正)",
        Correction::Bonferroni => "Bonferroni",
        Correction::Holm => "Holm",
        Correction::BenjaminiHochberg => "Benjamini-Hochberg (FDR)",
        Correction::BenjaminiYekutieli => "Benjamini-Yekutieli (FDR, 保守的)",
    };

    let power = approx_power(&id, &res, alpha)
        .map(round4)
        .unwrap_or(Json::Null);

    Ok(json!({
        "result": serde_json::to_value(&res).map_err(|e| e.to_string())?,
        "posthoc": posthoc,
        "correction": correction_label,
        "power": power,
        "note": "この結果は探索的分析です。事前に仮説・指標・検定を決めていない場合、確証的な結論には追試が必要です。",
    }))
}
