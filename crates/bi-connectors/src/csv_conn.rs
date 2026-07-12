//! CSVコネクタ。UTF-8(BOM付き含む)/Shift-JISを自動判別し、
//! 区切り文字(カンマ/タブ/セミコロン/パイプ)を自動推定する。

use bi_core::*;
use std::path::Path;

pub struct CsvConnector;

impl Connector for CsvConnector {
    fn connector_type(&self) -> &'static str {
        "csv"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["csv", "tsv", "txt"]
    }

    fn list_objects(&self, path: &Path) -> BiResult<Vec<String>> {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "data".to_string());
        Ok(vec![stem])
    }

    fn load(&self, path: &Path, _object: &str, opts: &ImportOptions) -> BiResult<TableData> {
        let bytes = std::fs::read(path).map_err(|e| format!("ファイルを開けません: {e}"))?;
        let text = decode_text(&bytes);
        let delim = match opts.delimiter.as_deref() {
            Some("\\t") | Some("tab") => b'\t',
            Some(s) if !s.is_empty() => s.as_bytes()[0],
            _ => sniff_delimiter(&text, path),
        };

        let mut reader = csv::ReaderBuilder::new()
            .delimiter(delim)
            .has_headers(false)
            .flexible(true)
            .from_reader(text.as_bytes());

        let header_idx = opts.header_row; // 1始まり。0はヘッダーなし
        let data_start = if header_idx == 0 { 0 } else { header_idx } + opts.skip_rows;
        let mut header: Option<Vec<String>> = None;
        let mut rows: Vec<Vec<String>> = Vec::new();
        let max = opts.max_rows.unwrap_or(usize::MAX);

        for (i, rec) in reader.records().enumerate() {
            let rec = rec.map_err(|e| format!("CSV解析エラー(行{}): {e}", i + 1))?;
            let line = i + 1; // 1始まり
            if header_idx != 0 && line == header_idx {
                header = Some(rec.iter().map(|s| s.to_string()).collect());
                continue;
            }
            if line <= data_start {
                continue;
            }
            if rows.len() >= max {
                break;
            }
            rows.push(rec.iter().map(|s| s.to_string()).collect());
        }

        let ncols = header
            .as_ref()
            .map(|h| h.len())
            .unwrap_or(0)
            .max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
        if ncols == 0 {
            return Err("データがありません".to_string());
        }

        let names = match header {
            Some(h) => {
                let mut h = h;
                h.resize(ncols, String::new());
                normalize_names(h)
            }
            None => (1..=ncols).map(|i| format!("column_{i}")).collect(),
        };

        let (types, values) = parse_text_table(rows, ncols);
        let columns = names
            .into_iter()
            .zip(types)
            .map(|(name, data_type)| ColumnSchema { name, data_type })
            .collect();
        Ok(TableData {
            schema: TableSchema { columns },
            rows: values,
        })
    }
}

/// UTF-8として妥当ならUTF-8(BOM除去)、そうでなければShift-JISとして読む
fn decode_text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let (cow, _, _) = encoding_rs::SHIFT_JIS.decode(bytes);
            cow.into_owned()
        }
    }
}

/// 先頭行の出現数から区切り文字を推定。tsv拡張子はタブ優先。
fn sniff_delimiter(text: &str, path: &Path) -> u8 {
    if path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("tsv"))
        .unwrap_or(false)
    {
        return b'\t';
    }
    let first = text.lines().next().unwrap_or("");
    let candidates = *b",\t;|";
    let mut best = b',';
    let mut best_n = 0;
    for &c in &candidates {
        let n = first.bytes().filter(|&b| b == c).count();
        if n > best_n {
            best_n = n;
            best = c;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str, content: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("bi_test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(content).unwrap();
        p
    }

    #[test]
    fn test_csv_load() {
        let p = temp_file("t1.csv", "a,b,c\n1,2.5,x\n2,,y\n".as_bytes());
        let td = CsvConnector
            .load(&p, "", &ImportOptions::default())
            .unwrap();
        assert_eq!(td.schema.columns.len(), 3);
        assert_eq!(td.schema.columns[0].data_type, DataType::Int64);
        assert_eq!(td.schema.columns[1].data_type, DataType::Float64);
        assert_eq!(td.rows.len(), 2);
        assert_eq!(td.rows[1][1], Value::Null);
    }

    #[test]
    fn test_sjis_csv() {
        // "名前,値\nテスト,1\n" をShift-JISで
        let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("名前,値\nテスト,1\n");
        let p = temp_file("t2.csv", &sjis);
        let td = CsvConnector
            .load(&p, "", &ImportOptions::default())
            .unwrap();
        assert_eq!(td.schema.columns[0].name, "名前");
        assert_eq!(td.rows[0][0], Value::Text("テスト".to_string()));
    }

    #[test]
    fn test_header_row_option() {
        let p = temp_file("t3.csv", "junk\na,b\n1,2\n".as_bytes());
        let opts = ImportOptions {
            header_row: 2,
            ..Default::default()
        };
        let td = CsvConnector.load(&p, "", &opts).unwrap();
        assert_eq!(td.schema.columns[0].name, "a");
        assert_eq!(td.rows.len(), 1);
    }
}
