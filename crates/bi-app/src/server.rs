//! ローカルHTTPサーバー。UI(静的ファイル)とJSON APIを提供する。
//! UIはCommandを投げるだけで、データ処理はすべてRust側で行う(設計Rule 1)。

use crate::engine::Engine;
use bi_connectors::ConnectorRegistry;
use bi_core::*;
use serde_json::{json, Value as Json};
use std::io::Read;
use std::path::{Path, PathBuf};
use tiny_http::{Header, Method, Request, Response, Server};

const INDEX_HTML: &str = include_str!("../ui/index.html");
const APP_JS: &str = include_str!("../ui/app.js");
const STYLE_CSS: &str = include_str!("../ui/style.css");

const MAX_BODY: usize = 32 * 1024 * 1024;
/// インポート時の安全上限(在メモリ SQLite に載せるため)
const MAX_IMPORT_ROWS: usize = 2_000_000;

pub struct AppState {
    pub engine: Engine,
    pub registry: ConnectorRegistry,
    pub datasets: Vec<DatasetDef>,
    pub charts: Vec<Json>,
    pub queries: Vec<String>,
    pub project_name: String,
    /// Parquetキャッシュの有効/無効(--no-cache で無効化)
    pub use_cache: bool,
}

impl AppState {
    pub fn new(use_cache: bool) -> BiResult<AppState> {
        Ok(AppState {
            engine: Engine::new()?,
            registry: ConnectorRegistry::new(),
            datasets: Vec::new(),
            charts: Vec::new(),
            queries: Vec::new(),
            project_name: "無題プロジェクト".to_string(),
            use_cache,
        })
    }
}

pub fn run(port: u16, open_browser: bool, use_cache: bool) -> BiResult<()> {
    let mut state = AppState::new(use_cache)?;
    let mut bound_port = port;
    let server = {
        let mut srv = None;
        for p in port..port + 20 {
            match Server::http(("127.0.0.1", p)) {
                Ok(s) => {
                    bound_port = p;
                    srv = Some(s);
                    break;
                }
                Err(_) => continue,
            }
        }
        srv.ok_or("ポートをバインドできません")?
    };
    let url = format!("http://127.0.0.1:{bound_port}/");
    println!("Kohaku Studio 起動: {url}");
    println!("終了するにはこのウィンドウで Ctrl+C を押してください。");
    if open_browser {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", &url])
            .spawn();
    }

    for mut request in server.incoming_requests() {
        let path = request.url().split('?').next().unwrap_or("/").to_string();
        let response = match path.as_str() {
            "/" => static_resp(INDEX_HTML, "text/html; charset=utf-8"),
            "/app.js" => static_resp(APP_JS, "application/javascript; charset=utf-8"),
            "/style.css" => static_resp(STYLE_CSS, "text/css; charset=utf-8"),
            p if p.starts_with("/api/") => {
                // CSRFガードの結果に関わらず、必ずボディを読み切ってから応答する。
                // (単一スレッドのtiny_httpでボディ未読のまま応答するとkeep-alive接続が
                //  壊れ、後続リクエストの読み取りがブロックされるため)
                let guard = api_guard(&request);
                let mut body = String::new();
                let _ = request
                    .as_reader()
                    .take(MAX_BODY as u64)
                    .read_to_string(&mut body);
                if let Some(rejected) = guard {
                    let _ = request.respond(rejected);
                    continue;
                }
                let req_json: Json = if body.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str(&body).unwrap_or(json!({}))
                };
                let result = handle_api(&mut state, p, &req_json);
                let payload = match result {
                    Ok(v) => json!({"ok": true, "data": v}),
                    Err(e) => json!({"ok": false, "error": e}),
                };
                static_resp(&payload.to_string(), "application/json; charset=utf-8")
            }
            _ => Response::from_string("not found")
                .with_status_code(404)
                .boxed(),
        };
        let _ = request.respond(response);
    }
    Ok(())
}

fn static_resp(content: &str, ctype: &str) -> tiny_http::ResponseBox {
    let header = Header::from_bytes("Content-Type", ctype).unwrap();
    Response::from_string(content).with_header(header).boxed()
}

