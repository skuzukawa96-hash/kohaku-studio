//! bi-core: BIツール内部の共通データモデル。
//! すべてのデータソース(CSV/Excel/DB)はここで定義する TableData に正規化される。

use serde::{Deserialize, Serialize};
use std::path::Path;

pub type BiResult<T> = Result<T, String>;

/// 内部データ型。外部ソースの型はすべてこれに写像する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataType {
    Null,
    Boolean,
    Int64,
    Float64,
    Utf8,
}

impl DataType {
    pub fn name(&self) -> &'static str {
        match self {
            DataType::Null => "null",
            DataType::Boolean => "boolean",
            DataType::Int64 => "integer",
            DataType::Float64 => "real",
            DataType::Utf8 => "text",
        }
    }
    /// SQLite上でのストレージ型
    pub fn sqlite_type(&self) -> &'static str {
        match self {
            DataType::Boolean | DataType::Int64 => "INTEGER",
            DataType::Float64 => "REAL",
            _ => "TEXT",
        }
    }
}

/// セル値。JSONへは untagged でそのままシリアライズされる(Null → null)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl Value {
    pub fn dtype(&self) -> DataType {
        match self {
            Value::Null => DataType::Null,
            Value::Bool(_) => DataType::Boolean,
            Value::Int(_) => DataType::Int64,
            Value::Float(_) => DataType::Float64,
            Value::Text(_) => DataType::Utf8,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnSchema {
    pub name: String,
    pub data_type: DataType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSchema {
    pub columns: Vec<ColumnSchema>,
}

/// 正規化済み表データ。コネクタの出力であり、クエリエンジンの入力。
#[derive(Debug, Clone)]
pub struct TableData {
    pub schema: TableSchema,
    pub rows: Vec<Vec<Value>>,
}

/// インポート時のオプション。ソース種別ごとに使う項目が異なる。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportOptions {
    /// ヘッダー行(1始まり)。0 はヘッダーなし(列名を自動生成)
    pub header_row: usize,
    /// ヘッダー後にスキップするデータ行数
    pub skip_rows: usize,
    /// CSV区切り文字。None なら自動判定
    pub delimiter: Option<String>,
    /// 読み込む最大行数(プレビュー用)
    pub max_rows: Option<usize>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        ImportOptions {
            header_row: 1,
            skip_rows: 0,
            delimiter: None,
            max_rows: None,
        }
    }
}

/// データコネクタ共通trait。新形式対応はこのtraitの実装を追加するだけでよい。
pub trait Connector: Send + Sync {
    fn connector_type(&self) -> &'static str;
    /// 対応する拡張子(小文字)
    fn extensions(&self) -> &'static [&'static str];
    /// ソース内のオブジェクト一覧(Excelならシート、DBならテーブル)
    fn list_objects(&self, path: &Path) -> BiResult<Vec<String>>;
    /// オブジェクトを TableData として読み込む
    fn load(&self, path: &Path, object: &str, opts: &ImportOptions) -> BiResult<TableData>;
}

/// 登録済みデータセットの定義(プロジェクト保存対象)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetDef {
    pub name: String,
    pub path: String,
    pub object: String,
    pub options: ImportOptions,
    #[serde(default)]
    pub row_count: usize,
    #[serde(default)]
    pub schema: Option<TableSchema>,
}

/// プロジェクトファイル(JSON)の構造
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    pub name: String,
    pub datasets: Vec<DatasetDef>,
    /// チャート定義はUI側スキーマのJSONをそのまま保存する(ChartSpec)
    pub charts: Vec<serde_json::Value>,
    #[serde(default)]
    pub queries: Vec<String>,
}

// ---------- 共通ユーティリティ ----------

/// 列名の正規化: 空欄には連番名、重複にはサフィックスを付ける
pub fn normalize_names(raw: Vec<String>) -> Vec<String> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    raw.into_iter()
        .enumerate()
        .map(|(i, n)| {
            let mut name = n.trim().to_string();
            if name.is_empty() {
                name = format!("column_{}", i + 1);
            }
            let count = seen.entry(name.clone()).or_insert(0);
            *count += 1;
            if *count > 1 {
                name = format!("{}_{}", name, *count);
            }
            name
        })
        .collect()
}

