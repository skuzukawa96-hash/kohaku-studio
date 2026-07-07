//! 分析API: データプロファイル / 回帰分析 / クラスタリング。
//! データ取得はクエリエンジン経由(データセットまたは任意SQLをソースにできる)。

use crate::engine::QueryResult;
use crate::server::AppState;
use bi_core::*;
use serde_json::{json, Value as Json};
use std::collections::HashSet;

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
