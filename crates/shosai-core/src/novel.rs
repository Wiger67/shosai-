//! 書本模式（像輕小說一樣分頁閱讀）的資料層。
//!
//! - 把一個資料夾整理成一本書：裡面的 Markdown 是章，子資料夾是卷
//! - 把 Markdown 簡化成適合分頁排版的段落（標題、內文、場景分隔、插圖…）
//! - 支援小說網站常見的注音（ルビ）寫法：`｜漢字《かんじ》`

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::hmd::{self, Block, Inline};
use crate::library::{Entry, EntryKind};
use crate::Book;

/// 一章。
#[derive(Debug, Clone, PartialEq)]
pub struct NovelChapter {
    pub path: PathBuf,
    pub title: String,
    /// 所屬的卷（子資料夾名稱）；最上層的章為 `None`
    pub volume: Option<String>,
}

/// 以資料夾為單位的一本書。
#[derive(Debug, Clone, PartialEq)]
pub struct NovelBook {
    pub root: PathBuf,
    pub title: String,
    pub author: String,
    /// 封面圖（資料夾中名為 cover / 封面 / 表紙 的圖片）
    pub cover: Option<PathBuf>,
    pub chapters: Vec<NovelChapter>,
}

/// 章名快取：路徑 →（檔案修改時間、章名）。
///
/// 章名要讀檔案裡的第一個標題才知道；檔案樹每次更新都會重建一次書，
/// 有了快取就只需要重讀真的改過的那幾章。
pub type TitleCache = HashMap<PathBuf, (Option<SystemTime>, String)>;

impl NovelBook {
    /// 由檔案樹建立。`children` 是這個資料夾裡已排序的項目；
    /// `book` 為 `book.toml` 書本時使用它的章節順序與書名。
    pub fn new(root: &Path, children: &[Entry], book: Option<&Book>) -> NovelBook {
        Self::with_cache(root, children, book, &mut TitleCache::new())
    }

    /// 同 [`NovelBook::new`]，但章名先查 `titles`，沒改過的檔案不再重讀。
    pub fn with_cache(root: &Path, children: &[Entry], book: Option<&Book>, titles: &mut TitleCache) -> NovelBook {
        let mut chapters = Vec::new();
        match book {
            Some(b) => chapters_of_book(b, None, &mut chapters),
            None => collect(children, None, titles, &mut chapters),
        }
        let folder = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        NovelBook {
            root: root.to_path_buf(),
            title: book.map(|b| b.meta.title.clone()).filter(|t| !t.trim().is_empty()).unwrap_or(folder),
            author: book.map(|b| b.meta.author.clone()).unwrap_or_default(),
            cover: find_cover(root),
            chapters,
        }
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.chapters.iter().position(|c| c.path == path)
    }
}

fn chapters_of_book(book: &Book, volume: Option<&str>, out: &mut Vec<NovelChapter>) {
    for ch in &book.chapters {
        // 章節放在子資料夾裡時，以子資料夾為卷名
        let sub = ch.rel.rsplit_once('/').map(|(dir, _)| dir.replace('/', " / "));
        let volume = match (volume, sub) {
            (Some(v), Some(s)) => Some(format!("{v} / {s}")),
            (Some(v), None) => Some(v.to_string()),
            (None, s) => s,
        };
        out.push(NovelChapter { path: ch.path.clone(), title: ch.title.clone(), volume });
    }
}

fn collect(entries: &[Entry], volume: Option<&str>, titles: &mut TitleCache, out: &mut Vec<NovelChapter>) {
    for e in entries {
        match e.kind {
            EntryKind::Markdown => {
                let title = match titles.get(&e.path) {
                    Some((stamp, title)) if e.modified.is_some() && *stamp == e.modified => title.clone(),
                    _ => {
                        let title = crate::book::chapter_title(&e.path);
                        titles.insert(e.path.clone(), (e.modified, title.clone()));
                        title
                    }
                };
                out.push(NovelChapter { path: e.path.clone(), title, volume: volume.map(str::to_string) });
            }
            EntryKind::Dir => {
                let name = match volume {
                    Some(v) => format!("{v} / {}", e.name),
                    None => e.name.clone(),
                };
                collect(&e.children, Some(&name), titles, out);
            }
            EntryKind::Book => {
                if let Ok(b) = Book::open(&e.path) {
                    let name = match volume {
                        Some(v) => format!("{v} / {}", b.meta.title),
                        None => b.meta.title.clone(),
                    };
                    chapters_of_book(&b, Some(&name), out);
                }
            }
            EntryKind::Pdf => {}
        }
    }
}

const COVER_NAMES: &[&str] = &["cover", "封面", "表紙", "folder", "front"];
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif"];