/// CSRF対策ガード。悪意あるWebページがブラウザ経由で127.0.0.1のAPIを
/// 叩く攻撃(DNS非依存のローカルCSRF)を遮断する。
/// - POST以外を拒否(405)
/// - Content-Type は application/json のみ許可(415)。text/plain等の
///   「プリフライトが発生しないシンプルリクエスト」を排除する
/// - Originヘッダがある場合(=ブラウザ由来)は localhost / 127.0.0.1 のみ許可(403)。
///   curl等の非ブラウザクライアントはOriginを送らないため影響しない
fn api_guard(request: &Request) -> Option<tiny_http::ResponseBox> {
    let err = |code: u16, msg: &str| {
        Some(
            Response::from_string(format!("{{\"ok\":false,\"error\":\"{msg}\"}}"))
                .with_status_code(code)
                .with_header(
                    Header::from_bytes("Content-Type", "application/json; charset=utf-8").unwrap(),
                )
                .boxed(),
        )
    };
    if request.method() != &Method::Post {
        return err(405, "APIはPOSTのみ受け付けます");
    }
    let ctype = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Content-Type"))
        .map(|h| h.value.as_str().to_ascii_lowercase())
        .unwrap_or_default();
    if !ctype.starts_with("application/json") {
        return err(415, "Content-Typeはapplication/jsonを指定してください");
    }
    if let Some(origin) = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Origin"))
        .map(|h| h.value.as_str().to_string())
    {
        // "null"(file://等)や外部サイトのOriginはここで弾かれる
        let allowed = origin == "http://127.0.0.1"
            || origin == "http://localhost"
            || origin.starts_with("http://127.0.0.1:")
            || origin.starts_with("http://localhost:");
        if !allowed {
            return err(403, "許可されていないオリジンからのリクエストです");
        }
    }
    None
}

fn s(v: &Json, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn handle_api(state: &mut AppState, path: &str, req: &Json) -> BiResult<Json> {
    match path {
        "/api/browse" => api_browse(state, req),
        "/api/objects" => api_objects(state, req),
        "/api/preview" => api_preview(state, req),
        "/api/import" => api_import(state, req),
        "/api/datasets" => api_datasets(state),
        "/api/dataset/delete" => api_dataset_delete(state, req),
        "/api/query" => api_query(state, req),
        "/api/charts/get" => Ok(json!(state.charts)),
        "/api/charts/set" => {
            state.charts = req
                .get("charts")
                .and_then(|c| c.as_array())
                .cloned()
                .unwrap_or_default();
            Ok(json!({"count": state.charts.len()}))
        }
        "/api/analyze/profile" => crate::analysis::api_profile(state, req),
        "/api/analyze/regression" => crate::analysis::api_regression(state, req),
        "/api/analyze/cluster" => crate::analysis::api_cluster(state, req),
        "/api/analyze/advise" => crate::analysis::api_advise(state, req),
        "/api/analyze/test" => crate::analysis::api_test(state, req),
        "/api/project/save" => api_project_save(state, req),
        "/api/project/load" => api_project_load(state, req),
        "/api/state" => Ok(json!({
            "project_name": state.project_name,
            "datasets": state.datasets,
            "charts": state.charts,
            "queries": state.queries,
        })),
        _ => Err(format!("不明なAPI: {path}")),
    }
}

/// ファイルブラウザ: 指定ディレクトリのサブディレクトリと対応ファイルを返す
fn api_browse(state: &AppState, req: &Json) -> BiResult<Json> {
    let mut path = s(req, "path");
    if path.is_empty() {
        path = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".to_string());
    }
    let p = PathBuf::from(&path);
    let p = if p.is_dir() {
        p
    } else {
        p.parent().map(|x| x.to_path_buf()).unwrap_or(p)
    };
    let exts = state.registry.all_extensions();
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<Json> = Vec::new();
    let entries = std::fs::read_dir(&p).map_err(|e| format!("フォルダを開けません: {e}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.starts_with('$') {
            continue;
        }
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ft.is_dir() {
            dirs.push(name);
        } else {
            let ext = Path::new(&name)
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if exts.contains(&ext.as_str()) {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                files.push(json!({"name": name, "size": size}));
            }
        }
    }
    dirs.sort_by_key(|a| a.to_lowercase());
    files.sort_by_key(|a| a["name"].as_str().unwrap_or("").to_lowercase());
    let parent = p.parent().map(|x| x.to_string_lossy().to_string());
    Ok(json!({
        "path": p.to_string_lossy(),
        "parent": parent,
        "dirs": dirs,
        "files": files,
    }))
}

fn connector_for<'a>(state: &'a AppState, path: &Path) -> BiResult<&'a dyn Connector> {
    state.registry.for_path(path).ok_or_else(|| {
        format!(
            "未対応のファイル形式・接続URLです: {}(対応スキーム: postgres:// mysql://)",
            path.display()
        )
    })
}

