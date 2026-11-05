//! 輸出：HTML（單一檔案、可直接列印）、Typst 原始碼、PDF。

use std::path::{Path, PathBuf};

use crate::book::Layout;
use crate::Result;

pub mod html;
pub mod pdf;
pub mod typst;

/// 一份待輸出的文件：可以是整本書，也可以是單一 Markdown 檔。
#[derive(Debug, Clone)]
pub struct Document {
    pub title: String,
    pub subtitle: String,
    pub author: String,
    pub language: String,
    pub layout: Layout,
    /// 圖片等資源的根目錄（PDF 只能讀取此目錄下的檔案）。
    pub root: PathBuf,
    pub chapters: Vec<DocChapter>,
    /// 整本書會產生封面、目錄，且每章另起新頁。
    pub is_book: bool,
}

#[derive(Debug, Clone)]
pub struct DocChapter {
    pub title: String,
    pub markdown: String,
    /// 章節檔所在資料夾，用於解析相對圖片路徑。
    pub base_dir: PathBuf,
}

impl Document {
    /// 由單一 Markdown 檔建立文件（不含封面與目錄）。
    pub fn from_markdown(path: &Path, text: String) -> Document {
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let title = crate::book::chapter_title(path);
        let layout = Layout { cover: false, toc: false, paper: "a4".into(), ..Layout::default() };
        Document {
            title: title.clone(),
            subtitle: String::new(),
            author: String::new(),
            language: "zh-TW".into(),
            layout,
            root: base.clone(),
            chapters: vec![DocChapter { title, markdown: text, base_dir: base }],
            is_book: false,
        }
    }
}

/// 解析 Markdown 中的本機圖片路徑；網路圖片或找不到的檔案回傳 `None`。
pub(crate) fn resolve_local_image(base_dir: &Path, url: &str) -> Option<PathBuf> {
    if url.contains("://") || url.starts_with("data:") {
        return None;
    }
    let url = url.split(['?', '#']).next().unwrap_or(url);
    let decoded = percent_decode(url);
    let p = Path::new(&decoded);
    let path = if p.is_absolute() { p.to_path_buf() } else { base_dir.join(p) };
    path.is_file().then_some(path)
}

/// 解碼網址中的 `%XX`（例如連結中的中文檔名）。
pub fn percent_decode_pub(s: &str) -> String {
    percent_decode(s)
}

pub(crate) fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 依副檔名寫出：`.pdf` / `.html` / `.typ`。
pub fn export_to(doc: &Document, out: &Path) -> Result<()> {
    let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => std::fs::write(out, html::render(doc))?,
        "typ" => std::fs::write(out, typst::render(doc).source)?,
        _ => std::fs::write(out, pdf::render(doc)?)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode() {
        assert_eq!(percent_decode("a%20b%E4%B8%80"), "a b一");
        assert_eq!(percent_decode("100%"), "100%");
    }
}
