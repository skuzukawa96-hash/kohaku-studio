//! 外部プロセス方式の Connector プラグイン(Plugin API Phase 1)。
//! 設計は docs/plugin-api-draft.md を参照。
//!
//! 方針:
//! - プラグインは実行ファイル。1リクエスト=1プロセス起動で、stdin にJSON 1行を書き、
//!   stdout からJSON 1行を読む(常駐させない)。stderr はエラーメッセージに使う。
//! - Rust の ABI 不安定性を避けるため動的ライブラリは使わない。言語は自由。
//! - **プラグインはユーザーの権限で動く任意コード**。既定は無効で、
//!   `--enable-plugins` を付けたときだけ読み込む。
//! - プラグインの失敗はプラグイン名付きのエラーにして返す(本体は落とさない)。

use bi_core::*;
use serde::Deserialize;
use serde_json::{json, Value as Json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// 対応するプロトコル版。マニフェストの api_version がこれと違えば読み込まない
const API_VERSION: u32 = 1;
/// 応答(stdout)の上限。暴走したプラグインでメモリを食い潰さないための保険
const MAX_RESPONSE: u64 = 256 * 1024 * 1024;

// タイムアウト(draft 5.5)
const TIMEOUT_DESCRIBE: Duration = Duration::from_secs(5);
const TIMEOUT_LIST: Duration = Duration::from_secs(30);
const TIMEOUT_LOAD: Duration = Duration::from_secs(600);

/// plugin.json の内容
#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    pub api_version: u32,
    pub name: String,
    #[serde(default)]
    pub version: String,
    pub kind: String,
    #[serde(default)]
    pub description: String,
    /// 実行コマンドと引数(例: ["python", "main.py"])。
    /// Windowsではシェバンが効かないため、インタプリタを明示できる形にしている。
    pub entry: Vec<String>,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub schemes: Vec<String>,
}

/// 既定のプラグインディレクトリ。
/// KOHAKU_PLUGIN_DIR > %LOCALAPPDATA% > $XDG_DATA_HOME > $HOME/.local/share の順。
pub fn default_plugin_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("KOHAKU_PLUGIN_DIR") {
        return Some(PathBuf::from(dir));
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("kohaku-studio").join("plugins"))
}

/// プラグインディレクトリを走査してコネクタを作る。
/// 読み込めなかったものは理由を warnings に入れて返す(黙って無視しない)。
pub fn discover(root: &Path) -> (Vec<PluginConnector>, Vec<String>) {
    let mut found = Vec::new();
    let mut warnings = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        // ディレクトリが無いのは正常(プラグイン未導入)
        Err(_) => return (found, warnings),
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let manifest_path = dir.join("plugin.json");
        if !manifest_path.is_file() {
            continue;
        }
        match load_manifest(&manifest_path) {
            Ok(m) => match PluginConnector::new(m, dir.clone()) {
                Ok(c) => found.push(c),
                Err(e) => warnings.push(format!("{}: {e}", dir.display())),
            },
            Err(e) => warnings.push(format!("{}: {e}", manifest_path.display())),
        }
    }
    (found, warnings)
}

fn load_manifest(path: &Path) -> BiResult<PluginManifest> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("plugin.json を読めません: {e}"))?;
    let m: PluginManifest =
        serde_json::from_str(&text).map_err(|e| format!("plugin.json の形式が不正: {e}"))?;
    if m.api_version != API_VERSION {
        return Err(format!(
            "api_version {} は未対応です(このバージョンは {API_VERSION})",
            m.api_version
        ));
    }
    if m.name.trim().is_empty() {
        return Err("name が空です".to_string());
    }
    if m.entry.is_empty() {
        return Err("entry が空です".to_string());
    }
    Ok(m)
}

/// 起動時のみ使う。Connector trait が &'static を要求するため、
/// マニフェスト由来の文字列をリークして 'static にする(プラグイン数ぶんで有界)。
fn leak_strs(v: &[String]) -> &'static [&'static str] {
    let refs: Vec<&'static str> = v
        .iter()
        .map(|s| &*Box::leak(s.clone().to_lowercase().into_boxed_str()))
        .collect();
    Box::leak(refs.into_boxed_slice())
}

pub struct PluginConnector {
    manifest: PluginManifest,
    dir: PathBuf,
    extensions: &'static [&'static str],
    schemes: &'static [&'static str],
}

