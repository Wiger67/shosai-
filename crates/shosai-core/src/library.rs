//! 書庫：一個根資料夾，裡面可放散落的 Markdown 筆記、資料夾與書本。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::book::is_book;
use crate::{is_markdown, is_pdf, Result};

/// 掃描時忽略的資料夾。
pub const IGNORED_DIRS: &[&str] = &["target", "node_modules", TRASH_DIR];
/// 刪除的檔案會被移到這裡，而不是直接消失。
pub const TRASH_DIR: &str = ".shosai-trash";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    Book,
    Markdown,
    Pdf,
}

impl EntryKind {
    pub fn is_file(&self) -> bool {
        matches!(self, EntryKind::Markdown | EntryKind::Pdf)
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
    pub children: Vec<Entry>,
    /// 修改時間；資料夾為裡面最新的檔案
    pub modified: Option<SystemTime>,
    /// 建立時間（部分 Linux 檔案系統沒有，會退回修改時間）
    pub created: Option<SystemTime>,
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

    /// 依排序設定重新排列整棵樹。
    pub fn sort(&mut self, opts: &SortOptions, order: &CustomOrder) {
        let root = self.root.clone();
        sort_tree(&mut self.entries, &root, &root, opts, order);
    }

    /// 找出某個路徑對應的項目。
    pub fn find(&self, path: &Path) -> Option<&Entry> {
        fn walk<'a>(entries: &'a [Entry], path: &Path) -> Option<&'a Entry> {
            for e in entries {
                if e.path == path {
                    return Some(e);
                }
                if path.starts_with(&e.path) {
                    return walk(&e.children, path);
                }
            }
            None
        }
        walk(&self.entries, path)
    }

    /// 資料夾裡的項目（已排序）；`dir` 為書庫根目錄時回傳最上層。
    pub fn children_of(&self, dir: &Path) -> Option<&[Entry]> {
        if dir == self.root {
            Some(&self.entries)
        } else {
            self.find(dir).map(|e| e.children.as_slice())
        }
    }

    /// 所有檔案（Markdown 與 PDF，扁平），用於快速搜尋。
    pub fn all_files(&self) -> Vec<&Entry> {
        fn walk<'a>(entries: &'a [Entry], out: &mut Vec<&'a Entry>) {
            for e in entries {
                if e.kind.is_file() {
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
        let meta = entry.metadata().ok();
        let modified = meta.as_ref().and_then(|m| m.modified().ok());
        let created = meta.as_ref().and_then(|m| m.created().ok()).or(modified);
        let kind = if ft.is_dir() {
            if is_book(&path) {
                EntryKind::Book
            } else {
                EntryKind::Dir
            }
        } else if is_markdown(&path) {
            EntryKind::Markdown
        } else if is_pdf(&path) {
            EntryKind::Pdf
        } else {
            continue;
        };
        let children = if ft.is_dir() { scan(&path, depth + 1).unwrap_or_default() } else { Vec::new() };
        // 資料夾的「修改時間」取裡面最新的檔案，排序時才有意義
        let modified = children.iter().filter_map(|c| c.modified).chain(modified).max();
        out.push(Entry { path, name, kind, children, modified, created });
    }
    out.sort_by(|a, b| {
        let a_file = a.kind.is_file();
        let b_file = b.kind.is_file();
        a_file.cmp(&b_file).then_with(|| natural_cmp(&a.name, &b.name))
    });
    Ok(out)
}

// ---------------------------------------------------------------------------
// 排序

/// 檔案樹的排序依據。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SortKey {
    /// 名稱（自然排序：2 在 10 之前）
    #[default]
    Name,
    /// 修改時間（新的在前）
    Modified,
    /// 建立時間（新的在前）
    Created,
    /// 自訂（拖曳排列，存在 `.shosai/order.toml`）
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SortOptions {
    pub key: SortKey,
    /// 反過來排
    pub reverse: bool,
    /// 資料夾排在檔案前面（自訂排序時只用在還沒排過的項目）
    pub folders_first: bool,
}

impl Default for SortOptions {
    fn default() -> Self {
        Self { key: SortKey::Name, reverse: false, folders_first: true }
    }
}

/// 自訂排序：每個資料夾（相對於書庫，以 `/` 分隔，根目錄為空字串）裡項目的順序。
///
/// 存在書庫的 `.shosai/order.toml`，會跟著 Nextcloud 同步。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CustomOrder {
    #[serde(default)]
    pub folders: BTreeMap<String, Vec<String>>,
}

pub const ORDER_FILE: &str = ".shosai/order.toml";

impl CustomOrder {
    pub fn load(library_root: &Path) -> CustomOrder {
        fs::read_to_string(library_root.join(ORDER_FILE)).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, library_root: &Path) -> Result<()> {
        let path = library_root.join(ORDER_FILE);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| crate::Error::Toml(e.to_string()))?;
        fs::write(path, text)?;
        Ok(())
    }

    pub fn get(&self, library_root: &Path, dir: &Path) -> Option<&[String]> {
        self.folders.get(&rel_key(library_root, dir)).map(Vec::as_slice)
    }

    pub fn set(&mut self, library_root: &Path, dir: &Path, names: Vec<String>) {
        self.folders.insert(rel_key(library_root, dir), names);
    }

    /// 檔案改名後更新順序表中的名稱。
    pub fn rename(&mut self, library_root: &Path, old: &Path, new: &Path) -> bool {
        let (Some(dir), Some(old_name), Some(new_name)) = (old.parent(), old.file_name(), new.file_name()) else {
            return false;
        };
        let old_name = old_name.to_string_lossy();
        let mut changed = false;
        if let Some(list) = self.folders.get_mut(&rel_key(library_root, dir)) {
            for n in list.iter_mut().filter(|n| **n == old_name) {
                *n = new_name.to_string_lossy().into_owned();
                changed = true;
            }
        }
        // 資料夾改名：底下子資料夾的鍵也要跟著改
        let (old_key, new_key) = (rel_key(library_root, old), rel_key(library_root, new));
        let moved: Vec<String> =
            self.folders.keys().filter(|k| **k == old_key || k.starts_with(&format!("{old_key}/"))).cloned().collect();
        for k in moved {
            if let Some(v) = self.folders.remove(&k) {
                self.folders.insert(format!("{new_key}{}", &k[old_key.len()..]), v);
                changed = true;
            }
        }
        changed
    }
}

