//! PostgreSQL / MySQL コネクタ(sqlx)。
//! path には接続URL(例: postgres://user:pass@localhost:5432/db)を渡す。
//! 同期の Connector trait に合わせ、呼び出しごとに小さな
//! current-thread ランタイムで block_on する(常駐スレッドを持たない)。
//!
//! 注意: 接続URLはデータセット定義としてプロジェクトファイルに平文保存される。
//! 共有環境では読み取り専用ユーザーの使用を推奨。

use bi_core::*;
use rust_decimal::prelude::ToPrimitive;
use sqlx::{Column, Executor, Row, Statement, TypeInfo};
use std::path::Path;
use std::time::Duration;

/// 接続確立のタイムアウト
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

fn runtime() -> BiResult<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("非同期ランタイムの初期化に失敗: {e}"))
}

fn url_of(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn dec_to_f64(v: rust_decimal::Decimal) -> f64 {
    v.to_f64().unwrap_or(f64::NAN)
}

/// f32→f64。単純キャストは単精度の誤差を露出させる(0.1→0.10000000149…)ため、
/// 最短往復表現(Display)を経由して 0.1 のまま f64 化する。
fn f32_to_f64(v: f32) -> f64 {
    format!("{v}").parse().unwrap_or(v as f64)
}

/// 行データから TableData を組み立てる共通処理
fn build_table(names: Vec<String>, mut rows: Vec<Vec<Value>>) -> TableData {
    let ncols = names.len();
    let types = unify_columns(&mut rows, ncols);
    let columns = normalize_names(names)
        .into_iter()
        .zip(types)
        .map(|(name, data_type)| ColumnSchema { name, data_type })
        .collect();
    TableData {
        schema: TableSchema { columns },
        rows,
    }
}

// ---------- PostgreSQL ----------

pub struct PostgresConnector;

async fn pg_pool(url: &str) -> BiResult<sqlx::PgPool> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(CONNECT_TIMEOUT)
        .connect(url)
        .await
        .map_err(|e| format!("PostgreSQLに接続できません: {e}"))
}

/// PostgreSQLの1セルを Value へ変換(型名ベース、失敗時は文字列へフォールバック)
fn pg_value(row: &sqlx::postgres::PgRow, i: usize) -> Value {
    let tname = row.column(i).type_info().name().to_uppercase();
    macro_rules! take {
        ($t:ty, $conv:expr) => {
            match row.try_get::<Option<$t>, _>(i) {
                Ok(Some(v)) => return $conv(v),
                Ok(None) => return Value::Null,
                Err(_) => {}
            }
        };
    }
    match tname.as_str() {
        "INT2" => take!(i16, |v: i16| Value::Int(v as i64)),
        "INT4" => take!(i32, |v: i32| Value::Int(v as i64)),
        "INT8" => take!(i64, Value::Int),
        "FLOAT4" => take!(f32, |v: f32| Value::Float(f32_to_f64(v))),
        "FLOAT8" => take!(f64, Value::Float),
        "NUMERIC" => take!(rust_decimal::Decimal, |v| Value::Float(dec_to_f64(v))),
        "BOOL" => take!(bool, Value::Bool),
        "DATE" => take!(chrono::NaiveDate, |v: chrono::NaiveDate| Value::Text(
            v.to_string()
        )),
        "TIME" => take!(chrono::NaiveTime, |v: chrono::NaiveTime| Value::Text(
            v.to_string()
        )),
        "TIMESTAMP" => take!(chrono::NaiveDateTime, |v: chrono::NaiveDateTime| {
            Value::Text(v.format("%Y-%m-%d %H:%M:%S").to_string())
        }),
        "TIMESTAMPTZ" => take!(chrono::DateTime<chrono::Utc>, |v: chrono::DateTime<
            chrono::Utc,
        >| {
            Value::Text(v.format("%Y-%m-%d %H:%M:%S%:z").to_string())
        }),
        _ => {}
    }
    take!(String, Value::Text);
    Value::Text(format!("[{tname}]"))
}