fn api_objects(state: &AppState, req: &Json) -> BiResult<Json> {
    let path = PathBuf::from(s(req, "path"));
    let conn = connector_for(state, &path)?;
    let objects = conn.list_objects(&path)?;
    Ok(json!({
        "connector": conn.connector_type(),
        "objects": objects,
    }))
}

/// Parquetキャッシュ付きロード。有効なキャッシュがあればそこから復元し、
/// ミスならコネクタで読み込んでキャッシュへ書き戻す(Cache-Aside方式)。
/// 戻り値の bool はキャッシュヒットしたかどうか。
fn load_with_cache(
    state: &AppState,
    path: &Path,
    object: &str,
    opts: &ImportOptions,
) -> BiResult<(TableData, bool)> {
    if state.use_cache {
        if let Some(td) = bi_connectors::parquet_cache::load(path, object, opts) {
            return Ok((td, true));
        }
    }
    let conn = connector_for(state, path)?;
    let td = conn.load(path, object, opts)?;
    if state.use_cache {
        // キャッシュ書き込みの失敗でインポートを止めない(高速化はベストエフォート)
        if let Err(e) = bi_connectors::parquet_cache::store(path, object, opts, &td) {
            eprintln!("Parquetキャッシュの保存に失敗(処理は継続): {e}");
        }
    }
    Ok((td, false))
}

fn parse_options(req: &Json) -> ImportOptions {
    req.get("options")
        .and_then(|o| serde_json::from_value(o.clone()).ok())
        .unwrap_or_default()
}

fn table_json(td: &TableData) -> Json {
    json!({
        "columns": td.schema.columns.iter().map(|c| json!({
            "name": c.name,
            "type": c.data_type.name(),
        })).collect::<Vec<_>>(),
        "rows": td.rows,
    })
}

fn api_preview(state: &AppState, req: &Json) -> BiResult<Json> {
    let path = PathBuf::from(s(req, "path"));
    let object = s(req, "object");
    let mut opts = parse_options(req);
    opts.max_rows = Some(opts.max_rows.unwrap_or(50).min(500));
    let conn = connector_for(state, &path)?;
    let td = conn.load(&path, &object, &opts)?;
    Ok(table_json(&td))
}

fn api_import(state: &mut AppState, req: &Json) -> BiResult<Json> {
    let path_s = s(req, "path");
    let path = PathBuf::from(&path_s);
    let object = s(req, "object");
    let mut name = s(req, "name");
    if name.trim().is_empty() {
        name = path
            .file_stem()
            .map(|x| x.to_string_lossy().to_string())
            .unwrap_or_else(|| "dataset".to_string());
    }
    let name = sanitize_dataset_name(&name);
    let mut opts = parse_options(req);
    opts.max_rows = match opts.max_rows {
        Some(n) => Some(n.min(MAX_IMPORT_ROWS)),
        None => Some(MAX_IMPORT_ROWS),
    };
    let (td, from_cache) = load_with_cache(state, &path, &object, &opts)?;
    let row_count = td.rows.len();
    if from_cache {
        println!("データセット「{name}」をParquetキャッシュから復元({row_count}行)");
    }
    state.engine.register(&name, &td)?;
    let def = DatasetDef {
        name: name.clone(),
        path: path_s,
        object,
        options: ImportOptions {
            max_rows: None,
            ..opts
        },
        row_count,
        schema: Some(td.schema.clone()),
    };
    state.datasets.retain(|d| d.name != name);
    state.datasets.push(def);
    Ok(
        json!({"name": name, "rows": row_count, "schema": table_json(&TableData{schema: td.schema, rows: vec![]})["columns"]}),
    )
}

