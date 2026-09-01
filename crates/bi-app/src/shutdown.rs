//! Ctrl+C(および終了シグナル)を受けたときに、正常終了(コード0)する処理。
//!
//! ハンドラを入れないと、OSがプロセスを異常終了として打ち切るため、
//! `cargo run` が「process didn't exit successfully」と報告してしまう
//! (Windowsの終了コードは 0xC000013A = STATUS_CONTROL_C_EXIT)。
//! ユーザーは正常に止めただけなので、エラーに見えるのは誤解を招く。
//!
//! 依存を増やさないため、OSのAPIを自前で宣言して呼ぶ。
//! Windowsのkernel32・Unixのlibcはどちらも常にリンクされているので、
//! クレートを追加せずに使える。

/// 終了時に出すメッセージ(前後の改行込み)。
/// Unixのシグナルハンドラ内では文字列を組み立てられない
/// (メモリ確保は async-signal-safe でない)ため、
/// 完成形を定数で持ち、両プラットフォームで同じ文言を使う。
const BYE: &str = "\nKohaku Studio を終了しました。\n";

#[cfg(windows)]
mod imp {
    use std::io::Write;

    extern "system" {
        /// コンソール制御イベント(Ctrl+C等)のハンドラを登録する。
        /// add に 1 を渡すと追加、0 で解除。
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }

    const CTRL_C_EVENT: u32 = 0;
    const CTRL_BREAK_EVENT: u32 = 1;
    const CTRL_CLOSE_EVENT: u32 = 2;

    /// Windowsではこのハンドラを **OSが専用スレッドで呼ぶ** ため、
    /// print! や process::exit をそのまま使える
    /// (Unixのシグナルハンドラのような async-signal-safe の制約がない)。
    unsafe extern "system" fn handler(ctrl_type: u32) -> i32 {
        match ctrl_type {
            CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT => {
                print!("{}", super::BYE);
                // process::exit はスタック上のデストラクタを走らせないため、
                // バッファに残ったままにならないよう明示的に流し出す
                let _ = std::io::stdout().flush();
                std::process::exit(0);
            }
            // 扱わないイベントは 0 を返して既定の動作に任せる
            _ => 0,
        }
    }

    pub fn install() {
        // 登録に失敗しても致命的ではない(従来どおり異常終了になるだけ)ため、
        // 起動は止めずに続行する
        unsafe { SetConsoleCtrlHandler(Some(handler), 1) };
    }
}

#[cfg(unix)]
mod imp {
    extern "C" {
        /// シグナルハンドラを登録する。libcの sighandler_t は
        /// ポインタ幅の整数なので usize で受け渡しする。
        fn signal(sig: i32, handler: usize) -> usize;
        /// 低水準の書き出し。async-signal-safe。
        fn write(fd: i32, buf: *const u8, count: usize) -> isize;
        /// 後処理を走らせずに即座に終了する。async-signal-safe。
        fn _exit(code: i32) -> !;
    }

    const SIGINT: i32 = 2; // Ctrl+C
    const STDOUT: i32 = 1;

    // SIGTERM は意図的に既定のままにする。ここで捕まえて終了コード0にすると、
    // kill やサービス管理ツール・CIによる「外部からの強制終了」まで
    // 正常終了に見えてしまい、監視側が異常終了を検知できなくなる。
    // Ctrl+C の対応に必要なのは SIGINT だけ。

    /// Unixのシグナルハンドラ内では **async-signal-safe な関数しか呼べない**。
    /// print! は内部でロックを取るため、ロック保持中に割り込むとデッドロック
    /// しうる。std::process::exit も後処理を走らせるので安全ではない。
    /// そのため write と _exit だけで済ませる。
    extern "C" fn handler(_sig: i32) {
        unsafe {
            // 書けなくても終了は続ける(戻り値は使わない)
            let _ = write(STDOUT, super::BYE.as_ptr(), super::BYE.len());
            _exit(0);
        }
    }

    pub fn install() {
        // 関数を直接 usize へキャストすると function_casts_as_integer 警告が出るため、
        // いったんポインタを経由する
        let h = handler as *const () as usize;
        unsafe {
            signal(SIGINT, h);
        }
    }
}

/// 終了ハンドラを設置する。サーバーを起動する前に1回だけ呼ぶ。
pub fn install() {
    imp::install();
}
