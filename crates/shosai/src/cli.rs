//! 命令列模式（無視窗），方便在 homelab / 伺服器上自動化。

use std::path::{Path, PathBuf};

use shosai_core::export::{self, Document};
use shosai_core::{book, Book};

const HELP: &str = "書齋 Shosai — 輕量 Markdown 寫作與閱讀器

用法：
  shosai [資料夾或 .md 檔]             開啟視窗
  shosai export <書本資料夾或 .md> [-o 輸出檔]
                                       輸出成 PDF（預設）、.html 或 .typ
  shosai new-book <資料夾> [書名]       建立一本新書
  shosai sync <書庫資料夾>              與 Nextcloud 同步（需 nextcloud 功能）
  shosai --help | --version
";

/// 處理子命令；回傳 `Some(exit code)` 表示已處理完畢、不需開啟視窗。
pub fn run(args: &[String]) -> Option<i32> {
    let cmd = args.first()?.as_str();
    let rest = &args[1..];
    let result = match cmd {
        "-h" | "--help" | "help" => {
            print!("{HELP}");
            Ok(())
        }
        "-V" | "--version" => {
            println!("shosai {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "export" => export_cmd(rest),
        "new-book" => new_book(rest),
        "sync" => sync_cmd(rest),
        _ => return None,
    };
    Some(match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("錯誤：{e}");
            1
        }
    })
}

fn export_cmd(args: &[String]) -> Result<(), String> {
    let mut input = None;
    let mut output = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" | "--output" => output = it.next().map(PathBuf::from),
            _ => input = Some(PathBuf::from(a)),
        }
    }
    let input = input.ok_or("請指定要輸出的書本資料夾或 Markdown 檔")?;
    let doc = load_document(&input)?;
    let output = output.unwrap_or_else(|| default_output(&input, &doc.title, "pdf"));
    let started = std::time::Instant::now();
    export::export_to(&doc, &output).map_err(|e| e.to_string())?;
    println!("已輸出 {}（{:.1} 秒）", output.display(), started.elapsed().as_secs_f32());
    Ok(())
}

pub fn load_document(input: &Path) -> Result<Document, String> {
    if input.is_dir() {
        if !book::is_book(input) {
            return Err(format!("{} 不是書本資料夾（缺少 book.toml）", input.display()));
        }
        Book::open(input).and_then(|b| b.to_document()).map_err(|e| e.to_string())
    } else {
        let text = std::fs::read_to_string(input).map_err(|e| format!("無法讀取 {}：{e}", input.display()))?;
        Ok(Document::from_markdown(input, text))
    }
}

/// 書本 → 輸出到書本資料夾內；單檔 → 同名不同副檔名。
pub fn default_output(input: &Path, title: &str, ext: &str) -> PathBuf {
    if input.is_dir() {
        input.join(format!("{}.{ext}", shosai_core::library::sanitize_name(title)))
    } else {
        input.with_extension(ext)
    }
}

fn new_book(args: &[String]) -> Result<(), String> {
    let dir = args.first().map(PathBuf::from).ok_or("請指定資料夾")?;
    let title = args.get(1).cloned().unwrap_or_else(|| {
        dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "未命名的書".into())
    });
    if book::is_book(&dir) {
        return Err(format!("{} 已經是一本書", dir.display()));
    }
    Book::create(&dir, &title, "").map_err(|e| e.to_string())?;
    println!("已建立《{title}》於 {}", dir.display());
    Ok(())
}

#[cfg(feature = "nextcloud")]
fn sync_cmd(args: &[String]) -> Result<(), String> {
    use shosai_core::sync::{sync, SyncConfig};
    let dir = args.first().map(PathBuf::from).ok_or("請指定書庫資料夾")?;
    let cfg = SyncConfig::load(&dir).map_err(|e| e.to_string())?;
    let report = sync(&dir, &cfg).map_err(|e| e.to_string())?;
    println!("{report}");
    Ok(())
}

#[cfg(not(feature = "nextcloud"))]
fn sync_cmd(_args: &[String]) -> Result<(), String> {
    Err("此版本未啟用 Nextcloud 同步，請以 `cargo build --features nextcloud` 重新編譯".into())
}
