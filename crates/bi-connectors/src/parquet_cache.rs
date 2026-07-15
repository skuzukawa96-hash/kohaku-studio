//! Parquet キャッシュ(v0.3)。ファイル系ソース(CSV/Excel/SQLite)の読み込み結果を
//! ユーザーのキャッシュディレクトリに Parquet として保存し、ソースが未変更なら
//! 再パースせず高速に復元する。
//!
//! 設計方針:
//! - キャッシュはあくまで高速化。ここでの失敗は呼び出し側が無視して
//!   ソースからの通常読み込みにフォールバックする(ユーザーを止めない)。
//! - 無効化はソースファイルの「サイズ + 更新時刻」の完全一致。少しでも
//!   合わなければキャッシュを使わない(古いデータを見せないことを最優先)。
//! - DB接続(URL)はサーバー側でデータが変わるため対象外。

use arrow_array::builder::{BooleanBuilder, Float64Builder, Int64Builder, StringBuilder};
use arrow_array::{
    Array, ArrayRef, BooleanArray, Float64Array, Int64Array, RecordBatch, StringArray,
};
use arrow_schema::{DataType as ArrowType, Field, Schema};
use bi_core::*;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// キャッシュ形式のバージョン。互換性のない変更をしたら上げる(古いキャッシュは無視される)
const CACHE_FORMAT: &str = "1";
/// 1バッチの行数。メモリピークを抑えるため分割して読み書きする
const BATCH_ROWS: usize = 65_536;

// ---------- キャッシュの置き場所とキー ----------

/// 既定のキャッシュディレクトリ。
/// KOHAKU_CACHE_DIR > %LOCALAPPDATA% > $XDG_CACHE_HOME > $HOME/.cache の順で解決する。
pub fn default_cache_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("KOHAKU_CACHE_DIR") {
        return Some(PathBuf::from(dir));
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("kohaku-studio").join("cache"))
}

/// FNV-1a 64bit。キャッシュキー用の決定的ハッシュ(暗号用途ではない)
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// ソースパス + オブジェクト + インポートオプションからキャッシュファイル名を決める。
/// オプションが違えば取り込まれる内容も違うため、別キャッシュにする。
fn cache_file(root: &Path, path: &Path, object: &str, opts: &ImportOptions) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let key = format!(
        "{}\n{}\n{}\n{}\n{:?}\n{:?}",
        canonical.to_string_lossy(),
        object,
        opts.header_row,
        opts.skip_rows,
        opts.delimiter,
        opts.max_rows,
    );
    root.join(format!("{:016x}.parquet", fnv1a(key.as_bytes())))
}

/// ソースファイルの指紋(サイズ + 更新時刻ミリ秒)。ファイルでなければ None。
fn fingerprint(path: &Path) -> Option<(u64, u128)> {
    let md = std::fs::metadata(path).ok()?;
    if !md.is_file() {
        return None;
    }
    let mtime = md
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    Some((md.len(), mtime))
}

// ---------- 列メタデータ ----------

/// Parquetに埋め込む列情報。enc は値の格納方式:
/// - "native": 列型どおりのArrow配列(通常ケース)
/// - "json": 型推定サンプル外の行に型外れの値が混在する列。
///   無損失で往復させるため各値をJSON文字列として保存する
#[derive(Serialize, Deserialize)]
struct ColMeta {
    name: String,
    #[serde(rename = "type")]
    dtype: String,
    enc: String,
}

fn dtype_from_name(s: &str) -> Option<DataType> {
    Some(match s {
        "null" => DataType::Null,
        "boolean" => DataType::Boolean,
        "integer" => DataType::Int64,
        "real" => DataType::Float64,
        "text" => DataType::Utf8,
        _ => return None,
    })
}

/// 列の全値が宣言型(+Null)に収まっているか。収まらない列は "json" 方式で保存する
fn is_native_column(col_type: DataType, rows: &[Vec<Value>], c: usize) -> bool {
    rows.iter().all(|row| {
        matches!(
            (col_type, row.get(c).unwrap_or(&Value::Null)),
            (_, Value::Null)
                | (DataType::Boolean, Value::Bool(_))
                | (DataType::Int64, Value::Int(_))
                | (DataType::Float64, Value::Float(_))
                | (DataType::Float64, Value::Int(_))
                | (DataType::Utf8, Value::Text(_))
        )
    })
}

fn arrow_type(col_type: DataType, native: bool) -> ArrowType {
    if !native {
        return ArrowType::Utf8;
    }
    match col_type {
        DataType::Boolean => ArrowType::Boolean,
        DataType::Int64 => ArrowType::Int64,
        DataType::Float64 => ArrowType::Float64,
        DataType::Null | DataType::Utf8 => ArrowType::Utf8,
    }
}

// ---------- 書き込み ----------