impl PluginConnector {
    fn new(manifest: PluginManifest, dir: PathBuf) -> BiResult<Self> {
        if manifest.kind != "connector" {
            return Err(format!(
                "kind「{}」はこのバージョンでは未対応です(connector のみ)",
                manifest.kind
            ));
        }
        if manifest.extensions.is_empty() && manifest.schemes.is_empty() {
            return Err("extensions か schemes のどちらかを指定してください".to_string());
        }
        let extensions = leak_strs(&manifest.extensions);
        let schemes = leak_strs(&manifest.schemes);
        Ok(PluginConnector {
            manifest,
            dir,
            extensions,
            schemes,
        })
    }

    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    pub fn description(&self) -> &str {
        &self.manifest.description
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// このプラグインが受け持つ対象(表示用): [".jsonl", "myapi://"]
    pub fn targets(&self) -> Vec<String> {
        self.extensions
            .iter()
            .map(|e| format!(".{e}"))
            .chain(self.schemes.iter().map(|s| format!("{s}://")))
            .collect()
    }

    /// プラグインを起動して1往復する。エラーには必ずプラグイン名を含める。
    fn call(&self, req: &Json, timeout: Duration) -> BiResult<Json> {
        let name = &self.manifest.name;
        let mut child = self
            .spawn()
            .map_err(|e| format!("プラグイン「{name}」を起動できません: {e}"))?;

        // リクエストは1行。書き終えたらstdinを閉じてEOFを伝える
        let line = format!("{req}\n");
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(e) = stdin.write_all(line.as_bytes()) {
                let _ = child.kill();
                return Err(format!("プラグイン「{name}」への書き込みに失敗: {e}"));
            }
        }

        // stdout/stderr は別スレッドで読む(パイプが詰まって双方が待ち合うのを防ぐ)
        let mut stdout = child.stdout.take().ok_or("stdoutを取得できません")?;
        let mut stderr = child.stderr.take().ok_or("stderrを取得できません")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let r = stdout.by_ref().take(MAX_RESPONSE).read_to_end(&mut buf);
            let _ = tx.send(r.map(|_| buf));
        });
        let (etx, erx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr.by_ref().take(64 * 1024).read_to_end(&mut buf);
            let _ = etx.send(buf);
        });

        let out = match rx.recv_timeout(timeout) {
            Ok(Ok(buf)) => buf,
            Ok(Err(e)) => {
                let _ = child.kill();
                return Err(format!("プラグイン「{name}」の出力を読めません: {e}"));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "プラグイン「{name}」が応答しません({}秒でタイムアウト)",
                    timeout.as_secs()
                ));
            }
        };
        let status = child.wait().ok();
        let err_text = erx
            .recv_timeout(Duration::from_secs(2))
            .map(|b| String::from_utf8_lossy(&b).trim().to_string())
            .unwrap_or_default();

        if out.is_empty() {
            let code = status.map(|s| s.to_string()).unwrap_or_default();
            return Err(format!(
                "プラグイン「{name}」が応答を返しませんでした({code}){}",
                detail(&err_text)
            ));
        }
        // 応答は UTF-8 のJSON。Windowsではプラグイン側の標準出力が
        // ロケール依存(cp932など)になりやすく、ここで気づけるようにする。
        let text = std::str::from_utf8(&out).map_err(|_| {
            format!(
                "プラグイン「{name}」の応答がUTF-8ではありません(標準出力をUTF-8で書き出してください){}",
                detail(&err_text)
            )
        })?;
        let resp: Json = serde_json::from_str(text).map_err(|e| {
            format!(
                "プラグイン「{name}」の応答がJSONとして読めません: {e}{}",
                detail(&err_text)
            )
        })?;
        if resp.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let msg = resp
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("原因不明のエラー");
            return Err(format!("プラグイン「{name}」: {msg}"));
        }
        Ok(resp)
    }

    fn spawn(&self) -> std::io::Result<Child> {
        let mut cmd = Command::new(&self.manifest.entry[0]);
        cmd.args(&self.manifest.entry[1..])
            .current_dir(&self.dir) // entry の相対パスはプラグインのディレクトリ基準
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.spawn()
    }

    /// プラグインが起動でき、プロトコルを話せるかを確認する(--list-plugins 用)
    pub fn describe(&self) -> BiResult<String> {
        let resp = self.call(
            &json!({"cmd": "describe", "api_version": API_VERSION}),
            TIMEOUT_DESCRIBE,
        )?;
        Ok(resp
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.manifest.name)
            .to_string())
    }
}

fn detail(stderr: &str) -> String {
    if stderr.is_empty() {
        String::new()
    } else {
        // 長いスタックトレースは末尾だけ見せる(原因は最後の行に出ることが多い)
        let tail: Vec<&str> = stderr.lines().rev().take(3).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        format!(" / プラグインの出力: {}", tail.join(" | "))
    }
}

