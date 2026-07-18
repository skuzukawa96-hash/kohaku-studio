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

    fn load(&self, path: &Path, object: &str, opts: &ImportOptions) -> BiResult<TableData> {
        // 実装は load_stream に一本化する(挙動を1箇所に保つ)
        let mut sink = CollectSink::default();
        self.load_stream(path, object, opts, &mut sink)?;
        Ok(TableData {
            schema: sink.schema.ok_or("CSVの読み込みに失敗しました")?,
            rows: sink.rows,
        })
    }

    /// ストリーミング読み込み(v0.4.x)。全行のStringを同時に持たないよう、
    /// パス1で列数・ヘッダー・型推定サンプル(先頭2000行)だけを集め、
    /// パス2で行を直接 Value 化してチャンク単位で sink へ流す。
    /// CSVを2回パースするCPUコストと引き換えに、ピークメモリを大きく下げる。
    fn load_stream(
        &self,
        path: &Path,
        _object: &str,
        opts: &ImportOptions,
        sink: &mut dyn RowSink,
    ) -> BiResult<()> {
        let bytes = std::fs::read(path).map_err(|e| format!("ファイルを開けません: {e}"))?;
        let text = decode_text(&bytes);
        drop(bytes);
        let delim = match opts.delimiter.as_deref() {
            Some("\\t") | Some("tab") => b'\t',
            Some(s) if !s.is_empty() => s.as_bytes()[0],
            _ => sniff_delimiter(&text, path),
        };

        let header_idx = opts.header_row; // 1始まり。0はヘッダーなし
        let data_start = if header_idx == 0 { 0 } else { header_idx } + opts.skip_rows;
        let max = opts.max_rows.unwrap_or(usize::MAX);

        // パス1: ヘッダー・列数の最大値・型推定用サンプルを収集
        // (行のStringはサンプル分しか保持しない)
        const SAMPLE_ROWS: usize = 2_000;
        let mut header: Option<Vec<String>> = None;
        let mut sample: Vec<Vec<String>> = Vec::new();
        let mut ncols = 0usize;
        let mut data_rows = 0usize;
        for (i, rec) in make_reader(&text, delim).records().enumerate() {
            let rec = rec.map_err(|e| format!("CSV解析エラー(行{}): {e}", i + 1))?;
            let line = i + 1; // 1始まり
            if header_idx != 0 && line == header_idx {
                ncols = ncols.max(rec.len());
                header = Some(rec.iter().map(|s| s.to_string()).collect());
                continue;
            }
            if line <= data_start {
                continue;
            }
            if data_rows >= max {
                break;
            }
            data_rows += 1;
            ncols = ncols.max(rec.len());
            if sample.len() < SAMPLE_ROWS {
                sample.push(rec.iter().map(|s| s.to_string()).collect());
            }
        }
        if ncols == 0 {
            return Err("データがありません".to_string());
        }

        let names = match header {
            Some(mut h) => {
                h.resize(ncols, String::new());
                normalize_names(h)
            }
            None => (1..=ncols).map(|i| format!("column_{i}")).collect(),
        };
        let types = infer_text_types(&sample, ncols);
        drop(sample);
        let columns = names
            .into_iter()
            .zip(types.iter().copied())
            .map(|(name, data_type)| ColumnSchema { name, data_type })
            .collect();
        sink.start(&TableSchema { columns })?;

        // パス2: 直接 Value 化してチャンク送出
        const CHUNK_ROWS: usize = 50_000;
        let mut chunk: Vec<Vec<Value>> = Vec::new();
        let mut sent = 0usize;
        for (i, rec) in make_reader(&text, delim).records().enumerate() {
            let rec = rec.map_err(|e| format!("CSV解析エラー(行{}): {e}", i + 1))?;
            let line = i + 1;
            if (header_idx != 0 && line == header_idx) || line <= data_start {
                continue;
            }
            if sent + chunk.len() >= max {
                break;
            }
            let row: Vec<Value> = (0..ncols)
                .map(|c| parse_text_cell(rec.get(c).unwrap_or(""), types[c]))
                .collect();
            chunk.push(row);
            if chunk.len() >= CHUNK_ROWS {
                sent += chunk.len();
                sink.push_rows(std::mem::take(&mut chunk))?;
            }
        }
        if !chunk.is_empty() {
            sink.push_rows(chunk)?;
        }
        Ok(())
    }
}