fn find_cover(root: &Path) -> Option<PathBuf> {
    let rd = std::fs::read_dir(root).ok()?;
    let mut found: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let ext = p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).unwrap_or_default();
            let stem = p.file_stem().and_then(|s| s.to_str()).map(|s| s.to_lowercase()).unwrap_or_default();
            IMAGE_EXTS.contains(&ext.as_str()) && COVER_NAMES.contains(&stem.as_str())
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

// ---------------------------------------------------------------------------
// 內文

/// 一段文字中的一小段（同樣式）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Span {
    pub text: String,
    /// 粗體
    pub strong: bool,
    /// 強調：以著重號（傍點）表示
    pub emph: bool,
    /// 程式碼、連結等以淡色表示
    pub weak: bool,
    /// 注音（ルビ），`text` 為被注音的字
    pub ruby: Option<String>,
}

/// 簡化後的段落。
#[derive(Debug, Clone, PartialEq)]
pub enum NBlock {
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    /// 內文段落；`\n` 表示段內換行（另起一行，同樣首行縮排）
    Para(Vec<Span>),
    /// 引言 / 提示框：整段縮排
    Quote(Vec<Span>),
    /// 程式碼、表格、數學式：照原樣、不縮排
    Pre(String),
    /// 插圖（獨立一頁）
    Image {
        url: String,
        alt: String,
    },
    /// 場景分隔（`---`、`* * *`）
    Scene,
    /// 強制換頁（HackMD 的分頁符號）
    PageBreak,
}

/// 把 Markdown 轉成書本模式的段落。章首的一級標題會被當成章名。
pub fn blocks(markdown: &str) -> Vec<NBlock> {
    let doc = hmd::parse(markdown);
    let mut out = Vec::new();
    convert(&doc.blocks, &mut out, false);
    out
}

fn convert(bs: &[Block], out: &mut Vec<NBlock>, quoted: bool) {
    for b in bs {
        match b {
            Block::Paragraph(inl) => {
                // 只有圖片的段落 → 插圖頁
                let images: Vec<_> = inl
                    .iter()
                    .filter_map(|i| match i {
                        Inline::Image { url, alt, .. } => Some((url.clone(), alt.clone())),
                        _ => None,
                    })
                    .collect();
                let only_images = !images.is_empty()
                    && inl.iter().all(|i| match i {
                        Inline::Image { .. } | Inline::SoftBreak | Inline::HardBreak => true,
                        Inline::Text(t) => t.trim().is_empty(),
                        _ => false,
                    });
                if only_images {
                    out.extend(images.into_iter().map(|(url, alt)| NBlock::Image { url, alt }));
                    continue;
                }
                let spans = spans(inl);
                if is_scene_break(&spans) {
                    out.push(NBlock::Scene);
                } else if quoted {
                    out.push(NBlock::Quote(spans));
                } else {
                    out.push(NBlock::Para(spans));
                }
            }
            Block::Heading { level, content, .. } => out.push(NBlock::Heading { level: *level, spans: spans(content) }),
            Block::Quote(inner) => convert(inner, out, true),
            Block::Container { title, blocks, .. } => {
                if !title.trim().is_empty() {
                    out.push(NBlock::Quote(vec![Span { text: title.clone(), strong: true, ..Default::default() }]));
                }
                convert(blocks, out, true);
            }
            Block::Code { code, .. } => out.push(NBlock::Pre(code.trim_end().to_string())),
            Block::Math(tex) => out.push(NBlock::Pre(tex.trim().to_string())),
            Block::List { start, items } => {
                for (i, item) in items.iter().enumerate() {
                    let marker = match start {
                        Some(s) => format!("{}. ", s + i as u64),
                        None => "・".to_string(),
                    };
                    let mut inner = Vec::new();
                    convert(item, &mut inner, quoted);
                    let mut first = true;
                    for b in inner {
                        match b {
                            NBlock::Para(mut s) | NBlock::Quote(mut s) if first => {
                                s.insert(0, Span { text: marker.clone(), ..Default::default() });
                                out.push(NBlock::Quote(s));
                            }
                            NBlock::Para(s) => out.push(NBlock::Quote(s)),
                            other => out.push(other),
                        }
                        first = false;
                    }
                }
            }
            Block::Table { header, rows, .. } => {
                let row =
                    |cells: &[Vec<Inline>]| cells.iter().map(|c| hmd::plain_text(c)).collect::<Vec<_>>().join("｜");
                let mut text = row(header);
                for r in rows {
                    text.push('\n');
                    text.push_str(&row(r));
                }
                out.push(NBlock::Pre(text));
            }
            Block::DefList(items) => {
                for it in items {
                    out.push(NBlock::Para(vec![Span {
                        text: hmd::plain_text(&it.term),
                        strong: true,
                        ..Default::default()
                    }]));
                    convert(&it.defs.concat(), out, true);
                }
            }
            Block::Rule => out.push(NBlock::Scene),
            Block::PageBreak => out.push(NBlock::PageBreak),
            Block::Toc | Block::Html(_) => {}
        }
    }
}

