//! 書庫：一個根資料夾，裡面可放散落的 Markdown 筆記、資料夾與書本。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::book::is_book;
use crate::{is_markdown, Result};

/// 掃描時忽略的資料夾。
pub const IGNORED_DIRS: &[&str] = &["target", "node_modules", TRASH_DIR];
/// 刪除的檔案會被移到這裡，而不是直接消失。
pub const TRASH_DIR: &str = ".shosai-trash";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    Book,
    Markdown,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
    pub children: Vec<Entry>,
}

#[derive(Debug, Clone)]
pub struct Library {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
}

impl Library {
    pub fn open(root: &Path) -> Result<Library> {
        let mut lib = Library { root: root.to_path_buf(), entries: Vec::new() };
        lib.refresh()?;
        Ok(lib)
    }

    pub fn refresh(&mut self) -> Result<()> {
        self.entries = scan(&self.root, 0)?;
        Ok(())
    }

    /// 所有 Markdown 檔（扁平），用於快速搜尋。
    pub fn all_files(&self) -> Vec<&Entry> {
        fn walk<'a>(entries: &'a [Entry], out: &mut Vec<&'a Entry>) {
            for e in entries {
                if e.kind == EntryKind::Markdown {
                    out.push(e);
                }
                walk(&e.children, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.entries, &mut out);
        out
    }
}

const MAX_DEPTH: usize = 8;

fn scan(dir: &Path, depth: usize) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    if depth > MAX_DEPTH {
        return Ok(out);
    }
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || IGNORED_DIRS.contains(&name.as_str()) {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            let kind = if is_book(&path) { EntryKind::Book } else { EntryKind::Dir };
            let children = scan(&path, depth + 1).unwrap_or_default();
            out.push(Entry { path, name, kind, children });
        } else if is_markdown(&path) {
            out.push(Entry { path, name, kind: EntryKind::Markdown, children: Vec::new() });
        }
    }
    out.sort_by(|a, b| {
        let a_file = a.kind == EntryKind::Markdown;
        let b_file = b.kind == EntryKind::Markdown;
        a_file.cmp(&b_file).then_with(|| natural_cmp(&a.name, &b.name))
    });
    Ok(out)
}

/// 自然排序：`2.md` 排在 `10.md` 之前。
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    bi.next();
                }
                let na_t = na.trim_start_matches('0');
                let nb_t = nb.trim_start_matches('0');
                let ord = na_t.len().cmp(&nb_t.len()).then_with(|| na_t.cmp(nb_t));
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

/// 將使用者輸入轉成安全的檔名。
pub fn sanitize_name(name: &str) -> String {
    let cleaned: String =
        name.trim()
            .chars()
            .map(|c| {
                if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() {
                    '-'
                } else {
                    c
                }
            })
            .collect();
    let cleaned = cleaned.trim_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        "未命名".into()
    } else {
        cleaned
    }
}

/// 在 `dir` 建立新的 Markdown 檔（自動避開重名），回傳路徑。
pub fn create_note(dir: &Path, name: &str) -> Result<PathBuf> {
    let base = sanitize_name(name.trim_end_matches(".md"));
    let mut path = dir.join(format!("{base}.md"));
    let mut i = 2;
    while path.exists() {
        path = dir.join(format!("{base} {i}.md"));
        i += 1;
    }
    let title = base.clone();
    fs::write(&path, format!("# {title}\n\n"))?;
    Ok(path)
}

/// 建立資料夾（自動避開重名）。
pub fn create_dir(dir: &Path, name: &str) -> Result<PathBuf> {
    let base = sanitize_name(name);
    let mut path = dir.join(&base);
    let mut i = 2;
    while path.exists() {
        path = dir.join(format!("{base} {i}"));
        i += 1;
    }
    fs::create_dir_all(&path)?;
    Ok(path)
}

/// 改名（同資料夾內）。Markdown 檔若沒寫副檔名會自動補上。
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf> {
    let mut name = sanitize_name(new_name);
    if path.is_file() && is_markdown(path) && !is_markdown(Path::new(&name)) {
        name.push_str(".md");
    }
    let target = path.with_file_name(name);
    if target.exists() && target != path {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "已有同名檔案").into());
    }
    fs::rename(path, &target)?;
    update_book_reference(path, &target);
    Ok(target)
}

/// 移動到另一個資料夾。
pub fn move_into(path: &Path, dir: &Path) -> Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| std::io::Error::other("無效路徑"))?;
    let target = dir.join(name);
    if target.exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "目的地已有同名檔案").into());
    }
    if dir.starts_with(path) {
        return Err(std::io::Error::other("不能把資料夾移到自己裡面").into());
    }
    fs::rename(path, &target)?;
    update_book_reference(path, &target);
    Ok(target)
}

/// 移到書庫根目錄下的 `.shosai-trash/`（可手動救回）。
pub fn trash(library_root: &Path, path: &Path) -> Result<PathBuf> {
    let trash = library_root.join(TRASH_DIR);
    fs::create_dir_all(&trash)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let target = trash.join(format!("{stamp}-{name}"));
    fs::rename(path, &target)?;
    Ok(target)
}

/// 檔案改名 / 移動後，若原本屬於某本書，就同步更新 `book.toml` 的章節清單。
fn update_book_reference(old: &Path, new: &Path) {
    let (Some(old_dir), Some(new_dir)) = (old.parent(), new.parent()) else { return };
    if old_dir != new_dir || !is_book(old_dir) {
        return;
    }
    let (Some(old_name), Some(new_name)) = (old.file_name(), new.file_name()) else { return };
    if let Ok(mut book) = crate::Book::open(old_dir) {
        let old_name = old_name.to_string_lossy();
        if let Some(slot) = book.meta.chapters.iter_mut().find(|c| **c == old_name) {
            *slot = new_name.to_string_lossy().into_owned();
        }
        let _ = book.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_sort() {
        let mut v = vec!["10.md", "2.md", "1.md", "b.md", "A.md"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["1.md", "2.md", "10.md", "A.md", "b.md"]);
    }

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_name("a/b:c"), "a-b-c");
        assert_eq!(sanitize_name("  "), "未命名");
    }

    #[test]
    fn rename_keeps_book_order() {
        let dir = std::env::temp_dir().join(format!("shosai-lib-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let book = crate::Book::create(&dir, "書", "").unwrap();
        let first = book.chapters[0].path.clone();
        let renamed = rename(&first, "01-開端").unwrap();
        assert!(renamed.ends_with("01-開端.md"));
        let book = crate::Book::open(&dir).unwrap();
        assert_eq!(book.meta.chapters, vec!["01-開端.md".to_string()]);
        let lib = Library::open(&dir).unwrap();
        assert_eq!(lib.all_files().len(), 1);
        trash(&dir, &renamed).unwrap();
        let lib = Library::open(&dir).unwrap();
        assert_eq!(lib.all_files().len(), 0);
        fs::remove_dir_all(dir).unwrap();
    }
}