fn make_reader(text: &str, delim: u8) -> csv::Reader<&[u8]> {
    csv::ReaderBuilder::new()
        .delimiter(delim)
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes())
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

    /// 受け取ったチャンクの数と行を記録する RowSink(ストリーミング検証用)
    #[derive(Default)]
    struct CountingSink {
        schema: Option<TableSchema>,
        chunks: Vec<usize>,
        rows: Vec<Vec<Value>>,
    }

    impl RowSink for CountingSink {
        fn start(&mut self, schema: &TableSchema) -> BiResult<()> {
            self.schema = Some(schema.clone());
            Ok(())
        }
        fn push_rows(&mut self, mut rows: Vec<Vec<Value>>) -> BiResult<()> {
            self.chunks.push(rows.len());
            self.rows.append(&mut rows);
            Ok(())
        }
    }

    /// チャンク境界(5万行)をまたいでも、行が落ちず順序も保たれる
    #[test]
    fn test_stream_chunk_boundary_and_order() {
        let n = 120_000; // 50,000 × 2 + 20,000 の3チャンクになる
        let mut csv = String::from("i,name\n");
        for i in 0..n {
            csv.push_str(&format!("{i},row{i}\n"));
        }
        let p = temp_file("t_stream_chunk.csv", csv.as_bytes());

        let mut sink = CountingSink::default();
        CsvConnector
            .load_stream(&p, "", &ImportOptions::default(), &mut sink)
            .unwrap();

        assert!(
            sink.chunks.len() >= 2,
            "複数チャンクに分割される: {:?}",
            sink.chunks
        );
        assert_eq!(sink.chunks.iter().sum::<usize>(), n);
        assert_eq!(sink.rows.len(), n);
        // 先頭・境界前後・末尾の値と順序を確認
        for i in [0usize, 49_999, 50_000, 99_999, 100_000, n - 1] {
            assert_eq!(sink.rows[i][0], Value::Int(i as i64), "行{i}の値");
            assert_eq!(
                sink.rows[i][1],
                Value::Text(format!("row{i}")),
                "行{i}の名前"
            );
        }
        assert_eq!(sink.schema.unwrap().columns[0].data_type, DataType::Int64);
    }

    /// max_rows は2パス通して守られる(先頭から指定行数ちょうど)
    #[test]
    fn test_stream_max_rows() {
        let mut csv = String::from("a\n");
        for i in 0..100 {
            csv.push_str(&format!("{i}\n"));
        }
        let p = temp_file("t_stream_max.csv", csv.as_bytes());
        let opts = ImportOptions {
            max_rows: Some(7),
            ..Default::default()
        };
        let td = CsvConnector.load(&p, "", &opts).unwrap();
        assert_eq!(td.rows.len(), 7);
        assert_eq!(td.rows[0][0], Value::Int(0));
        assert_eq!(td.rows[6][0], Value::Int(6));
    }

    /// 型推定サンプル(先頭2000行)の外に型外れの値があっても、
    /// 列型は維持しつつその値だけ Text にフォールバックする(既存挙動の維持)
    #[test]
    fn test_stream_type_outlier_beyond_sample() {
        let mut csv = String::from("v\n");
        for i in 0..2_500 {
            if i == 2_400 {
                csv.push_str("N/A\n");
            } else {
                csv.push_str(&format!("{i}\n"));
            }
        }
        let p = temp_file("t_stream_outlier.csv", csv.as_bytes());
        let td = CsvConnector
            .load(&p, "", &ImportOptions::default())
            .unwrap();
        assert_eq!(td.schema.columns[0].data_type, DataType::Int64);
        assert_eq!(td.rows.len(), 2_500);
        assert_eq!(td.rows[2_400][0], Value::Text("N/A".to_string()));
        assert_eq!(td.rows[2_399][0], Value::Int(2_399));
    }
}