impl Connector for PostgresConnector {
    fn connector_type(&self) -> &'static str {
        "postgres"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[]
    }

    fn schemes(&self) -> &'static [&'static str] {
        &["postgres", "postgresql"]
    }

    fn list_objects(&self, path: &Path) -> BiResult<Vec<String>> {
        let url = url_of(path);
        runtime()?.block_on(async {
            let pool = pg_pool(&url).await?;
            let rows = sqlx::query(
                "SELECT table_name FROM information_schema.tables \
                 WHERE table_schema = 'public' AND table_type IN ('BASE TABLE', 'VIEW') \
                 ORDER BY table_name",
            )
            .fetch_all(&pool)
            .await
            .map_err(|e| format!("テーブル一覧の取得に失敗: {e}"))?;
            Ok(rows.iter().map(|r| r.get::<String, _>(0)).collect())
        })
    }

    fn load(&self, path: &Path, object: &str, opts: &ImportOptions) -> BiResult<TableData> {
        let url = url_of(path);
        let ident = object.replace('"', "\"\"");
        let sql = match opts.max_rows {
            Some(n) => format!("SELECT * FROM \"{ident}\" LIMIT {n}"),
            None => format!("SELECT * FROM \"{ident}\""),
        };
        runtime()?.block_on(async {
            let pool = pg_pool(&url).await?;
            // prepareで列名を先に確定させる(0行のテーブルでもスキーマを得るため)
            let stmt = pool
                .prepare(sql.as_str())
                .await
                .map_err(|e| format!("テーブル「{object}」を読み込めません: {e}"))?;
            let names: Vec<String> = stmt
                .columns()
                .iter()
                .map(|c| c.name().to_string())
                .collect();
            let db_rows = sqlx::query(&sql)
                .fetch_all(&pool)
                .await
                .map_err(|e| format!("テーブル「{object}」の読み込みに失敗: {e}"))?;
            let rows: Vec<Vec<Value>> = db_rows
                .iter()
                .map(|r| (0..names.len()).map(|i| pg_value(r, i)).collect())
                .collect();
            Ok(build_table(names, rows))
        })
    }
}

// ---------- MySQL ----------

pub struct MySqlConnector;

async fn my_pool(url: &str) -> BiResult<sqlx::MySqlPool> {
    // mariadb:// スキームも受け付ける(ドライバはmysql://のみ解釈する)
    let url = url
        .strip_prefix("mariadb://")
        .map(|rest| format!("mysql://{rest}"))
        .unwrap_or_else(|| url.to_string());
    sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(CONNECT_TIMEOUT)
        .connect(&url)
        .await
        .map_err(|e| format!("MySQLに接続できません: {e}"))
}

/// MySQLの1セルを Value へ変換(型名ベース、失敗時は文字列へフォールバック)
fn my_value(row: &sqlx::mysql::MySqlRow, i: usize) -> Value {
    let tname = row.column(i).type_info().name().to_uppercase();
    macro_rules! take {
        ($t:ty, $conv:expr) => {
            match row.try_get::<Option<$t>, _>(i) {
                Ok(Some(v)) => return $conv(v),
                Ok(None) => return Value::Null,
                Err(_) => {}
            }
        };
    }
    match tname.as_str() {
        "BOOLEAN" => take!(bool, Value::Bool),
        "TINYINT" => take!(i8, |v: i8| Value::Int(v as i64)),
        "SMALLINT" => take!(i16, |v: i16| Value::Int(v as i64)),
        "INT" | "MEDIUMINT" => take!(i32, |v: i32| Value::Int(v as i64)),
        "BIGINT" => take!(i64, Value::Int),
        "TINYINT UNSIGNED" => take!(u8, |v: u8| Value::Int(v as i64)),
        "SMALLINT UNSIGNED" => take!(u16, |v: u16| Value::Int(v as i64)),
        "INT UNSIGNED" | "MEDIUMINT UNSIGNED" => take!(u32, |v: u32| Value::Int(v as i64)),
        "BIGINT UNSIGNED" => take!(u64, |v: u64| match i64::try_from(v) {
            Ok(i) => Value::Int(i),
            Err(_) => Value::Float(v as f64), // i64を超える値は精度を落として保持
        }),
        "YEAR" => take!(u16, |v: u16| Value::Int(v as i64)),
        "FLOAT" => take!(f32, |v: f32| Value::Float(f32_to_f64(v))),
        "DOUBLE" => take!(f64, Value::Float),
        "DECIMAL" => take!(rust_decimal::Decimal, |v| Value::Float(dec_to_f64(v))),
        "DATE" => take!(chrono::NaiveDate, |v: chrono::NaiveDate| Value::Text(
            v.to_string()
        )),
        "TIME" => take!(chrono::NaiveTime, |v: chrono::NaiveTime| Value::Text(
            v.to_string()
        )),
        "DATETIME" => take!(chrono::NaiveDateTime, |v: chrono::NaiveDateTime| {
            Value::Text(v.format("%Y-%m-%d %H:%M:%S").to_string())
        }),
        "TIMESTAMP" => take!(chrono::DateTime<chrono::Utc>, |v: chrono::DateTime<
            chrono::Utc,
        >| {
            Value::Text(v.format("%Y-%m-%d %H:%M:%S%:z").to_string())
        }),
        _ => {}
    }
    take!(String, Value::Text);
    Value::Text(format!("[{tname}]"))
}