/// 文字列セルの表から列型を推定し、値をパースする(CSV用)。
/// 列の非空値がすべて整数なら Int64、すべて数値なら Float64、それ以外は Utf8。
pub fn parse_text_table(rows: Vec<Vec<String>>, ncols: usize) -> (Vec<DataType>, Vec<Vec<Value>>) {
    let mut all_int = vec![true; ncols];
    let mut all_float = vec![true; ncols];
    let mut any_val = vec![false; ncols];
    // 型推定は先頭2000行をサンプルにする(全走査を避けて高速化)
    for row in rows.iter().take(2000) {
        for c in 0..ncols {
            let s = row.get(c).map(|s| s.trim()).unwrap_or("");
            if s.is_empty() {
                continue;
            }
            any_val[c] = true;
            if all_int[c] && parse_int(s).is_none() {
                all_int[c] = false;
            }
            if all_float[c] && parse_float(s).is_none() {
                all_float[c] = false;
            }
        }
    }
    let types: Vec<DataType> = (0..ncols)
        .map(|c| {
            if !any_val[c] {
                DataType::Utf8
            } else if all_int[c] {
                DataType::Int64
            } else if all_float[c] {
                DataType::Float64
            } else {
                DataType::Utf8
            }
        })
        .collect();
    let parsed = rows
        .into_iter()
        .map(|row| {
            (0..ncols)
                .map(|c| {
                    let s = row.get(c).map(|s| s.trim()).unwrap_or("");
                    if s.is_empty() {
                        return Value::Null;
                    }
                    match types[c] {
                        DataType::Int64 => parse_int(s).map(Value::Int).unwrap_or_else(|| Value::Text(s.to_string())),
                        DataType::Float64 => parse_float(s).map(Value::Float).unwrap_or_else(|| Value::Text(s.to_string())),
                        _ => Value::Text(s.to_string()),
                    }
                })
                .collect()
        })
        .collect();
    (types, parsed)
}

fn parse_int(s: &str) -> Option<i64> {
    let t = s.replace(',', "");
    t.parse::<i64>().ok()
}

fn parse_float(s: &str) -> Option<f64> {
    let t = s.replace(',', "");
    t.parse::<f64>().ok()
}

/// 型付き値の表(Excel/DB由来)の列型を統一する。
/// Int と Float が混在すれば Float64 に昇格、Text が混じれば Utf8。
pub fn unify_columns(rows: &mut Vec<Vec<Value>>, ncols: usize) -> Vec<DataType> {
    let mut types = vec![DataType::Null; ncols];
    for row in rows.iter() {
        for c in 0..ncols {
            let vt = row.get(c).map(|v| v.dtype()).unwrap_or(DataType::Null);
            types[c] = widen(types[c], vt);
        }
    }
    for t in types.iter_mut() {
        if *t == DataType::Null {
            *t = DataType::Utf8;
        }
    }
    // 昇格が必要な値を変換する
    for row in rows.iter_mut() {
        for c in 0..ncols {
            if let Some(v) = row.get_mut(c) {
                let new_v = match (types[c], &*v) {
                    (DataType::Float64, Value::Int(i)) => Some(Value::Float(*i as f64)),
                    (DataType::Int64, Value::Bool(b)) => Some(Value::Int(*b as i64)),
                    (DataType::Float64, Value::Bool(b)) => Some(Value::Float(*b as i64 as f64)),
                    (DataType::Utf8, Value::Int(i)) => Some(Value::Text(i.to_string())),
                    (DataType::Utf8, Value::Float(f)) => Some(Value::Text(f.to_string())),
                    (DataType::Utf8, Value::Bool(b)) => Some(Value::Text(b.to_string())),
                    _ => None,
                };
                if let Some(nv) = new_v {
                    *v = nv;
                }
            }
        }
    }
    types
}

fn widen(a: DataType, b: DataType) -> DataType {
    use DataType::*;
    match (a, b) {
        (Null, x) | (x, Null) => x,
        (x, y) if x == y => x,
        (Int64, Float64) | (Float64, Int64) => Float64,
        (Boolean, Int64) | (Int64, Boolean) => Int64,
        (Boolean, Float64) | (Float64, Boolean) => Float64,
        _ => Utf8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_names() {
        let n = normalize_names(vec!["a".into(), "".into(), "a".into()]);
        assert_eq!(n, vec!["a", "column_2", "a_2"]);
    }

    #[test]
    fn test_parse_text_table() {
        let rows = vec![
            vec!["1".to_string(), "1.5".to_string(), "x".to_string()],
            vec!["2".to_string(), "".to_string(), "y".to_string()],
        ];
        let (types, parsed) = parse_text_table(rows, 3);
        assert_eq!(types, vec![DataType::Int64, DataType::Float64, DataType::Utf8]);
        assert_eq!(parsed[0][0], Value::Int(1));
        assert_eq!(parsed[1][1], Value::Null);
    }

    #[test]
    fn test_unify_bool_numeric_mix() {
        // ExcelでTRUE/FALSEと数値が同一列に混在するケース
        let mut rows = vec![
            vec![Value::Bool(true), Value::Bool(false)],
            vec![Value::Int(5), Value::Float(2.5)],
        ];
        let types = unify_columns(&mut rows, 2);
        assert_eq!(types, vec![DataType::Int64, DataType::Float64]);
        assert_eq!(rows[0][0], Value::Int(1));
        assert_eq!(rows[0][1], Value::Float(0.0));
    }

    #[test]
    fn test_unify_columns() {
        let mut rows = vec![
            vec![Value::Int(1), Value::Text("a".into())],
            vec![Value::Float(2.5), Value::Int(3)],
        ];
        let types = unify_columns(&mut rows, 2);
        assert_eq!(types, vec![DataType::Float64, DataType::Utf8]);
        assert_eq!(rows[0][0], Value::Float(1.0));
        assert_eq!(rows[1][1], Value::Text("3".into()));
    }
}
