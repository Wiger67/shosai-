//! shosai-core：書齋的核心函式庫。
//!
//! - [`library`]：掃描資料夾、整理檔案（新增 / 改名 / 移到回收桶）
//! - [`book`]：以資料夾 + `book.toml` 表示一本書，管理章節順序
//! - [`markdown`]：Markdown 小工具（標題、大綱、字數）
//! - [`export`]：輸出 HTML、Typst 原始碼與 PDF
//! - [`sync`]：（`nextcloud` feature）透過 WebDAV 與 Nextcloud 同步

pub mod book;
pub mod export;
pub mod hmd;
pub mod library;
pub mod markdown;
pub mod math;
pub mod novel;
#[cfg(feature = "nextcloud")]
pub mod sync;

pub use book::{Book, BookMeta, Chapter, Layout};
pub use library::{Entry, EntryKind, Library};

/// 本函式庫所有可能的錯誤。
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Toml(String),
    Export(String),
    Sync(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "檔案錯誤：{e}"),
            Error::Toml(e) => write!(f, "book.toml 格式錯誤：{e}"),
            Error::Export(e) => write!(f, "輸出失敗：{e}"),
            Error::Sync(e) => write!(f, "同步失敗：{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// 判斷路徑是否為 PDF 檔。
pub fn is_pdf(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// 判斷路徑是否為 Markdown 檔。
pub fn is_markdown(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("md" | "markdown" | "mdown" | "mkd")
    )
}

/// 由 Unix 日數換算公曆日期（Howard Hinnant 演算法）。
pub fn civil_from_days(z: i64) -> (i32, u8, u8) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let y = (yoe + era * 400 + i64::from(m <= 2)) as i32;
    (y, m, d)
}

/// 目前時間（UTC），格式 `YYYY-MM-DD HHMMSS`，用於檔名。
pub fn timestamp() -> String {
    let secs =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02} {:02}{:02}{:02}", t / 3600, t % 3600 / 60, t % 60)
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates() {
        assert_eq!(super::civil_from_days(0), (1970, 1, 1));
        assert_eq!(super::civil_from_days(20_396), (2025, 11, 4));
        assert_eq!(super::civil_from_days(-1), (1969, 12, 31));
    }
}