/// TableData をキャッシュへ保存する。ファイル系ソース以外は何もしない。
/// 失敗してもインポート自体は成功しているため、呼び出し側はエラーを無視してよい。
pub fn store(path: &Path, object: &str, opts: &ImportOptions, td: &TableData) -> BiResult<()> {
    let Some(root) = default_cache_dir() else {
        return Ok(());
    };
    store_at(&root, path, object, opts, td)
}

fn store_at(
    root: &Path,
    path: &Path,
    object: &str,
    opts: &ImportOptions,
    td: &TableData,
) -> BiResult<()> {
    let Some((len, mtime)) = fingerprint(path) else {
        return Ok(()); // DB接続などファイル以外は対象外
    };
    std::fs::create_dir_all(root).map_err(|e| format!("キャッシュディレクトリ作成失敗: {e}"))?;

    let natives: Vec<bool> = (0..td.schema.columns.len())
        .map(|c| is_native_column(td.schema.columns[c].data_type, &td.rows, c))
        .collect();
    let fields: Vec<Field> = td
        .schema
        .columns
        .iter()
        .zip(&natives)
        .map(|(col, &n)| Field::new(&col.name, arrow_type(col.data_type, n), true))
        .collect();
    let schema = Arc::new(Schema::new(fields));

    let col_meta: Vec<ColMeta> = td
        .schema
        .columns
        .iter()
        .zip(&natives)
        .map(|(col, &n)| ColMeta {
            name: col.name.clone(),
            dtype: col.data_type.name().to_string(),
            enc: if n { "native" } else { "json" }.to_string(),
        })
        .collect();
    let meta_json = serde_json::to_string(&col_meta).map_err(|e| e.to_string())?;

    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            KeyValue::new("kohaku:format".to_string(), CACHE_FORMAT.to_string()),
            KeyValue::new("kohaku:source_len".to_string(), len.to_string()),
            KeyValue::new("kohaku:source_mtime_ms".to_string(), mtime.to_string()),
            KeyValue::new("kohaku:columns".to_string(), meta_json),
        ]))
        .build();

    // 途中クラッシュで壊れたキャッシュを残さないよう、一時ファイルに書いてから置き換える
    let dest = cache_file(root, path, object, opts);
    let tmp = dest.with_extension("tmp");
    let file = std::fs::File::create(&tmp).map_err(|e| format!("キャッシュ作成失敗: {e}"))?;
    let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(props))
        .map_err(|e| format!("Parquetライター初期化失敗: {e}"))?;
    for chunk in td.rows.chunks(BATCH_ROWS) {
        let batch = build_batch(&schema, &td.schema, &natives, chunk)?;
        writer
            .write(&batch)
            .map_err(|e| format!("Parquet書き込み失敗: {e}"))?;
    }
    writer
        .close()
        .map_err(|e| format!("Parquetクローズ失敗: {e}"))?;
    let _ = std::fs::remove_file(&dest); // Windowsのrenameは上書き不可のため先に消す
    std::fs::rename(&tmp, &dest).map_err(|e| format!("キャッシュ置き換え失敗: {e}"))?;
    Ok(())
}

fn build_batch(
    schema: &Arc<Schema>,
    ts: &TableSchema,
    natives: &[bool],
    chunk: &[Vec<Value>],
) -> BiResult<RecordBatch> {
    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(ts.columns.len());
    for (c, col) in ts.columns.iter().enumerate() {
        let cell = |row: &Vec<Value>| row.get(c).cloned().unwrap_or(Value::Null);
        let arr: ArrayRef = if !natives[c] {
            // 混在列: 各値をJSONとして無損失に文字列化(例: 5 / 2.5 / "abc" / true)
            let mut b = StringBuilder::new();
            for row in chunk {
                match cell(row) {
                    Value::Null => b.append_null(),
                    // NaN等JSON化できない値はNullとして保存する(実データではほぼ出ない)
                    v => match serde_json::to_string(&v) {
                        Ok(s) => b.append_value(s),
                        Err(_) => b.append_null(),
                    },
                }
            }
            Arc::new(b.finish())
        } else {
            match col.data_type {
                DataType::Boolean => {
                    let mut b = BooleanBuilder::with_capacity(chunk.len());
                    for row in chunk {
                        match cell(row) {
                            Value::Bool(v) => b.append_value(v),
                            _ => b.append_null(),
                        }
                    }
                    Arc::new(b.finish())
                }
                DataType::Int64 => {
                    let mut b = Int64Builder::with_capacity(chunk.len());
                    for row in chunk {
                        match cell(row) {
                            Value::Int(v) => b.append_value(v),
                            _ => b.append_null(),
                        }
                    }
                    Arc::new(b.finish())
                }
                DataType::Float64 => {
                    let mut b = Float64Builder::with_capacity(chunk.len());
                    for row in chunk {
                        match cell(row) {
                            Value::Float(v) => b.append_value(v),
                            Value::Int(v) => b.append_value(v as f64),
                            _ => b.append_null(),
                        }
                    }
                    Arc::new(b.finish())
                }
                DataType::Null | DataType::Utf8 => {
                    let mut b = StringBuilder::new();
                    for row in chunk {
                        match cell(row) {
                            Value::Text(v) => b.append_value(v),
                            _ => b.append_null(),
                        }
                    }
                    Arc::new(b.finish())
                }
            }
        };
        arrays.push(arr);
    }
    RecordBatch::try_new(schema.clone(), arrays).map_err(|e| format!("バッチ構築失敗: {e}"))
}