fn rel_key(root: &Path, dir: &Path) -> String {
    dir.strip_prefix(root)
        .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
        .unwrap_or_default()
}

fn sort_tree(entries: &mut [Entry], root: &Path, dir: &Path, opts: &SortOptions, order: &CustomOrder) {
    sort_level(entries, order.get(root, dir), opts);
    for e in entries.iter_mut() {
        if !e.children.is_empty() {
            let path = e.path.clone();
            sort_tree(&mut e.children, root, &path, opts, order);
        }
    }
}

/// 依排序設定排列一層項目。`custom` 是這個資料夾的自訂順序（沒列到的接在後面，依名稱）。
pub fn sort_level(entries: &mut [Entry], custom: Option<&[String]>, opts: &SortOptions) {
    use std::cmp::Ordering;
    let by_name = |a: &Entry, b: &Entry| natural_cmp(&a.name, &b.name);
    let group = |a: &Entry, b: &Entry| -> Ordering {
        if opts.folders_first {
            a.kind.is_file().cmp(&b.kind.is_file())
        } else {
            Ordering::Equal
        }
    };
    let pos = |e: &Entry| custom.and_then(|c| c.iter().position(|n| *n == e.name)).unwrap_or(usize::MAX);
    entries.sort_by(|a, b| {
        let primary = match opts.key {
            SortKey::Name => by_name(a, b),
            // 新的在前；沒有時間的排最後
            SortKey::Modified => b.modified.cmp(&a.modified),
            SortKey::Created => b.created.cmp(&a.created),
            SortKey::Custom => pos(a).cmp(&pos(b)),
        };
        let primary = if opts.reverse { primary.reverse() } else { primary };
        if opts.key == SortKey::Custom {
            // 自訂順序優先；還沒排過的項目照預設規則接在後面
            primary.then(group(a, b)).then_with(|| by_name(a, b))
        } else {
            group(a, b).then(primary).then_with(|| by_name(a, b))
        }
    });
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

    fn entry(name: &str, kind: EntryKind, secs: u64) -> Entry {
        let t = Some(UNIX_EPOCH + std::time::Duration::from_secs(secs));
        Entry { path: PathBuf::from(name), name: name.into(), kind, children: Vec::new(), modified: t, created: t }
    }

    #[test]
    fn sort_options() {
        let mut v = vec![
            entry("b.md", EntryKind::Markdown, 30),
            entry("資料夾", EntryKind::Dir, 10),
            entry("a.md", EntryKind::Markdown, 20),
        ];
        let names = |v: &[Entry]| v.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        sort_level(&mut v, None, &SortOptions::default());
        assert_eq!(names(&v), ["資料夾", "a.md", "b.md"]);
        sort_level(&mut v, None, &SortOptions { key: SortKey::Modified, reverse: false, folders_first: false });
        assert_eq!(names(&v), ["b.md", "a.md", "資料夾"]);
        sort_level(&mut v, None, &SortOptions { key: SortKey::Name, reverse: true, folders_first: true });
        assert_eq!(names(&v), ["資料夾", "b.md", "a.md"]);
        let custom = vec!["b.md".to_string(), "資料夾".to_string()];
        sort_level(&mut v, Some(&custom), &SortOptions { key: SortKey::Custom, ..Default::default() });
        assert_eq!(names(&v), ["b.md", "資料夾", "a.md"]);
    }

    #[test]
    fn custom_order_rename() {
        let root = Path::new("/lib");
        let mut o = CustomOrder::default();
        o.set(root, root, vec!["甲".into(), "乙.md".into()]);
        o.set(root, &root.join("甲"), vec!["x.md".into()]);
        o.set(root, &root.join("甲/子"), vec!["y.md".into()]);
        assert!(o.rename(root, &root.join("甲"), &root.join("丙")));
        assert_eq!(o.get(root, root).unwrap(), ["丙", "乙.md"]);
        assert_eq!(o.get(root, &root.join("丙")).unwrap(), ["x.md"]);
        assert_eq!(o.get(root, &root.join("丙/子")).unwrap(), ["y.md"]);
        let text = toml::to_string_pretty(&o).unwrap();
        assert_eq!(toml::from_str::<CustomOrder>(&text).unwrap(), o);
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
        fs::write(dir.join("書.pdf"), b"%PDF-1.4").unwrap();
        let lib = Library::open(&dir).unwrap();
        assert_eq!(lib.all_files().len(), 2);
        assert!(lib.all_files().iter().any(|e| e.kind == EntryKind::Pdf));
        fs::remove_file(dir.join("書.pdf")).unwrap();
        trash(&dir, &renamed).unwrap();
        let lib = Library::open(&dir).unwrap();
        assert_eq!(lib.all_files().len(), 0);
        fs::remove_dir_all(dir).unwrap();
    }
}