impl Connector for PluginConnector {
    fn connector_type(&self) -> &'static str {
        "plugin"
    }

    fn extensions(&self) -> &'static [&'static str] {
        self.extensions
    }

    fn schemes(&self) -> &'static [&'static str] {
        self.schemes
    }

    fn list_objects(&self, path: &Path) -> BiResult<Vec<String>> {
        let resp = self.call(
            &json!({"cmd": "list_objects", "path": path.to_string_lossy()}),
            TIMEOUT_LIST,
        )?;
        let objects = resp
            .get("objects")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("プラグイン「{}」: objects がありません", self.manifest.name))?
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        Ok(objects)
    }

    fn load(&self, path: &Path, object: &str, opts: &ImportOptions) -> BiResult<TableData> {
        let resp = self.call(
            &json!({
                "cmd": "load",
                "path": path.to_string_lossy(),
                "object": object,
                "options": {
                    "header_row": opts.header_row,
                    "skip_rows": opts.skip_rows,
                    "delimiter": opts.delimiter,
                    "max_rows": opts.max_rows,
                },
            }),
            TIMEOUT_LOAD,
        )?;
        let table = resp
            .get("table")
            .ok_or_else(|| format!("プラグイン「{}」: table がありません", self.manifest.name))?;
        let mut td = parse_table(table, &self.manifest.name)?;
        // プラグインが max_rows を守らなくても本体側で必ず切る
        if let Some(n) = opts.max_rows {
            td.rows.truncate(n);
        }
        Ok(td)
    }
}