/// 只由分隔符號組成的段落（`◇◇◇`、`＊＊＊`、`* * *`）視為場景分隔。
fn is_scene_break(spans: &[Span]) -> bool {
    let text: String = spans.iter().map(|s| s.text.as_str()).collect();
    let t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    !t.is_empty() && t.chars().count() <= 9 && t.chars().all(|c| "◇◆*＊※☆★・◎○●#＃".contains(c))
}

#[derive(Clone, Copy, Default)]
struct St {
    strong: bool,
    emph: bool,
    weak: bool,
}

fn spans(inl: &[Inline]) -> Vec<Span> {
    let mut out = Vec::new();
    walk(inl, St::default(), &mut out);
    // 合併相鄰同樣式的片段
    let mut merged: Vec<Span> = Vec::new();
    for s in out {
        if s.text.is_empty() {
            continue;
        }
        match merged.last_mut() {
            Some(last)
                if last.ruby.is_none()
                    && s.ruby.is_none()
                    && (last.strong, last.emph, last.weak) == (s.strong, s.emph, s.weak) =>
            {
                last.text.push_str(&s.text)
            }
            _ => merged.push(s),
        }
    }
    // 去掉段尾換行
    if let Some(last) = merged.last_mut() {
        let t = last.text.trim_end_matches('\n').len();
        last.text.truncate(t);
    }
    merged
}

fn walk(inl: &[Inline], st: St, out: &mut Vec<Span>) {
    for i in inl {
        match i {
            Inline::Text(t) => push_ruby_text(t, st, out),
            Inline::Code(t) => out.push(Span { text: t.clone(), weak: true, ..span(st) }),
            Inline::Math { tex, .. } => out.push(Span { text: tex.clone(), weak: true, ..span(st) }),
            Inline::Emph(c) | Inline::Mark(c) | Inline::Ins(c) => walk(c, St { emph: true, ..st }, out),
            Inline::Strong(c) => walk(c, St { strong: true, ..st }, out),
            Inline::Strike(c) | Inline::Sup(c) | Inline::Sub(c) => walk(c, st, out),
            Inline::Link { children, .. } => walk(children, St { weak: true, ..st }, out),
            Inline::Image { alt, .. } => {
                if !alt.is_empty() {
                    out.push(Span { text: format!("［{alt}］"), weak: true, ..span(st) })
                }
            }
            Inline::FootnoteRef { number, .. } => out.push(Span { text: format!("*{number}"), weak: true, ..span(st) }),
            Inline::Task { checked, .. } => {
                out.push(Span { text: if *checked { "☑ " } else { "☐ " }.into(), ..span(st) })
            }
            Inline::SoftBreak | Inline::HardBreak => out.push(Span { text: "\n".into(), ..span(st) }),
            Inline::Html(_) => {}
        }
    }
}

fn span(st: St) -> Span {
    Span { text: String::new(), strong: st.strong, emph: st.emph, weak: st.weak, ruby: None }
}

/// 解析注音：`｜漢字《かんじ》` 或 `|漢字《かんじ》`。
///
/// 只認有直線開頭的寫法：中文的書名號《》很常見，不能把「讀了《某書》」誤判成注音。
fn push_ruby_text(t: &str, st: St, out: &mut Vec<Span>) {
    let mut rest = t;
    while let Some(start) = rest.find(['｜', '|']) {
        let bar_len = rest[start..].chars().next().map(char::len_utf8).unwrap_or(1);
        let after = &rest[start + bar_len..];
        match parse_ruby(after) {
            Some((base, ruby, end)) => {
                out.push(Span { text: rest[..start].to_string(), ..span(st) });
                out.push(Span { text: base.to_string(), ruby: Some(ruby.to_string()), ..span(st) });
                rest = &after[end..];
            }
            None => {
                out.push(Span { text: rest[..start + bar_len].to_string(), ..span(st) });
                rest = after;
            }
        }
    }
    out.push(Span { text: rest.to_string(), ..span(st) });
}

/// 直線之後的 `漢字《かんじ》`：回傳（被注音的字、讀音、整段的長度）。
fn parse_ruby(after: &str) -> Option<(&str, &str, usize)> {
    let open = after.find('《')?;
    let base = &after[..open];
    let close = after[open..].find('》')? + open;
    let ruby = &after[open + '《'.len_utf8()..close];
    let ok = !base.is_empty() && !ruby.is_empty() && base.chars().count() <= 20 && !base.contains(['\n', '|', '｜']);
    ok.then(|| (base, ruby, close + '》'.len_utf8()))
}

