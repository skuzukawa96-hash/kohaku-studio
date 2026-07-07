//! Kohaku Studio: 軽量BIツール。
//! 単一バイナリでローカルWebサーバーを起動し、既定ブラウザでUIを開く。
//!
//! 使い方:
//!   kohaku-studio.exe                    ... 起動(ブラウザ自動オープン、ポート5590)
//!   kohaku-studio.exe --port 8080        ... ポート指定
//!   kohaku-studio.exe --no-browser      ... ブラウザを開かない
//!   kohaku-studio.exe --make-samples DIR ... サンプルデータ(CSV/SQLite)を生成

mod analysis;
mod engine;
mod server;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut port: u16 = 5590;
    let mut open_browser = true;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                if let Some(p) = args.get(i + 1).and_then(|x| x.parse().ok()) {
                    port = p;
                }
                i += 1;
            }
            "--no-browser" => open_browser = false,
            "--make-samples" => {
                let dir = args.get(i + 1).cloned().unwrap_or_else(|| ".".to_string());
                match make_samples(&dir) {
                    Ok(_) => println!("サンプルデータを {dir} に生成しました"),
                    Err(e) => eprintln!("サンプル生成に失敗: {e}"),
                }
                return;
            }
            "--help" | "-h" => {
                println!("kohaku-studio [--port N] [--no-browser] [--make-samples DIR]");
                return;
            }
            _ => {}
        }
        i += 1;
    }
    if let Err(e) = server::run(port, open_browser) {
        eprintln!("起動エラー: {e}");
        std::process::exit(1);
    }
}

/// 動作確認用のサンプルデータ生成
fn make_samples(dir: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    // CSV: 売上データ
    let mut csv = String::from("date,region,product,units,unit_price\n");
    let regions = ["東京", "大阪", "名古屋", "福岡"];
    let products = ["ProductA", "ProductB", "ProductC"];
    let mut seed: u64 = 42;
    let mut rand = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as f64 / (u32::MAX as f64) * 2.0
    };
    for day in 1..=90 {
        let m = 1 + (day - 1) / 30;
        let d = 1 + (day - 1) % 30;
        for (ri, r) in regions.iter().enumerate() {
            for (pi, p) in products.iter().enumerate() {
                let units = (10.0 + ri as f64 * 5.0 + pi as f64 * 3.0 + rand() * 20.0) as i64;
                let price = 1000 + pi as i64 * 500;
                csv.push_str(&format!("2026-{m:02}-{d:02},{r},{p},{units},{price}\n"));
            }
        }
    }
    let csv_path = format!("{dir}/sample_sales.csv");
    std::fs::write(&csv_path, csv).map_err(|e| e.to_string())?;

    // SQLite: 装置マスタ + 測定データ
    let db_path = format!("{dir}/sample_fab.db");
    let _ = std::fs::remove_file(&db_path);
    let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "CREATE TABLE tools(tool_id TEXT PRIMARY KEY, tool_type TEXT, install_year INTEGER);
         INSERT INTO tools VALUES
           ('ETCH-01','etcher',2019),('ETCH-02','etcher',2022),
           ('CVD-01','cvd',2020),('CVD-02','cvd',2023),('LITHO-01','litho',2021);",
    )
    .map_err(|e| e.to_string())?;
    conn.execute_batch("CREATE TABLE measurements(lot_id TEXT, tool_id TEXT, step INTEGER, yield REAL, defects INTEGER);")
        .map_err(|e| e.to_string())?;
    let tools = ["ETCH-01", "ETCH-02", "CVD-01", "CVD-02", "LITHO-01"];
    let mut stmt = conn
        .prepare("INSERT INTO measurements VALUES (?,?,?,?,?)")
        .map_err(|e| e.to_string())?;
    for lot in 1..=200 {
        let tool = tools[lot % tools.len()];
        let base = 90.0 + (lot % tools.len()) as f64 * 1.5;
        let y = base + rand() * 4.0 - 4.0;
        let defects = ((100.0 - y) * 3.0) as i64;
        stmt.execute(rusqlite::params![
            format!("LOT{lot:04}"),
            tool,
            lot % 5 + 1,
            (y * 100.0).round() / 100.0,
            defects
        ])
        .map_err(|e| e.to_string())?;
    }
    drop(stmt);
    println!("  {csv_path}");
    println!("  {db_path}");
    Ok(())
}
