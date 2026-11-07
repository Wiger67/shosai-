//! 書齋 Shosai：輕量的 Markdown 寫作與閱讀器。
//!
//! ```text
//! shosai [資料夾或檔案]              開啟 GUI
//! shosai export <書或.md> [-o 輸出]   輸出 PDF / HTML / Typst
//! shosai new-book <資料夾> [書名]     建立新書
//! ```

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod cli;
mod icons;
mod images;
mod md_view;
mod novel_view;
mod pdf_view;
mod theme;
mod tree;

/// 程式啟動時間（用於 `SHOSAI_STARTUP_TRACE` 量測啟動速度）。
pub static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn main() {
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