impl Connector for MySqlConnector {
    fn connector_type(&self) -> &'static str {
        "mysql"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &[]
    }

    fn schemes(&self) -> &'static [&'static str] {
        &["mysql", "mariadb"]
    }

    fn list_objects(&self, path: &Path) -> BiResult<Vec<String>> {
        let url = url_of(path);
        runtime()?.block_on(async {
            let pool = my_pool(&url).await?;
            let rows = sqlx::query(
                "SELECT table_name FROM information_schema.tables \
                 WHERE table_schema = DATABASE() ORDER BY table_name",
            )
            .fetch_all(&pool)
            .await
            .map_err(|e| format!("テーブル一覧の取得に失敗: {e}"))?;
            Ok(rows.iter().map(|r| r.get::<String, _>(0)).collect())
        })
    }

    fn load(&self, path: &Path, object: &str, opts: &ImportOptions) -> BiResult<TableData> {
        let url = url_of(path);
        let ident = object.replace('`', "``");
        let sql = match opts.max_rows {
            Some(n) => format!("SELECT * FROM `{ident}` LIMIT {n}"),
            None => format!("SELECT * FROM `{ident}`"),
        };
        runtime()?.block_on(async {
            let pool = my_pool(&url).await?;
            let stmt = pool
                .prepare(sql.as_str())
                .await
                .map_err(|e| format!("テーブル「{object}」を読み込めません: {e}"))?;
            let names: Vec<String> = stmt
                .columns()
                .iter()
                .map(|c| c.name().to_string())
                .collect();
            let db_rows = sqlx::query(&sql)
                .fetch_all(&pool)
                .await
                .map_err(|e| format!("テーブル「{object}」の読み込みに失敗: {e}"))?;
            let rows: Vec<Vec<Value>> = db_rows
                .iter()
                .map(|r| (0..names.len()).map(|i| my_value(r, i)).collect())
                .collect();
            Ok(build_table(names, rows))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実DBが必要なテスト。環境変数が無ければスキップする。
    /// 例: KOHAKU_TEST_PG_URL=postgres://kohaku:kohaku@localhost:5432/demo
    #[test]
    fn test_postgres_live() {
        let Ok(url) = std::env::var("KOHAKU_TEST_PG_URL") else {
            eprintln!("KOHAKU_TEST_PG_URL 未設定のためスキップ");
            return;
        };
        let p = std::path::PathBuf::from(&url);
        let objs = PostgresConnector.list_objects(&p).unwrap();
        assert!(objs.contains(&"sales".to_string()));
        let td = PostgresConnector
            .load(&p, "sales", &ImportOptions::default())
            .unwrap();
        assert_eq!(td.rows.len(), 8);
        // quantity は整数、unit_price(numeric) は浮動小数として取り込まれる
        let qi = td
            .schema
            .columns
            .iter()
            .position(|c| c.name == "quantity")
            .unwrap();
        assert_eq!(td.schema.columns[qi].data_type, DataType::Int64);
    }

    #[test]
    fn test_mysql_live() {
        let Ok(url) = std::env::var("KOHAKU_TEST_MYSQL_URL") else {
            eprintln!("KOHAKU_TEST_MYSQL_URL 未設定のためスキップ");
            return;
        };
        let p = std::path::PathBuf::from(&url);
        let objs = MySqlConnector.list_objects(&p).unwrap();
        assert!(objs.contains(&"sales".to_string()));
        let td = MySqlConnector
            .load(&p, "sales", &ImportOptions::default())
            .unwrap();
        assert_eq!(td.rows.len(), 8);
    }
}
