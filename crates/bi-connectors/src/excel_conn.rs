//! Excelコネクタ(calamine使用、読み込み専用)。
//! シート選択・ヘッダー行指定・型統一・日付シリアル値のISO文字列化を行う。
//! Excel固有の事情(シート/セル型/日付シリアル)はこのモジュール内に閉じ込める。

use bi_core::*;
use calamine::{open_workbook_auto, Data, Reader};
use std::path::Path;

pub struct ExcelConnector;

impl Connector for ExcelConnector {
    fn connector_type(&self) -> &'static str {
        "excel"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["xlsx", "xlsm", "xlsb", "xls", "ods"]
    }

    fn list_objects(&self, path: &Path) -> BiResult<Vec<String>> {
        let wb = open_workbook_auto(path).map_err(|e| format!("Excelを開けません: {e}"))?;
        Ok(wb.sheet_names().to_vec())
    }

    fn load(&self, path: &Path, object: &str, opts: &ImportOptions) -> BiResult<TableData> {
        let mut wb = open_workbook_auto(path).map_err(|e| format!("Excelを開けません: {e}"))?;
        let range = wb
            .worksheet_range(object)
            .map_err(|e| format!("シート「{object}」を読めません: {e}"))?;

        let header_idx = opts.header_row; // 1始まり(シートの使用範囲内)。0はヘッダーなし
        let data_start = if header_idx == 0 { 0 } else { header_idx } + opts.skip_rows;
        let max = opts.max_rows.unwrap_or(usize::MAX);

        let mut header: Option<Vec<String>> = None;
        let mut rows: Vec<Vec<Value>> = Vec::new();

        for (i, row) in range.rows().enumerate() {
            let line = i + 1;
            if header_idx != 0 && line == header_idx {
                header = Some(row.iter().map(cell_to_header).collect());
                continue;
            }
            if line <= data_start {
                continue;
            }
            if rows.len() >= max {
                break;
            }
            rows.push(row.iter().map(cell_to_value).collect());
        }

        let ncols = header
            .as_ref()
            .map(|h| h.len())
            .unwrap_or(0)
            .max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
        if ncols == 0 {
            return Err("シートにデータがありません".to_string());
        }
        // 行の長さを揃える
        for r in rows.iter_mut() {
            r.resize(ncols, Value::Null);
        }

        let names = match header {
            Some(mut h) => {
                h.resize(ncols, String::new());
                normalize_names(h)
            }
            None => (1..=ncols).map(|i| format!("column_{i}")).collect(),
        };

        let mut types = unify_columns(&mut rows, ncols);
        // Excelは整数値もFloatとして格納するため、全値が整数のFloat列はInt64へ降格する
        for c in 0..ncols {
            if types[c] == DataType::Float64 {
                let integral = rows.iter().all(|r| match &r[c] {
                    Value::Float(f) => f.fract() == 0.0 && f.abs() < 9.0e15,
                    Value::Null => true,
                    _ => false,
                });
                if integral {
                    types[c] = DataType::Int64;
                    for r in rows.iter_mut() {
                        if let Value::Float(f) = r[c] {
                            r[c] = Value::Int(f as i64);
                        }
                    }
                }
            }
        }
        let columns = names
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

fn cell_to_header(c: &Data) -> String {
    match c {
        Data::Empty => String::new(),
        Data::String(s) => s.trim().to_string(),
        other => other.to_string().trim().to_string(),
    }
}

/// Excelセル値 → 内部Value。数式は計算済み値が返る(calamineの仕様)。
fn cell_to_value(c: &Data) -> Value {
    match c {
        Data::Empty => Value::Null,
        Data::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                Value::Null
            } else {
                Value::Text(t.to_string())
            }
        }
        Data::Float(f) => Value::Float(*f),
        Data::Int(i) => Value::Int(*i),
        Data::Bool(b) => Value::Bool(*b),
        Data::DateTime(dt) => Value::Text(serial_to_iso(dt.as_f64())),
        Data::DateTimeIso(s) => Value::Text(s.clone()),
        Data::DurationIso(s) => Value::Text(s.clone()),
        Data::Error(_) => Value::Null,
    }
}

/// Excel日付シリアル値(1899-12-30基準) → ISO 8601文字列
fn serial_to_iso(serial: f64) -> String {
    let days = serial.floor() as i64;
    let frac = serial - days as f64;
    let unix_days = days - 25569; // 1899-12-30 → 1970-01-01
    let (y, m, d) = civil_from_days(unix_days);
    let secs = (frac * 86400.0).round() as i64;
    if secs <= 0 {
        format!("{y:04}-{m:02}-{d:02}")
    } else {
        let (hh, rem) = (secs / 3600, secs % 3600);
        format!("{y:04}-{m:02}-{d:02} {hh:02}:{:02}:{:02}", rem / 60, rem % 60)
    }
}

/// days since 1970-01-01 → (年, 月, 日)  (Howard Hinnantのアルゴリズム)
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serial_to_iso() {
        assert_eq!(serial_to_iso(45658.0), "2025-01-01");
        assert_eq!(serial_to_iso(25569.0), "1970-01-01");
        assert_eq!(serial_to_iso(45658.5), "2025-01-01 12:00:00");
    }
}