// ---------- 読み込み ----------

/// 有効なキャッシュがあれば TableData を復元する。
/// キャッシュ無し・ソース変更済み・破損など、使えない理由が何であれ None を返す
/// (呼び出し側はソースからの通常読み込みにフォールバックする)。
pub fn load(path: &Path, object: &str, opts: &ImportOptions) -> Option<TableData> {
    load_at(&default_cache_dir()?, path, object, opts)
}

fn load_at(root: &Path, path: &Path, object: &str, opts: &ImportOptions) -> Option<TableData> {
    let (len, mtime) = fingerprint(path)?;
    let file = std::fs::File::open(cache_file(root, path, object, opts)).ok()?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).ok()?;

    let kv = builder.metadata().file_metadata().key_value_metadata()?;
    let get = |key: &str| {
        kv.iter()
            .find(|e| e.key == key)
            .and_then(|e| e.value.clone())
    };
    if get("kohaku:format")? != CACHE_FORMAT {
        return None;
    }
    // ソースの指紋が完全一致しなければ古いキャッシュとして捨てる
    if get("kohaku:source_len")? != len.to_string()
        || get("kohaku:source_mtime_ms")? != mtime.to_string()
    {
        return None;
    }
    let metas: Vec<ColMeta> = serde_json::from_str(&get("kohaku:columns")?).ok()?;

    let reader = builder.with_batch_size(BATCH_ROWS).build().ok()?;
    let mut rows: Vec<Vec<Value>> = Vec::new();
    for batch in reader {
        append_batch(&mut rows, &batch.ok()?, &metas)?;
    }

    let columns = metas
        .iter()
        .map(|m| {
            Some(ColumnSchema {
                name: m.name.clone(),
                data_type: dtype_from_name(&m.dtype)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(TableData {
        schema: TableSchema { columns },
        rows,
    })
}

fn append_batch(rows: &mut Vec<Vec<Value>>, batch: &RecordBatch, metas: &[ColMeta]) -> Option<()> {
    if batch.num_columns() != metas.len() {
        return None;
    }
    let n = batch.num_rows();
    let ncols = metas.len();
    let start = rows.len();
    rows.resize_with(start + n, || Vec::with_capacity(ncols));

    for (c, meta) in metas.iter().enumerate() {
        let arr = batch.column(c);
        for i in 0..n {
            let v = if arr.is_null(i) {
                Value::Null
            } else if meta.enc == "json" {
                let s = arr.as_any().downcast_ref::<StringArray>()?.value(i);
                serde_json::from_str::<Value>(s).ok()?
            } else {
                match dtype_from_name(&meta.dtype)? {
                    DataType::Boolean => {
                        Value::Bool(arr.as_any().downcast_ref::<BooleanArray>()?.value(i))
                    }
                    DataType::Int64 => {
                        Value::Int(arr.as_any().downcast_ref::<Int64Array>()?.value(i))
                    }
                    DataType::Float64 => {
                        Value::Float(arr.as_any().downcast_ref::<Float64Array>()?.value(i))
                    }
                    DataType::Null | DataType::Utf8 => Value::Text(
                        arr.as_any()
                            .downcast_ref::<StringArray>()?
                            .value(i)
                            .to_string(),
                    ),
                }
            };
            rows[start + i].push(v);
        }
    }
    Some(())
}

// ---------- テスト ----------

#[cfg(test)]
mod tests {
    use super::*;

    /// テストごとに独立した一時ディレクトリ(キャッシュ用+ソース用)を作る
    fn temp_dirs(name: &str) -> (PathBuf, PathBuf) {
        let base =
            std::env::temp_dir().join(format!("kohaku-pq-test-{}-{}", std::process::id(), name));
        let cache = base.join("cache");
        let src = base.join("src");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::create_dir_all(&src).unwrap();
        (cache, src)
    }

    fn sample_table() -> TableData {
        TableData {
            schema: TableSchema {
                columns: vec![
                    ColumnSchema {
                        name: "id".into(),
                        data_type: DataType::Int64,
                    },
                    ColumnSchema {
                        name: "score".into(),
                        data_type: DataType::Float64,
                    },
                    ColumnSchema {
                        name: "商品名".into(),
                        data_type: DataType::Utf8,
                    },
                    ColumnSchema {
                        name: "flag".into(),
                        data_type: DataType::Boolean,
                    },
                ],
            },
            rows: vec![
                vec![
                    Value::Int(1),
                    Value::Float(0.5),
                    Value::Text("コーヒー豆".into()),
                    Value::Bool(true),
                ],
                vec![Value::Null, Value::Null, Value::Null, Value::Null],
                vec![
                    Value::Int(-42),
                    Value::Float(1e15),
                    Value::Text("".into()),
                    Value::Bool(false),
                ],
            ],
        }
    }

    fn assert_same(a: &TableData, b: &TableData) {
        assert_eq!(a.schema.columns.len(), b.schema.columns.len());
        for (x, y) in a.schema.columns.iter().zip(&b.schema.columns) {
            assert_eq!(x.name, y.name);
            assert_eq!(x.data_type, y.data_type);
        }
        assert_eq!(a.rows, b.rows);
    }

    #[test]
    fn test_roundtrip() {
        let (cache, src) = temp_dirs("roundtrip");
        let source = src.join("data.csv");
        std::fs::write(&source, "dummy").unwrap();
        let td = sample_table();
        let opts = ImportOptions::default();
        store_at(&cache, &source, "data", &opts, &td).unwrap();
        let loaded = load_at(&cache, &source, "data", &opts).expect("キャッシュヒットするはず");
        assert_same(&td, &loaded);
    }

    #[test]
    fn test_mixed_column_roundtrip() {
        // 型推定サンプル外で Int64 列に文字列が混在したケース(json方式で無損失)
        let (cache, src) = temp_dirs("mixed");
        let source = src.join("data.csv");
        std::fs::write(&source, "dummy").unwrap();
        let td = TableData {
            schema: TableSchema {
                columns: vec![ColumnSchema {
                    name: "v".into(),
                    data_type: DataType::Int64,
                }],
            },
            rows: vec![
                vec![Value::Int(5)],
                vec![Value::Text("N/A".into())],
                vec![Value::Null],
                vec![Value::Float(2.5)],
            ],
        };
        let opts = ImportOptions::default();
        store_at(&cache, &source, "data", &opts, &td).unwrap();
        let loaded = load_at(&cache, &source, "data", &opts).unwrap();
        assert_same(&td, &loaded);
    }

    #[test]
    fn test_invalidated_when_source_changes() {
        let (cache, src) = temp_dirs("invalidate");
        let source = src.join("data.csv");
        std::fs::write(&source, "before").unwrap();
        let opts = ImportOptions::default();
        store_at(&cache, &source, "data", &opts, &sample_table()).unwrap();
        assert!(load_at(&cache, &source, "data", &opts).is_some());
        // ソースを書き換える(サイズも変える)とキャッシュは無効になる
        std::fs::write(&source, "after -- longer content").unwrap();
        assert!(load_at(&cache, &source, "data", &opts).is_none());
    }

    #[test]
    fn test_key_depends_on_options() {
        let (cache, src) = temp_dirs("key");
        let source = src.join("data.csv");
        std::fs::write(&source, "dummy").unwrap();
        let a = ImportOptions::default();
        let b = ImportOptions {
            header_row: 0,
            ..ImportOptions::default()
        };
        assert_ne!(
            cache_file(&cache, &source, "data", &a),
            cache_file(&cache, &source, "data", &b)
        );
        // オプションAで保存したキャッシュはオプションBではヒットしない
        store_at(&cache, &source, "data", &a, &sample_table()).unwrap();
        assert!(load_at(&cache, &source, "data", &b).is_none());
    }

    #[test]
    fn test_non_file_source_is_skipped() {
        let (cache, _src) = temp_dirs("nonfile");
        let url = PathBuf::from("mysql://kohaku@localhost:3306/demo");
        let opts = ImportOptions::default();
        // DB接続URLは対象外: 保存は黙って成功扱い、読み込みは常にミス
        store_at(&cache, &url, "sales", &opts, &sample_table()).unwrap();
        assert!(load_at(&cache, &url, "sales", &opts).is_none());
    }

    #[test]
    fn test_empty_table_roundtrip() {
        let (cache, src) = temp_dirs("empty");
        let source = src.join("data.csv");
        std::fs::write(&source, "dummy").unwrap();
        let td = TableData {
            schema: TableSchema {
                columns: vec![ColumnSchema {
                    name: "a".into(),
                    data_type: DataType::Utf8,
                }],
            },
            rows: vec![],
        };
        let opts = ImportOptions::default();
        store_at(&cache, &source, "data", &opts, &td).unwrap();
        let loaded = load_at(&cache, &source, "data", &opts).unwrap();
        assert_same(&td, &loaded);
    }
}
