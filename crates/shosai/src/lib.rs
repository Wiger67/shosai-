//! 書齋 Shosai：輕量的 Markdown 寫作與閱讀器。
//!
//! 桌面版的執行檔（`main.rs`）和 Android 版（`crates/shosai-android`）共用這個函式庫。
//!
//! ```text
//! shosai [資料夾或檔案]              開啟 GUI
//! shosai export <書或.md> [-o 輸出]   輸出 PDF / HTML / Typst
//! shosai new-book <資料夾> [書名]     建立新書
//! ```

#[cfg(target_os = "android")]
pub mod android;
mod app;
mod cli;
mod find;
mod icons;
mod images;
mod keys;
mod md_view;
mod novel_view;
mod pdf_view;
mod scroll_map;
#[cfg(test)]
mod shot;
mod theme;
mod tree;
mod window;

/// 程式啟動時間（用於 `SHOSAI_STARTUP_TRACE` 量測啟動速度）。
pub static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// `SHOSAI_STARTUP_TRACE` 開著時，印出啟動到現在經過的時間。
pub fn trace(what: &str) {
    if std::env::var_os("SHOSAI_STARTUP_TRACE").is_some() {
        if let Some(t) = STARTED.get() {
            eprintln!("{:>8.1} ms  {what}", t.elapsed().as_secs_f64() * 1000.0);
        }
    }
}

/// 桌面版的進入點：先看是不是命令列子命令，不是的話開啟視窗。
pub fn desktop_main() {
    STARTED.get_or_init(std::time::Instant::now);
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli::run(&args) {
        std::process::exit(code);
    }

    let open = args.first().map(std::path::PathBuf::from);
    if let Err(e) = app::run(open) {
        eprintln!("無法啟動視窗：{e}");
        std::process::exit(1);
    }
}
