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

/// 日時の文字列化フォーマット。`%.f` は小数部が0のときは何も出さないので、
/// 秒未満を持たない既存データの表記は変わらず、持つデータだけ精度が残る
/// (秒で切り捨てると、イベント時刻での並べ替えやJOINが静かに壊れる)。
const TS_FMT: &str = "%Y-%m-%d %H:%M:%S%.f";
const TSTZ_FMT: &str = "%Y-%m-%d %H:%M:%S%.f%:z";

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
            Value::Text(v.format(TS_FMT).to_string())
        }),
        "TIMESTAMPTZ" => take!(chrono::DateTime<chrono::Utc>, |v: chrono::DateTime<
            chrono::Utc,
        >| {
            Value::Text(v.format(TSTZ_FMT).to_string())
        }),
        _ => {}
    }
    take!(String, Value::Text);
    Value::Text(format!("[{tname}]"))
}

/// PostgreSQLの識別子を、必要なときだけクオートして表示名にする。
/// '.' を含む名前は必ずクオートされるので、`schema.table` の区切りの '.' と
/// 名前の中の '.' を取り違えずに済む(表示名がそのままSQLの参照になる)。
fn pg_quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// クオートが要らない単純な名前か(小文字・数字・下線のみで、数字始まりでない)。
/// PostgreSQL はクオートしない識別子を小文字に畳むので、この条件のときだけ
/// 素のまま表示してよい。
fn pg_ident_is_simple(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// 一覧に出す表示名。public は素のテーブル名、それ以外は schema.table。
/// クオートが必要な要素だけクオートする。
fn pg_display_name(schema: &str, table: &str) -> String {
    let t = if pg_ident_is_simple(table) {
        table.to_string()
    } else {
        pg_quote_ident(table)
    };
    if schema == "public" {
        return t;
    }
    let s = if pg_ident_is_simple(schema) {
        schema.to_string()
    } else {
        pg_quote_ident(schema)
    };
    format!("{s}.{t}")
}

/// 表示名を識別子の並びへ分解する。クオート内の '.' は区切りとして扱わない。
/// 形が崩れていたり3要素以上なら None(呼び出し側が名前全体として扱う)。
fn pg_split_ident(object: &str) -> Option<Vec<String>> {
    let mut parts: Vec<String> = Vec::new();
    let mut rest = object;
    loop {
        let part = if let Some(after) = rest.strip_prefix('"') {
            let mut out = String::new();
            let mut chars = after.char_indices();
            let end;
            loop {
                match chars.next() {
                    Some((k, '"')) => {
                        if after[k + 1..].starts_with('"') {
                            out.push('"');
                            chars.next();
                        } else {
                            end = k + 1;
                            break;
                        }
                    }
                    Some((_, c)) => out.push(c),
                    None => return None, // 閉じクオートが無い
                }
            }
            rest = &after[end..];
            out
        } else {
            let end = rest.find('.').unwrap_or(rest.len());
            let out = rest[..end].to_string();
            rest = &rest[end..];
            out
        };
        if part.is_empty() {
            return None;
        }
        parts.push(part);
        match rest.strip_prefix('.') {
            Some(r) => rest = r,
            None => break,
        }
    }
    if rest.is_empty() && (1..=2).contains(&parts.len()) {
        Some(parts)
    } else {
        None
    }
}

/// SQLに埋め込む参照の候補を優先順に作る。
/// 1つ目は表示名を識別子として解釈したもの。2つ目は名前全体を1つの識別子と
/// みなしたもので、この形で保存された古いプロジェクトや手入力のために残す。
fn pg_table_refs(object: &str) -> Vec<String> {
    let whole = pg_quote_ident(object);
    match pg_split_ident(object) {
        Some(parts) => {
            let joined = parts
                .iter()
                .map(|p| pg_quote_ident(p))
                .collect::<Vec<_>>()
                .join(".");
            // 名前全体を1つの識別子とみなす解釈も候補に残す。ただしクオートを
            // 含む表示名は新しい一覧が作ったものなので解釈は一意に決まる。
            // (残すのは、修飾なしの `fab.events` で保存された古いプロジェクトのため)
            if parts.len() > 1 && !object.contains('"') {
                vec![joined, whole]
            } else {
                vec![joined]
            }
        }
        None => vec![whole],
    }
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
            // public 以外のスキーマに置かれたテーブルも一覧に出す。
            // information_schema.tables はアクセス権のあるものだけを返すので、
            // システムスキーマ2つを除けば「その利用者に見えるもの」になる。
            // public はこれまでどおり素の名前で返し(保存済みプロジェクトとの
            // 互換のため)、それ以外は schema.table の形にする
            let rows = sqlx::query(
                "SELECT table_schema, table_name FROM information_schema.tables \
                 WHERE table_schema NOT IN ('pg_catalog', 'information_schema') \
                 AND table_type IN ('BASE TABLE', 'VIEW') \
                 ORDER BY table_schema <> 'public', table_schema, table_name",
            )
            .fetch_all(&pool)
            .await
            .map_err(|e| format!("テーブル一覧の取得に失敗: {e}"))?;
            Ok(rows
                .iter()
                .map(|r| pg_display_name(&r.get::<String, _>(0), &r.get::<String, _>(1)))
                .collect())
        })
    }

    fn load(&self, path: &Path, object: &str, opts: &ImportOptions) -> BiResult<TableData> {
        let url = url_of(path);
        let limit = match opts.max_rows {
            Some(n) => format!(" LIMIT {n}"),
            None => String::new(),
        };
        // 候補のSQLは async ブロックの外に置く(prepare が返す文は
        // 文字列を借りるため、ブロック内のローカルだと生存期間が足りない)
        let sqls: Vec<String> = pg_table_refs(object)
            .iter()
            .map(|r| format!("SELECT * FROM {r}{limit}"))
            .collect();
        runtime()?.block_on(async {
            let pool = pg_pool(&url).await?;
            // prepareで列名を先に確定させる(0行のテーブルでもスキーマを得るため)。
            // 参照の候補を順に試し、最初に通ったものを使う
            let mut last_err = String::new();
            let mut names: Vec<String> = Vec::new();
            let mut chosen: Option<&str> = None;
            for sql in &sqls {
                match pool.prepare(sql.as_str()).await {
                    Ok(stmt) => {
                        names = stmt
                            .columns()
                            .iter()
                            .map(|c| c.name().to_string())
                            .collect();
                        chosen = Some(sql.as_str());
                        break;
                    }
                    Err(e) => last_err = e.to_string(),
                }
            }
            let Some(sql) = chosen else {
                return Err(format!("テーブル「{object}」を読み込めません: {last_err}"));
            };
            let db_rows = sqlx::query(sql)
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
            Value::Text(v.format(TS_FMT).to_string())
        }),
        "TIMESTAMP" => take!(chrono::DateTime<chrono::Utc>, |v: chrono::DateTime<
            chrono::Utc,
        >| {
            Value::Text(v.format(TSTZ_FMT).to_string())
        }),
        _ => {}
    }
    take!(String, Value::Text);
    // バイナリ照合(utf8mb4_bin等)の文字列列は VARBINARY として返り、String で
    // 直読できないためバイト列経由で読む。ただし本物のバイナリを壊さないよう、
    // 型名と中身の両方で絞る:
    //   - BLOB系(BLOB/TINYBLOB/...)は本物のバイナリ列にしか現れないので、
    //     中身がたまたまUTF-8として妥当でも文字列にしない
    //   - VARBINARY/BINARY は本物のバイナリ列と、バイナリ照合の文字列列の
    //     どちらもこの型名で返るためMySQL側では区別できない。UTF-8として
    //     妥当なときだけ文字列にし、それ以外はプレースホルダに落とす
    // NULL 判定は型に関係なく先に済ませる(NULL を "[BLOB]" にしないため)
    match row.try_get::<Option<Vec<u8>>, _>(i) {
        Ok(None) => return Value::Null,
        Ok(Some(v)) => {
            if matches!(tname.as_str(), "VARBINARY" | "BINARY") {
                if let Ok(s) = String::from_utf8(v) {
                    return Value::Text(s);
                }
            }
        }
        Err(_) => {}
    }
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
            // 新しめのMySQL 8.0.xは information_schema の文字列列を
            // バイナリ照合で返し String 直読が VARBINARY エラーになるため、
            // 失敗時はバイト列経由でフォールバックする
            Ok(rows
                .iter()
                .map(|r| match r.try_get::<String, _>(0) {
                    Ok(s) => s,
                    Err(_) => String::from_utf8_lossy(&r.get::<Vec<u8>, _>(0)).into_owned(),
                })
                .collect())
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
    /// 参照候補の作られ方。public 以外は "schema"."table" と解釈しつつ、
    /// 名前に '.' を含むテーブル向けに従来どおりの候補も残す
    /// 表示名の作られ方。'.' を含む名前は必ずクオートされるので、
    /// public."fab.events" と fab.events を取り違えない
    #[test]
    fn test_pg_display_name() {
        assert_eq!(pg_display_name("public", "sales"), "sales");
        assert_eq!(pg_display_name("fab", "events"), "fab.events");
        // public にある '.' 入りの名前はクオートされ、スキーマ修飾と区別できる
        assert_eq!(pg_display_name("public", "fab.events"), r#""fab.events""#);
        // 大文字や記号を含む名前もクオートする(PostgreSQLは無クオートを小文字化する)
        assert_eq!(pg_display_name("public", "Sales"), r#""Sales""#);
        assert_eq!(pg_display_name("my.schema", "t"), r#""my.schema".t"#);
        assert_eq!(pg_display_name("public", "1st"), r#""1st""#);
    }

    /// 表示名 → SQLの参照。クオート内の '.' は区切りにしない
    #[test]
    fn test_pg_table_refs() {
        // 単純名は従来どおり1候補だけ
        assert_eq!(pg_table_refs("sales"), vec![r#""sales""#]);
        // スキーマ修飾。古いプロジェクト用に名前全体の解釈も残す
        assert_eq!(
            pg_table_refs("fab.events"),
            vec![r#""fab"."events""#, r#""fab.events""#]
        );
        // クオート済みの '.' 入り名前は1つの識別子として解釈する
        assert_eq!(pg_table_refs(r#""fab.events""#), vec![r#""fab.events""#]);
        // クオート内の二重引用符はエスケープされたまま復元される
        assert_eq!(pg_table_refs(r#""a""b""#), vec![r#""a""b""#]);
        assert_eq!(
            pg_table_refs(r#""my.schema".t"#),
            vec![r#""my.schema"."t""#]
        );
        // 壊れた形(閉じクオート無し・3要素)は名前全体として扱う
        assert_eq!(pg_table_refs(r#""abc"#), vec![r#""""abc""#]);
        assert_eq!(pg_table_refs("a.b.c"), vec![r#""a.b.c""#]);
        assert_eq!(pg_table_refs(".x"), vec![r#"".x""#]);
        assert_eq!(pg_table_refs("x."), vec![r#""x.""#]);
    }

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

        // public 以外のスキーマも一覧に出て、schema.table で読み込める
        assert!(objs.contains(&"fab.events".to_string()), "{objs:?}");
        let ev = PostgresConnector
            .load(&p, "fab.events", &ImportOptions::default())
            .unwrap();
        assert_eq!(ev.rows.len(), 2);
        // 秒未満が切り捨てられない。持たない行には余計な .000 も付かない
        assert_eq!(
            ev.rows[0][1],
            Value::Text("2026-03-01 08:15:30.123456".into())
        );
        assert_eq!(ev.rows[1][1], Value::Text("2026-03-01 08:15:31".into()));
        assert!(
            matches!(&ev.rows[0][2], Value::Text(t) if t.contains(".123456")),
            "{:?}",
            ev.rows[0][2]
        );
        // 存在しないテーブルは候補を試し切ってからエラーになる
        assert!(PostgresConnector
            .load(&p, "fab.nope", &ImportOptions::default())
            .is_err());

        // public."fab.events" と fab.events が同時にあっても取り違えない。
        // 一覧では '.' 入りの名前がクオートされ、別々の項目として出る
        let dotted = r#""fab.events""#;
        assert!(objs.contains(&dotted.to_string()), "{objs:?}");
        let pub_tbl = PostgresConnector
            .load(&p, dotted, &ImportOptions::default())
            .unwrap();
        assert_eq!(pub_tbl.schema.columns[1].name, "note");
        assert_eq!(pub_tbl.rows.len(), 1);
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

        let bt = MySqlConnector
            .load(&p, "binary_types", &ImportOptions::default())
            .unwrap();
        assert_eq!(bt.rows.len(), 3);
        // 本物のBLOBは置換文字だらけの文字列にせず、プレースホルダのままにする
        assert_eq!(bt.rows[0][1], Value::Text("[BLOB]".into()));
        assert_eq!(bt.rows[1][1], Value::Text("[BLOB]".into()));
        // バイナリ照合の文字列列はこれまでどおり読める
        assert_eq!(bt.rows[0][2], Value::Text("照合がバイナリの文字列".into()));
        // 秒未満が切り捨てられない
        assert_eq!(
            bt.rows[0][3],
            Value::Text("2026-03-01 08:15:30.123456".into())
        );
        assert_eq!(bt.rows[1][3], Value::Text("2026-03-01 08:15:31".into()));
        // 中身がUTF-8として妥当なBLOBも文字列にしない(型名で判断している)
        assert_eq!(bt.rows[2][1], Value::Text("[BLOB]".into()));
    }
}
