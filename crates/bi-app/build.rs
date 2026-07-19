//! Windows向けに実行ファイルのアイコンを埋め込む。
//! ビルド時にだけ動くため、実行時のサイズ・速度・メモリには影響しない。
//! Windows以外では何もしない。

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/app.rc");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // リソースコンパイラが無い環境ではスキップされる(ビルドは止めない)。
        // 実際に失敗した場合だけエラーにして、アイコン欠落を見逃さない。
        embed_resource::compile("assets/app.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("アイコンの埋め込みに失敗しました");
    }
}