pub(crate) fn sanitize_dataset_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || (c as u32) > 127 {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "dataset".to_string()
    } else {
        cleaned
    }
}

fn api_datasets(state: &AppState) -> BiResult<Json> {
    Ok(json!(state.datasets))
}

fn api_dataset_delete(state: &mut AppState, req: &Json) -> BiResult<Json> {
    let name = s(req, "name");
    state.engine.drop_table(&name)?;
    state.datasets.retain(|d| d.name != name);
    Ok(json!({"deleted": name}))
}

fn api_query(state: &mut AppState, req: &Json) -> BiResult<Json> {
    let sql = s(req, "sql");
    if sql.trim().is_empty() {
        return Err("SQLが空です".to_string());
    }
    let limit = req
        .get("limit")
        .and_then(|x| x.as_u64())
        .unwrap_or(5000)
        .min(100_000) as usize;
    let r = state.engine.query(&sql, limit)?;
    // 実行履歴(最新20件)
    let trimmed = sql.trim().to_string();
    state.queries.retain(|q| q != &trimmed);
    state.queries.insert(0, trimmed);
    state.queries.truncate(20);
    Ok(json!({
        "columns": r.columns,
        "rows": r.rows,
        "truncated": r.truncated,
        "total_returned": r.total_returned,
    }))
}

fn api_project_save(state: &mut AppState, req: &Json) -> BiResult<Json> {
    let mut path = s(req, "path");
    if path.trim().is_empty() {
        return Err("保存先パスを指定してください".to_string());
    }
    if !path.to_lowercase().ends_with(".kohaku") && !path.to_lowercase().ends_with(".json") {
        path.push_str(".kohaku");
    }
    let name = Path::new(&path)
        .file_stem()
        .map(|x| x.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".into());
    state.project_name = name.clone();
    let project = Project {
        version: 1,
        name,
        datasets: state.datasets.clone(),
        charts: state.charts.clone(),
        queries: state.queries.clone(),
    };
    let text = serde_json::to_string_pretty(&project).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("保存に失敗: {e}"))?;
    Ok(json!({"path": path}))
}

fn api_project_load(state: &mut AppState, req: &Json) -> BiResult<Json> {
    let path = s(req, "path");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("読み込みに失敗: {e}"))?;
    let project: Project =
        serde_json::from_str(&text).map_err(|e| format!("プロジェクト形式が不正: {e}"))?;

    // 状態をリセットして各データセットを再インポート
    state.engine = Engine::new()?;
    state.datasets.clear();
    state.charts = project.charts;
    state.queries = project.queries;
    state.project_name = project.name;

    let mut errors: Vec<String> = Vec::new();
    let mut cached_count = 0usize;
    for def in project.datasets {
        if def.path.starts_with('(') {
            errors.push(format!(
                "{}: 分析結果の派生データセットはプロジェクトに保存されません。分析を再実行してください",
                def.name
            ));
            continue;
        }
        let p = PathBuf::from(&def.path);
        let mut opts = def.options.clone();
        opts.max_rows = Some(MAX_IMPORT_ROWS);
        match load_with_cache(state, &p, &def.object, &opts) {
            Ok((td, from_cache)) => {
                let rows = td.rows.len();
                if let Err(e) = state.engine.register(&def.name, &td) {
                    errors.push(format!("{}: {}", def.name, e));
                } else {
                    if from_cache {
                        cached_count += 1;
                        println!(
                            "データセット「{}」をParquetキャッシュから復元({rows}行)",
                            def.name
                        );
                    }
                    state.datasets.push(DatasetDef {
                        row_count: rows,
                        schema: Some(td.schema),
                        options: def.options,
                        ..def
                    });
                }
            }
            Err(e) => errors.push(format!("{}: {}", def.name, e)),
        }
    }
    Ok(json!({
        "project_name": state.project_name,
        "datasets": state.datasets,
        "charts": state.charts,
        "queries": state.queries,
        "errors": errors,
        "cached_datasets": cached_count,
    }))
}
