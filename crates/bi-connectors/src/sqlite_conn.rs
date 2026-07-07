//! SQLiteファイルコネクタ。テーブル/ビュー一覧を取得し、内容を読み込む。

use bi_core::*;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

pub struct SqliteConnector;

impl Connector for SqliteConnector {
    fn connector_type(&self) -> &'static str {
        "sqlite"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["db", "sqlite", "sqlite3", "db3"]
    }

    fn list_objects(&self, path: &Path) -> BiResult<Vec<String>> {
        let conn = open_ro(path)?;
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .map_err(|e| e.to_string())?;
        let names = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        Ok(names)
    }

    fn load(&self, path: &Path, object: &str, opts: &ImportOptions) -> BiResult<TableData> {
        let conn = open_ro(path)?;
        let sql = match opts.max_rows {
            Some(n) => format!("SELECT * FROM \"{}\" LIMIT {n}", escape_ident(object)),
            None => format!("SELECT * FROM \"{}\"", escape_ident(object)),
        };
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let ncols = names.len();
        let mut rows: Vec<Vec<Value>> = Vec::new();
        let mut q = stmt.query([]).map_err(|e| e.to_string())?;
        while let Some(row) = q.next().map_err(|e| e.to_string())? {
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
        let types = unify_columns(&mut rows, ncols);
        let columns = normalize_names(names)
            .into_iter()
            .zip(types)
            .map(|(name, data_type)| ColumnSchema { name, data_type })
            .collect();
        Ok(TableData {
            schema: TableSchema { columns },
            rows,
        })
    }
}

fn open_ro(path: &Path) -> BiResult<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("DBを開けません: {e}"))
}

fn escape_ident(s: &str) -> String {
    s.replace('"', "\"\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sqlite_roundtrip() {
        let dir = std::env::temp_dir().join("bi_test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.db");
        let _ = std::fs::remove_file(&p);
        {
            let conn = Connection::open(&p).unwrap();
            conn.execute_batch(
                "CREATE TABLE items(id INTEGER, name TEXT, price REAL);
                 INSERT INTO items VALUES (1,'apple',120.5),(2,'banana',80.0),(3,NULL,NULL);",
            )
            .unwrap();
        }
        let objs = SqliteConnector.list_objects(&p).unwrap();
        assert_eq!(objs, vec!["items"]);
        let td = SqliteConnector
            .load(&p, "items", &ImportOptions::default())
            .unwrap();
        assert_eq!(td.rows.len(), 3);
        assert_eq!(td.schema.columns[0].data_type, DataType::Int64);
        assert_eq!(td.rows[0][1], Value::Text("apple".into()));
    }
}
