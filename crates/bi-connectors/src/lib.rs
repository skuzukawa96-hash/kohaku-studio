//! bi-connectors: データソースを bi-core::TableData へ正規化するコネクタ群。
//! 新しい形式への対応は Connector trait を実装し registry に登録するだけでよい。

mod csv_conn;
mod excel_conn;
mod sqlite_conn;

pub use csv_conn::CsvConnector;
pub use excel_conn::ExcelConnector;
pub use sqlite_conn::SqliteConnector;

use bi_core::Connector;
use std::path::Path;

/// コネクタレジストリ。拡張子からコネクタを解決する。
pub struct ConnectorRegistry {
    connectors: Vec<Box<dyn Connector>>,
}

impl Default for ConnectorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectorRegistry {
    pub fn new() -> Self {
        ConnectorRegistry {
            connectors: vec![
                Box::new(CsvConnector),
                Box::new(ExcelConnector),
                Box::new(SqliteConnector),
            ],
        }
    }

    pub fn register(&mut self, c: Box<dyn Connector>) {
        self.connectors.push(c);
    }

    pub fn for_path(&self, path: &Path) -> Option<&dyn Connector> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        self.connectors
            .iter()
            .find(|c| c.extensions().contains(&ext.as_str()))
            .map(|c| c.as_ref())
    }

    /// 対応するすべての拡張子
    pub fn all_extensions(&self) -> Vec<&'static str> {
        self.connectors
            .iter()
            .flat_map(|c| c.extensions().iter().copied())
            .collect()
    }
}