/// 去掉注音的記號與讀音，只留被注音的字：`｜漢字《かんじ》` → `漢字`。
fn strip_ruby(t: &str) -> Cow<'_, str> {
    if !t.contains('《') {
        return Cow::Borrowed(t);
    }
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(start) = rest.find(['｜', '|']) {
        let bar_len = rest[start..].chars().next().map(char::len_utf8).unwrap_or(1);
        let after = &rest[start + bar_len..];
        match parse_ruby(after) {
            Some((base, _, end)) => {
                out.push_str(&rest[..start]);
                out.push_str(base);
                rest = &after[end..];
            }
            None => {
                out.push_str(&rest[..start + bar_len]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// 書本模式的字數：算法和編輯器狀態列的字數相同（中日韓文字每字算一、西文每個單字算一），
/// 但注音（ルビ）的讀音不算，`｜漢字《かんじ》` 只算「漢字」兩個字。
pub fn word_count(markdown: &str) -> usize {
    crate::markdown::word_count_filtered(markdown, strip_ruby)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(spans: &[Span]) -> Vec<(&str, Option<&str>)> {
        spans.iter().map(|s| (s.text.as_str(), s.ruby.as_deref())).collect()
    }

    #[test]
    fn ruby_syntax() {
        let b = blocks("她看著｜魔導書《グリモワール》說：「讀了《紅樓夢》嗎？」");
        let NBlock::Para(s) = &b[0] else { panic!("{b:?}") };
        assert_eq!(
            texts(s),
            vec![("她看著", None), ("魔導書", Some("グリモワール")), ("說：「讀了《紅樓夢》嗎？」", None)]
        );
    }

    #[test]
    fn structure() {
        let md = "# 第一章　出發\n\n　　清晨。\n她醒了。\n\n◇◇◇\n\n*重要*的事\n\n![插圖](a.png)\n\n> 引言\n";
        let b = blocks(md);
        assert!(matches!(&b[0], NBlock::Heading { level: 1, .. }));
        let NBlock::Para(p) = &b[1] else { panic!() };
        assert_eq!(p[0].text, "　　清晨。\n她醒了。");
        assert_eq!(b[2], NBlock::Scene);
        let NBlock::Para(p) = &b[3] else { panic!() };
        assert!(p[0].emph && p[0].text == "重要");
        assert_eq!(b[4], NBlock::Image { url: "a.png".into(), alt: "插圖".into() });
        assert!(matches!(&b[5], NBlock::Quote(_)));
    }

    #[test]
    fn folder_book() {
        let dir = std::env::temp_dir().join(format!("shosai-novel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("第一卷")).unwrap();
        std::fs::write(dir.join("序章.md"), "# 序章\n").unwrap();
        std::fs::write(dir.join("第一卷/01.md"), "# 相遇\n").unwrap();
        std::fs::write(dir.join("封面.png"), b"x").unwrap();
        let lib = crate::Library::open(&dir).unwrap();
        let nb = NovelBook::new(&dir, &lib.entries, None);
        assert_eq!(nb.chapters.len(), 2);
        assert_eq!(nb.chapters[0].volume.as_deref(), Some("第一卷"));
        assert_eq!(nb.chapters[0].title, "相遇");
        assert_eq!(nb.chapters[1].title, "序章");
        assert!(nb.cover.is_some());

        // 章名快取：沒改過的檔案不重讀，改過的（修改時間不同）才重讀
        let mut titles = TitleCache::new();
        assert_eq!(NovelBook::with_cache(&dir, &lib.entries, None, &mut titles), nb);
        assert_eq!(titles.len(), 2);
        for (_, title) in titles.values_mut() {
            title.push_str("（快取）");
        }
        let cached = NovelBook::with_cache(&dir, &lib.entries, None, &mut titles);
        assert_eq!(cached.chapters[0].title, "相遇（快取）");
        let mut entries = lib.entries.clone();
        fn touch(list: &mut [Entry]) {
            for e in list {
                e.modified = e.modified.map(|t| t + std::time::Duration::from_secs(5));
                touch(&mut e.children);
            }
        }
        touch(&mut entries);
        assert_eq!(NovelBook::with_cache(&dir, &entries, None, &mut titles), nb);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn word_count_ignores_ruby_readings() {
        assert_eq!(word_count("她看著｜魔導書《グリモワール》說"), 7);
        // 書名號不是注音，照算
        assert_eq!(word_count("讀了《紅樓夢》嗎"), 6);
        // 沒有注音時和一般的字數統計一樣
        let md = "# 第一章\n\n　　清晨，她醒了。Hello world.\n\n- 清單\n";
        assert_eq!(word_count(md), crate::markdown::word_count(md));
    }
}
