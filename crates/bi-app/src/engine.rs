//! クエリエンジン: SQLite in-memory データベース。
//! すべてのデータセットはテーブルとして登録され、SQLで横断的に集計できる。
//! (低スペックPC向け: DataFusionの代わりにSQLiteを採用。省メモリで起動即応)

use bi_core::*;
use rusqlite::types::ValueRef;
use rusqlite::Connection;
use serde::Serialize;

pub struct Engine {
    conn: Connection,
}

#[derive(Debug, Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub truncated: bool,
    pub total_returned: usize,
}

impl Engine {
    pub fn new() -> BiResult<Engine> {
        let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
        // 速度と省メモリのバランス:
        // - 一時領域(GROUP BY等のソート)はメモリ
        // - ページキャッシュは16MBに制限
        // - ソートはワーカースレッド最大4本で並列化
        conn.execute_batch(
            "PRAGMA temp_store = MEMORY; PRAGMA cache_size = -16000; PRAGMA threads = 4;",
        )
        .map_err(|e| e.to_string())?;
        Ok(Engine { conn })
    }

    /// データセットをテーブルとして登録(既存なら置き換え)
    pub fn register(&mut self, name: &str, data: &TableData) -> BiResult<()> {
        validate_name(name)?;
        let qname = quote_ident(name);
        let cols: Vec<String> = data
            .schema
            .columns
            .iter()
            .map(|c| format!("{} {}", quote_ident(&c.name), c.data_type.sqlite_type()))
            .collect();
        self.conn
            .execute_batch(&format!("DROP TABLE IF EXISTS {qname}"))
            .map_err(|e| e.to_string())?;
        self.conn
            .execute_batch(&format!("CREATE TABLE {qname} ({})", cols.join(", ")))
            .map_err(|e| e.to_string())?;

        let placeholders: Vec<&str> = (0..data.schema.columns.len()).map(|_| "?").collect();
        let insert_sql = format!("INSERT INTO {qname} VALUES ({})", placeholders.join(","));
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        {
            let mut stmt = tx.prepare(&insert_sql).map_err(|e| e.to_string())?;
            for row in &data.rows {
                for (i, v) in row.iter().enumerate() {
                    let p = match v {
                        Value::Null => stmt.raw_bind_parameter(i + 1, rusqlite::types::Null),
                        Value::Bool(b) => stmt.raw_bind_parameter(i + 1, *b as i64),
                        Value::Int(x) => stmt.raw_bind_parameter(i + 1, *x),
                        Value::Float(x) => stmt.raw_bind_parameter(i + 1, *x),
                        Value::Text(s) => stmt.raw_bind_parameter(i + 1, s.as_str()),
                    };
                    p.map_err(|e| e.to_string())?;
                }
                stmt.raw_execute().map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn drop_table(&self, name: &str) -> BiResult<()> {
        validate_name(name)?;
        self.conn
            .execute_batch(&format!("DROP TABLE IF EXISTS {}", quote_ident(name)))
            .map_err(|e| e.to_string())
    }

    /// SQLを実行して結果を返す。limit行で打ち切り、truncatedフラグを立てる。
    pub fn query(&self, sql: &str, limit: usize) -> BiResult<QueryResult> {
        let mut stmt = self.conn.prepare(sql).map_err(|e| e.to_string())?;
        let ncols = stmt.column_count();
        if ncols == 0 {
            // SELECT以外(DDL等)
            stmt.raw_execute().map_err(|e| e.to_string())?;
            return Ok(QueryResult {
                columns: vec![],
                rows: vec![],
                truncated: false,
                total_returned: 0,
            });
        }
        let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let mut rows: Vec<Vec<Value>> = Vec::new();
        let mut truncated = false;
        let mut q = stmt.query([]).map_err(|e| e.to_string())?;
        while let Some(row) = q.next().map_err(|e| e.to_string())? {
            if rows.len() >= limit {
                truncated = true;
                break;
            }
            let mut out = Vec::with_capacity(ncols);
            for c in 0..ncols {
                let v = match row.get_ref(c).map_err(|e| e.to_string())? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(i) => Value::Int(i),
                    ValueRef::Real(f) => Value::Float(f),
                    ValueRef::Text(t) => Value::Text(String::from_utf8_lossy(t).into_owned()),
                    ValueRef::Blob(_) => Value::Text("[blob]".to_string()),
                };
                out.push(v);
            }
            rows.push(out);
        }
        let total = rows.len();
        Ok(QueryResult {
            columns,
            rows,
            truncated,
            total_returned: total,
        })
    }
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn validate_name(name: &str) -> BiResult<()> {
    if name.trim().is_empty() {
        return Err("データセット名が空です".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TableData {
        TableData {
            schema: TableSchema {
                columns: vec![
                    ColumnSchema {
                        name: "id".into(),
                        data_type: DataType::Int64,
                    },
                    ColumnSchema {
                        name: "grp".into(),
                        data_type: DataType::Utf8,
                    },
                    ColumnSchema {
                        name: "val".into(),
                        data_type: DataType::Float64,
                    },
                ],
            },
            rows: vec![
                vec![Value::Int(1), Value::Text("a".into()), Value::Float(10.0)],
                vec![Value::Int(2), Value::Text("a".into()), Value::Float(20.0)],
                vec![Value::Int(3), Value::Text("b".into()), Value::Null],
            ],
        }
    }

    #[test]
    fn test_register_and_query() {
        let mut e = Engine::new().unwrap();
        e.register("t", &sample()).unwrap();
        let r = e
            .query(
                "SELECT grp, AVG(val) AS m, COUNT(*) AS n FROM t GROUP BY grp ORDER BY grp",
                100,
            )
            .unwrap();
        assert_eq!(r.columns, vec!["grp", "m", "n"]);
        assert_eq!(r.rows.len(), 2);
        assert_eq!(r.rows[0][1], Value::Float(15.0));
        assert_eq!(r.rows[1][1], Value::Null);
    }

    #[test]
    fn test_limit_truncation() {
        let mut e = Engine::new().unwrap();
        e.register("t", &sample()).unwrap();
        let r = e.query("SELECT * FROM t", 2).unwrap();
        assert_eq!(r.rows.len(), 2);
        assert!(r.truncated);
    }

    #[test]
    fn test_replace_dataset() {
        let mut e = Engine::new().unwrap();
        e.register("t", &sample()).unwrap();
        e.register("t", &sample()).unwrap();
        let r = e.query("SELECT COUNT(*) FROM t", 10).unwrap();
        assert_eq!(r.rows[0][0], Value::Int(3));
    }
}
