//! 書本：一個含有 `book.toml` 的資料夾，章節為其中的 Markdown 檔。
//!
//! ```toml
//! title = "我的第一本書"
//! subtitle = "副標題"
//! author = "作者"
//! language = "zh-TW"
//! chapters = ["01-序章.md", "02-旅程.md"]
//!
//! [layout]
//! paper = "a5"
//! font_size = 10.5
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{is_markdown, markdown, Error, Result};

pub const BOOK_FILE: &str = "book.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BookMeta {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subtitle: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(default = "default_language")]
    pub language: String,
    /// 章節檔案（相對於書本資料夾），依此順序排列。
    #[serde(default)]
    pub chapters: Vec<String>,
    #[serde(default)]
    pub layout: Layout,
}

fn default_language() -> String {
    "zh-TW".into()
}

/// 排版設定（PDF / HTML 共用）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Layout {
    /// 紙張：a4、a5、b5、letter
    pub paper: String,
    /// 內文字級（pt）
    pub font_size: f32,
    /// 行距倍率
    pub line_height: f32,
    /// 段落首行縮排兩字（中文書常見）
    pub indent: bool,
    /// 章節編號（第一章、第二章…）
    pub numbered: bool,
    /// 產生封面
    pub cover: bool,
    /// 產生目錄
    pub toc: bool,
    /// 目錄深度
    pub toc_depth: u8,
    /// 內文字體（依序嘗試，找不到時自動退回系統字型）
    pub fonts: Vec<String>,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            paper: "a5".into(),
            font_size: 10.5,
            line_height: 1.8,
            indent: true,
            numbered: false,
            cover: true,
            toc: true,
            toc_depth: 2,
            fonts: vec![
                "Libertinus Serif".into(),
                "Noto Serif CJK TC".into(),
                "Source Han Serif TC".into(),
                "Noto Sans CJK TC".into(),
                "PingFang TC".into(),
                "Microsoft JhengHei".into(),
                "WenQuanYi Zen Hei".into(),
            ],
        }
    }
}

impl Layout {
    /// Typst 能辨識的紙張名稱。
    pub fn typst_paper(&self) -> &'static str {
        match self.paper.to_ascii_lowercase().as_str() {
            "a4" => "a4",
            "b5" => "iso-b5",
            "jis-b5" => "jis-b5",
            "letter" | "us-letter" => "us-letter",
            "a6" => "a6",
            _ => "a5",
        }
    }

    /// CSS `@page size`。
    pub fn css_paper(&self) -> &'static str {
        match self.typst_paper() {
            "a4" => "A4",
            "iso-b5" | "jis-b5" => "B5",
            "us-letter" => "letter",
            "a6" => "A6",
            _ => "A5",
        }
    }
}

/// 書中的一章。
#[derive(Debug, Clone, PartialEq)]
pub struct Chapter {
    /// 相對於書本資料夾的路徑（以 `/` 分隔）。
    pub rel: String,
    pub path: PathBuf,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct Book {
    pub root: PathBuf,
    pub meta: BookMeta,
    pub chapters: Vec<Chapter>,
}

/// 資料夾是否為一本書。
pub fn is_book(dir: &Path) -> bool {
    dir.join(BOOK_FILE).is_file()
}

impl Book {
    /// 讀取書本。`book.toml` 中未列出的 Markdown 檔會依檔名排序接在後面。
    pub fn open(root: &Path) -> Result<Book> {
        let text = fs::read_to_string(root.join(BOOK_FILE))?;
        let meta: BookMeta = toml::from_str(&text).map_err(|e| Error::Toml(e.to_string()))?;
        let mut book = Book { root: root.to_path_buf(), meta, chapters: Vec::new() };
        book.refresh()?;
        Ok(book)
    }

    /// 在 `root` 建立一本新書（含第一章）。
    pub fn create(root: &Path, title: &str, author: &str) -> Result<Book> {
        fs::create_dir_all(root)?;
        let first = "01-第一章.md";
        let first_path = root.join(first);
        if !first_path.exists() {
            fs::write(&first_path, "# 第一章\n\n從這裡開始寫作……\n")?;
        }
        let meta = BookMeta {
            title: title.to_string(),
            subtitle: String::new(),
            author: author.to_string(),
            language: default_language(),
            chapters: vec![first.to_string()],
            layout: Layout::default(),
        };
        let book = Book { root: root.to_path_buf(), meta, chapters: Vec::new() };
        book.save()?;
        Book::open(root)
    }