/// プラグインが返した table(JSON)を TableData に変換する。
/// 列の型宣言は値の解釈のヒントとして使い、最終的な型は unify_columns で決める
/// (他のコネクタと同じ扱いにして、宣言と実際の値がずれていても壊れないようにする)。
fn parse_table(table: &Json, plugin: &str) -> BiResult<TableData> {
    let cols = table
        .get("columns")
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("プラグイン「{plugin}」: columns がありません"))?;
    if cols.is_empty() {
        return Err(format!("プラグイン「{plugin}」: 列がありません"));
    }
    let names: Vec<String> = cols
        .iter()
        .enumerate()
        .map(|(i, c)| {
            c.get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("column_{}", i + 1))
        })
        .collect();
    let hints: Vec<&str> = cols
        .iter()
        .map(|c| c.get("type").and_then(|v| v.as_str()).unwrap_or("text"))
        .collect();
    let ncols = names.len();

    let rows_json = table
        .get("rows")
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("プラグイン「{plugin}」: rows がありません"))?;
    let mut rows: Vec<Vec<Value>> = Vec::with_capacity(rows_json.len());
    for r in rows_json {
        let cells = r.as_array().ok_or_else(|| {
            format!("プラグイン「{plugin}」: rows の要素は配列である必要があります")
        })?;
        let row: Vec<Value> = (0..ncols)
            .map(|i| json_to_value(cells.get(i).unwrap_or(&Json::Null), hints[i]))
            .collect();
        rows.push(row);
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

/// JSON値を宣言型のヒントに従って Value にする(合わない場合は素直な型で受ける)
fn json_to_value(v: &Json, hint: &str) -> Value {
    match v {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => match hint {
            // 整数宣言でも実際が小数なら Float で受け、後段の unify_columns に任せる
            "text" => Value::Text(n.to_string()),
            "integer" => n
                .as_i64()
                .map(Value::Int)
                .or_else(|| n.as_f64().map(Value::Float))
                .unwrap_or(Value::Null),
            _ => n
                .as_f64()
                .map(Value::Float)
                .or_else(|| n.as_i64().map(Value::Int))
                .unwrap_or(Value::Null),
        },
        Json::String(s) => match hint {
            "integer" => s
                .trim()
                .parse::<i64>()
                .map(Value::Int)
                .unwrap_or_else(|_| Value::Text(s.clone())),
            "real" => s
                .trim()
                .parse::<f64>()
                .map(Value::Float)
                .unwrap_or_else(|_| Value::Text(s.clone())),
            "boolean" => match s.trim().to_ascii_lowercase().as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => Value::Text(s.clone()),
            },
            _ => Value::Text(s.clone()),
        },
        // 配列・オブジェクトは表に収まらないためJSON文字列として保持する
        other => Value::Text(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(kind: &str, exts: &[&str]) -> PluginManifest {
        PluginManifest {
            api_version: 1,
            name: "test-plugin".into(),
            version: "0.1.0".into(),
            kind: kind.into(),
            description: String::new(),
            entry: vec!["python".into(), "main.py".into()],
            extensions: exts.iter().map(|s| s.to_string()).collect(),
            schemes: vec![],
        }
    }

    #[test]
    fn test_manifest_validation() {
        let dir = std::env::temp_dir();
        // connector 以外の kind は Phase 1 では受け付けない
        assert!(PluginConnector::new(manifest("transform", &["x"]), dir.clone()).is_err());
        // 拡張子もスキームも無いと解決できない
        assert!(PluginConnector::new(manifest("connector", &[]), dir.clone()).is_err());
        // 正常系: 拡張子は小文字に正規化される
        let c = PluginConnector::new(manifest("connector", &["JSONL"]), dir).unwrap();
        assert_eq!(c.extensions(), &["jsonl"]);
        assert_eq!(c.connector_type(), "plugin");
    }

    #[test]
    fn test_manifest_api_version_and_shape() {
        let dir = std::env::temp_dir().join("kohaku-plugin-manifest-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("plugin.json");

        std::fs::write(
            &p,
            r#"{"api_version":99,"name":"x","kind":"connector","entry":["x"]}"#,
        )
        .unwrap();
        assert!(load_manifest(&p).unwrap_err().contains("api_version"));

        std::fs::write(
            &p,
            r#"{"api_version":1,"name":"","kind":"connector","entry":["x"]}"#,
        )
        .unwrap();
        assert!(load_manifest(&p).unwrap_err().contains("name"));

        std::fs::write(
            &p,
            r#"{"api_version":1,"name":"x","kind":"connector","entry":[]}"#,
        )
        .unwrap();
        assert!(load_manifest(&p).unwrap_err().contains("entry"));

        std::fs::write(&p, "{ not json").unwrap();
        assert!(load_manifest(&p).unwrap_err().contains("形式が不正"));

        std::fs::write(
            &p,
            r#"{"api_version":1,"name":"ok","kind":"connector","entry":["python","main.py"],"extensions":["jsonl"]}"#,
        )
        .unwrap();
        let m = load_manifest(&p).unwrap();
        assert_eq!(m.name, "ok");
        assert_eq!(m.entry, vec!["python", "main.py"]);
    }

    #[test]
    fn test_parse_table() {
        let t = json!({
            "columns": [
                {"name": "id", "type": "integer"},
                {"name": "score", "type": "real"},
                {"name": "label", "type": "text"},
                {"name": "ok", "type": "boolean"}
            ],
            "rows": [
                [1, 0.5, "あ", true],
                [2, 1.5, "い", false],
                [null, null, null, null]
            ]
        });
        let td = parse_table(&t, "p").unwrap();
        assert_eq!(td.schema.columns.len(), 4);
        assert_eq!(td.schema.columns[0].data_type, DataType::Int64);
        assert_eq!(td.schema.columns[1].data_type, DataType::Float64);
        assert_eq!(td.schema.columns[2].data_type, DataType::Utf8);
        assert_eq!(td.schema.columns[3].data_type, DataType::Boolean);
        assert_eq!(td.rows.len(), 3);
        assert_eq!(td.rows[0][0], Value::Int(1));
        assert_eq!(td.rows[2][0], Value::Null);
    }

    #[test]
    fn test_parse_table_tolerates_mismatch() {
        // 型宣言と実際の値がずれていても壊れず、最終的な型は値から決まる
        let t = json!({
            "columns": [{"name": "v", "type": "integer"}],
            "rows": [[1], ["N/A"], [3]]
        });
        let td = parse_table(&t, "p").unwrap();
        assert_eq!(td.schema.columns[0].data_type, DataType::Utf8);
        assert_eq!(td.rows[1][0], Value::Text("N/A".into()));

        // 文字列で来た数値は宣言型に従って数値化される
        let t2 = json!({
            "columns": [{"name": "v", "type": "integer"}],
            "rows": [["10"], ["20"]]
        });
        let td2 = parse_table(&t2, "p").unwrap();
        assert_eq!(td2.schema.columns[0].data_type, DataType::Int64);
        assert_eq!(td2.rows[0][0], Value::Int(10));

        // 行が短くてもNullで埋める / 配列はJSON文字列として保持
        let t3 = json!({
            "columns": [{"name": "a", "type": "text"}, {"name": "b", "type": "text"}],
            "rows": [["x"], ["y", [1, 2]]]
        });
        let td3 = parse_table(&t3, "p").unwrap();
        assert_eq!(td3.rows[0][1], Value::Null);
        assert_eq!(td3.rows[1][1], Value::Text("[1,2]".into()));
    }

    #[test]
    fn test_parse_table_errors() {
        assert!(parse_table(&json!({"rows": []}), "p").is_err());
        assert!(parse_table(&json!({"columns": []}), "p").is_err());
        assert!(parse_table(&json!({"columns": [{"name": "a"}]}), "p").is_err());
        assert!(parse_table(&json!({"columns": [{"name": "a"}], "rows": [1]}), "p").is_err());
    }

    #[test]
    fn test_discover_missing_dir_is_ok() {
        let (found, warns) = discover(Path::new("C:/no/such/plugin/dir"));
        assert!(found.is_empty());
        assert!(warns.is_empty()); // 未導入は警告ではない
    }
}