    /// 重新掃描章節檔案。
    pub fn refresh(&mut self) -> Result<()> {
        let mut found = Vec::new();
        collect_markdown(&self.root, &self.root, &mut found)?;
        found.sort();

        let mut order: Vec<String> = self.meta.chapters.iter().filter(|c| found.contains(c)).cloned().collect();
        for f in found {
            if !order.contains(&f) {
                order.push(f);
            }
        }
        self.meta.chapters = order;
        self.chapters = self
            .meta
            .chapters
            .iter()
            .map(|rel| {
                let path = self.root.join(rel);
                let title = chapter_title(&path);
                Chapter { rel: rel.clone(), path, title }
            })
            .collect();
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        let text = toml::to_string_pretty(&self.meta).map_err(|e| Error::Toml(e.to_string()))?;
        fs::write(self.root.join(BOOK_FILE), text)?;
        Ok(())
    }

    /// 章節上移（`delta = -1`）或下移（`delta = 1`）。
    pub fn move_chapter(&mut self, index: usize, delta: isize) -> Result<()> {
        let target = index as isize + delta;
        if index >= self.meta.chapters.len() || target < 0 || target as usize >= self.meta.chapters.len() {
            return Ok(());
        }
        self.meta.chapters.swap(index, target as usize);
        self.chapters.swap(index, target as usize);
        self.save()
    }

    /// 新增章節檔案，回傳其路徑。
    pub fn add_chapter(&mut self, title: &str) -> Result<PathBuf> {
        let n = self.meta.chapters.len() + 1;
        let safe = crate::library::sanitize_name(title);
        let mut name = format!("{n:02}-{safe}.md");
        let mut i = 2;
        while self.root.join(&name).exists() {
            name = format!("{n:02}-{safe}-{i}.md");
            i += 1;
        }
        let path = self.root.join(&name);
        fs::write(&path, format!("# {title}\n\n"))?;
        self.meta.chapters.push(name);
        self.save()?;
        self.refresh()?;
        Ok(path)
    }

    /// 讀取所有章節內容，供輸出使用。
    pub fn to_document(&self) -> Result<crate::export::Document> {
        let mut chapters = Vec::new();
        for ch in &self.chapters {
            let text = fs::read_to_string(&ch.path)?;
            let base = ch.path.parent().unwrap_or(&self.root).to_path_buf();
            chapters.push(crate::export::DocChapter { title: ch.title.clone(), markdown: text, base_dir: base });
        }
        Ok(crate::export::Document {
            title: self.meta.title.clone(),
            subtitle: self.meta.subtitle.clone(),
            author: self.meta.author.clone(),
            language: self.meta.language.clone(),
            layout: self.meta.layout.clone(),
            root: self.root.clone(),
            chapters,
            is_book: true,
        })
    }
}

/// 章節標題：第一個一級標題，否則為去掉編號前綴的檔名。
pub fn chapter_title(path: &Path) -> String {
    if let Ok(text) = fs::read_to_string(path) {
        if let Some(t) = markdown::title(&text) {
            return t;
        }
    }
    title_from_filename(path)
}

/// `01-第一章.md` → `第一章`
pub fn title_from_filename(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("未命名");
    let trimmed = stem.trim_start_matches(|c: char| c.is_ascii_digit()).trim_start_matches(['-', '_', ' ', '.']);
    if trimmed.is_empty() {
        stem.to_string()
    } else {
        trimmed.to_string()
    }
}

fn collect_markdown(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let ft = entry.file_type()?;
        if ft.is_dir() {
            // 子資料夾若本身是另一本書則略過
            if !is_book(&path) && !crate::library::IGNORED_DIRS.contains(&name.as_ref()) {
                collect_markdown(root, &path, out)?;
            }
        } else if is_markdown(&path) {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shosai-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn create_and_reorder() {
        let dir = tmp("book");
        let mut book = Book::create(&dir, "測試書", "我").unwrap();
        assert_eq!(book.chapters.len(), 1);
        book.add_chapter("旅程").unwrap();
        fs::write(dir.join("99-附錄.md"), "附錄內容").unwrap();
        book.refresh().unwrap();
        assert_eq!(book.chapters.len(), 3);
        assert_eq!(book.chapters[1].title, "旅程");
        assert_eq!(book.chapters[2].title, "附錄");
        book.move_chapter(1, -1).unwrap();
        let reopened = Book::open(&dir).unwrap();
        assert_eq!(reopened.chapters[0].title, "旅程");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn filename_title() {
        assert_eq!(title_from_filename(Path::new("03_序.md")), "序");
        assert_eq!(title_from_filename(Path::new("2024.md")), "2024");
    }
}
