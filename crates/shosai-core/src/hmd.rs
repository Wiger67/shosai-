//! HackMD 相容的 Markdown 解析器。
//!
//! 流程：前處理（HackMD 專屬的區塊語法 → 標記）→ pulldown-cmark → 語法樹 [`MdDoc`]。
//! GUI 預覽、PDF（Typst）與 HTML 輸出都使用同一份語法樹，確保三者顯示一致。
//!
//! 支援的 HackMD 語法：
//! - 數學式 `$...$`、`$$...$$`、` ```math `（LaTeX）
//! - `==螢光標記==`、`++插入（底線）++`、`^上標^`、`~下標~`、`~~刪除線~~`
//! - 提示框 `:::info` / `:::success` / `:::warning` / `:::danger` / `:::spoiler 標題`
//! - `[TOC]` 目錄、`:smile:` 表情符號、圖片尺寸 `![](a.png =200x)`
//! - 程式碼行號 ` ```js= `、` ```js=101 `、` ```js=+ `
//! - 定義清單、註腳、待辦清單、表格、YAML front matter（`breaks: false` 等）
//! - 單一換行即換行（HackMD 預設 `breaks: true`）
//! - `{%youtube id %}` 等嵌入語法轉成連結

use std::collections::HashMap;

use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::markdown::level_num;

/// HackMD 相容的解析選項。
pub fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_GFM
        | Options::ENABLE_MATH
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_SUPERSCRIPT
        | Options::ENABLE_SUBSCRIPT
}

// ---------------------------------------------------------------------------
// 語法樹

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MdDoc {
    pub meta: FrontMatter,
    pub blocks: Vec<Block>,
    /// 依引用順序編號的註腳
    pub footnotes: Vec<Footnote>,
    /// `blocks` 裡每個區塊在原始文字中的位置（位元組），與 `blocks` 一一對應、由小到大。
    /// 預覽與編輯器的捲動同步靠它對位。
    pub offsets: Vec<usize>,
    /// 每個標題（以錨點 id 查）在原始文字中的位置（位元組）
    pub heading_offsets: HashMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrontMatter {
    pub title: Option<String>,
    pub tags: Vec<String>,
    /// 單一換行是否視為換行（HackMD 預設 true）
    pub breaks: bool,
    pub lang: Option<String>,
}

impl Default for FrontMatter {
    fn default() -> Self {
        Self { title: None, tags: Vec::new(), breaks: true, lang: None }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Footnote {
    pub label: String,
    pub number: usize,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    Info,
    Success,
    Warning,
    Danger,
    Spoiler,
}

impl ContainerKind {
    fn parse(s: &str) -> Option<ContainerKind> {
        Some(match s.to_ascii_lowercase().as_str() {
            "info" | "note" | "important" => ContainerKind::Info,
            "success" | "tip" => ContainerKind::Success,
            "warning" => ContainerKind::Warning,
            "danger" | "caution" => ContainerKind::Danger,
            "spoiler" => ContainerKind::Spoiler,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            ContainerKind::Info => "info",
            ContainerKind::Success => "success",
            ContainerKind::Warning => "warning",
            ContainerKind::Danger => "danger",
            ContainerKind::Spoiler => "spoiler",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    Heading {
        level: u8,
        content: Vec<Inline>,
        id: String,
    },
    Quote(Vec<Block>),
    Code {
        lang: String,
        code: String,
        line_start: Option<usize>,
    },
    /// 獨立成段的數學式（LaTeX）
    Math(String),
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Table {
        aligns: Vec<Align>,
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    Rule,
    Container {
        kind: ContainerKind,
        title: String,
        open: bool,
        blocks: Vec<Block>,
    },
    DefList(Vec<DefItem>),
    /// `[TOC]`
    Toc,
    PageBreak,
    Html(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DefItem {
    pub term: Vec<Inline>,
    pub defs: Vec<Vec<Block>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    Code(String),
    Math {
        tex: String,
        display: bool,
    },
    Emph(Vec<Inline>),
    Strong(Vec<Inline>),
    Strike(Vec<Inline>),
    /// `==螢光==`
    Mark(Vec<Inline>),
    /// `++插入++`
    Ins(Vec<Inline>),
    Sup(Vec<Inline>),
    Sub(Vec<Inline>),
    Link {
        url: String,
        title: String,
        children: Vec<Inline>,
    },
    Image {
        url: String,
        alt: String,
        title: String,
        width: Option<u32>,
        height: Option<u32>,
    },
    FootnoteRef {
        label: String,
        number: usize,
    },
    /// 待辦清單核取方塊；`offset` 是 `[ ]` 在原始文字中的位置（可用來切換勾選）
    Task {
        checked: bool,
        offset: usize,
    },
    SoftBreak,
    HardBreak,
    Html(String),
}

/// 文件中的標題（給目錄 `[TOC]` 使用）。
#[derive(Debug, Clone, PartialEq)]
pub struct TocEntry {
    pub level: u8,
    pub text: String,
    pub id: String,
    /// 在原始文字中的位置（位元組）
    pub offset: usize,
}

impl MdDoc {
    /// 所有標題（含提示框內的），依出現順序。
    pub fn headings(&self) -> Vec<TocEntry> {
        fn walk(blocks: &[Block], offsets: &HashMap<String, usize>, out: &mut Vec<TocEntry>) {
            for b in blocks {
                match b {
                    Block::Heading { level, content, id } => out.push(TocEntry {
                        level: *level,
                        text: plain_text(content),
                        id: id.clone(),
                        offset: offsets.get(id).copied().unwrap_or(0),
                    }),
                    Block::Quote(bs) | Block::Container { blocks: bs, .. } => walk(bs, offsets, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.blocks, &self.heading_offsets, &mut out);
        out
    }

    /// 第一個一級標題，否則 front matter 的 title。
    pub fn title(&self) -> Option<String> {
        self.headings().into_iter().find(|h| h.level == 1).map(|h| h.text).or_else(|| self.meta.title.clone())
    }

    pub fn starts_with_h1(&self) -> bool {
        matches!(self.blocks.first(), Some(Block::Heading { level: 1, .. }))
    }

    pub fn footnote(&self, number: usize) -> Option<&Footnote> {
        self.footnotes.iter().find(|f| f.number == number)
    }
}

/// 取出純文字（數學式保留 LaTeX 原文）。
pub fn plain_text(inlines: &[Inline]) -> String {
    let mut s = String::new();
    fn walk(inlines: &[Inline], s: &mut String) {
        for i in inlines {
            match i {
                Inline::Text(t) | Inline::Code(t) => s.push_str(t),
                Inline::Math { tex, .. } => s.push_str(tex),
                Inline::Emph(c)
                | Inline::Strong(c)
                | Inline::Strike(c)
                | Inline::Mark(c)
                | Inline::Ins(c)
                | Inline::Sup(c)
                | Inline::Sub(c)
                | Inline::Link { children: c, .. } => walk(c, s),
                Inline::Image { alt, .. } => s.push_str(alt),
                Inline::SoftBreak | Inline::HardBreak => s.push(' '),
                _ => {}
            }
        }
    }
    walk(inlines, &mut s);
    s
}

/// HackMD 風格的標題錨點：去掉標點、空白換成 `-`，保留中文。
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    for c in text.trim().chars() {
        if c.is_whitespace() {
            out.push('-');
        } else if c == '-' || c == '_' || c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 前處理

/// 前處理後文字位置 → 原始文字位置。
#[derive(Debug, Clone, Default)]
struct SourceMap {
    /// (前處理後位置, 原始位置)，每次替換結束後記一個錨點
    anchors: Vec<(usize, usize)>,
}

impl SourceMap {
    fn map(&self, pos: usize) -> usize {
        let i = self.anchors.partition_point(|(n, _)| *n <= pos);
        match i {
            0 => pos,
            _ => {
                let (n, o) = self.anchors[i - 1];
                o + (pos - n)
            }
        }
    }
}

struct Pre {
    out: String,
    map: SourceMap,
    /// 已處理到的原始位置
    orig: usize,
}

impl Pre {
    /// 複製原始文字到 `to`。
    fn copy(&mut self, src: &str, to: usize) {
        self.out.push_str(&src[self.orig..to]);
        self.orig = to;
    }
    /// 以 `replacement` 取代原始文字 `[self.orig, to)`。
    fn replace(&mut self, to: usize, replacement: &str) {
        self.out.push_str(replacement);
        self.orig = to;
        self.map.anchors.push((self.out.len(), self.orig));
    }
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> String {
    let bytes: Vec<u8> =
        (0..s.len() / 2).filter_map(|i| u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

const MARK_OPEN: &str = "<!--shosai:open:";
const MARK_END: &str = "<!--shosai:end-->";
const MARK_TOC: &str = "<!--shosai:toc-->";

fn preprocess(src: &str) -> (String, SourceMap) {
    let mut pre = Pre { out: String::with_capacity(src.len() + 64), map: SourceMap::default(), orig: 0 };
    let mut fence: Option<(char, usize)> = None;
    let mut in_math = false;
    let mut pos = 0;

    for line in src.split_inclusive('\n') {
        let start = pos;
        pos += line.len();
        let content = line.trim_end_matches(['\n', '\r']);
        let indent = content.len() - content.trim_start_matches(' ').len();
        let trimmed = content.trim();

        // 程式碼區塊內不做任何處理
        if let Some((ch, len)) = fence {
            let n = trimmed.chars().take_while(|c| *c == ch).count();
            if n >= len && trimmed[n..].trim().is_empty() && indent < 4 {
                fence = None;
            }
            continue;
        }
        if indent < 4 {
            let ch = trimmed.chars().next().unwrap_or(' ');
            if ch == '`' || ch == '~' {
                let n = trimmed.chars().take_while(|c| *c == ch).count();
                if n >= 3 {
                    fence = Some((ch, n));
                    continue;
                }
            }
        }
        if trimmed == "$$" {
            in_math = !in_math;
            continue;
        }
        if in_math || indent >= 4 {
            continue;
        }

        let line_end = start + content.len();
        // :::info 標題 / :::
        if let Some(rest) = trimmed.strip_prefix(":::") {
            let rest = rest.trim();
            pre.copy(src, start);
            if rest.is_empty() {
                pre.replace(line_end, MARK_END);
                continue;
            }
            let (kind, title) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            if let Some(kind) = ContainerKind::parse(kind) {
                let mut title = title.trim().to_string();
                let open = title.contains("{state=\"open\"}") || title.contains("{state='open'}");
                if let Some(i) = title.find("{state=") {
                    title.truncate(i);
                }
                let marker = format!("{MARK_OPEN}{}:{}:{}-->", kind.name(), u8::from(open), hex(title.trim()));
                pre.replace(line_end, &marker);
                continue;
            }
        }
        if trimmed.eq_ignore_ascii_case("[toc]") {
            pre.copy(src, start);
            pre.replace(line_end, MARK_TOC);
            continue;
        }
        inline_preprocess(src, start, content, &mut pre);
    }
    pre.copy(src, src.len());
    (pre.out, pre.map)
}

/// 行內前處理：`{%youtube id %}` 嵌入與圖片尺寸 `=200x`。
fn inline_preprocess(src: &str, start: usize, line: &str, pre: &mut Pre) {
    let mut i = 0;
    let bytes = line.as_bytes();
    let mut in_code = false;
    while i < line.len() {
        let c = bytes[i];
        if c == b'`' {
            in_code = !in_code;
            i += 1;
            continue;
        }
        if in_code {
            i += 1;
            continue;
        }
        // {%youtube ID %}
        if line[i..].starts_with("{%") {
            if let Some(end) = line[i..].find("%}") {
                let inner = line[i + 2..i + end].trim();
                let (kind, arg) = inner.split_once(char::is_whitespace).unwrap_or((inner, ""));
                let arg = arg.trim();
                let link = match kind {
                    "youtube" => Some(format!("[▶ YouTube 影片](https://www.youtube.com/watch?v={arg})")),
                    "vimeo" => Some(format!("[▶ Vimeo 影片](https://vimeo.com/{arg})")),
                    "gist" => Some(format!("[📄 Gist：{arg}](https://gist.github.com/{arg})")),
                    "slideshare" => Some(format!("[📊 SlideShare](https://www.slideshare.net/{arg})")),
                    "speakerdeck" => Some(format!("[📊 Speaker Deck](https://speakerdeck.com/{arg})")),
                    "pdf" => Some(format!("[📄 PDF]({arg})")),
                    "figma" => Some(format!("[🎨 Figma]({arg})")),
                    _ => None,
                };
                if let (Some(link), false) = (link, arg.is_empty()) {
                    pre.copy(src, start + i);
                    pre.replace(start + i + end + 2, &link);
                    i += end + 2;
                    continue;
                }
            }
        }
        // ](url =200x) → ](url "=200x")
        if line[i..].starts_with("](") {
            if let Some(close) = line[i + 2..].find(')') {
                let inner = &line[i + 2..i + 2 + close];
                if let Some(eq) = inner.rfind(" =") {
                    let size = &inner[eq + 2..];
                    let is_size = !size.is_empty()
                        && size.contains('x')
                        && size.chars().all(|c| c.is_ascii_digit() || c == 'x' || c == '%');
                    if is_size {
                        let head = inner[..eq].trim_end();
                        let new = match head.strip_suffix('"').and_then(|h| h.rsplit_once(" \"")) {
                            Some((url, title)) => format!("]({url} \"{title} ={size}\")"),
                            None => format!("]({head} \"={size}\")"),
                        };
                        pre.copy(src, start + i);
                        pre.replace(start + i + 2 + close + 1, &new);
                        i += 2 + close + 1;
                        continue;
                    }
                }
            }
        }
        i += line[i..].chars().next().map(char::len_utf8).unwrap_or(1);
    }
}

// ---------------------------------------------------------------------------
// 解析

type Ev<'a> = (Event<'a>, std::ops::Range<usize>);

struct P<'a> {
    events: std::iter::Peekable<std::vec::IntoIter<Ev<'a>>>,
    map: SourceMap,
    meta: FrontMatter,
    footnote_defs: HashMap<String, Vec<Block>>,
    footnote_numbers: HashMap<String, usize>,
    slugs: HashMap<String, usize>,
    /// 目前在第幾層的 `blocks`（1 = 文件最上層）
    depth: usize,
    /// 最上層每個區塊的原始位置
    top_offsets: Vec<usize>,
    /// 正在解析的區塊從哪裡開始（前處理後的位置）
    block_start: usize,
    heading_offsets: HashMap<String, usize>,
}

/// 解析 HackMD 風格的 Markdown。
pub fn parse(src: &str) -> MdDoc {
    let (text, map) = preprocess(src);
    let events: Vec<Ev> = Parser::new_ext(&text, options()).into_offset_iter().collect();
    let mut p = P {
        events: events.into_iter().peekable(),
        map,
        meta: FrontMatter::default(),
        footnote_defs: HashMap::new(),
        footnote_numbers: HashMap::new(),
        slugs: HashMap::new(),
        depth: 0,
        top_offsets: Vec::new(),
        block_start: 0,
        heading_offsets: HashMap::new(),
    };
    let blocks = p.blocks(None);
    debug_assert_eq!(blocks.len(), p.top_offsets.len());
    let offsets = std::mem::take(&mut p.top_offsets);
    let mut footnotes: Vec<Footnote> = p
        .footnote_numbers
        .iter()
        .map(|(label, &number)| Footnote {
            label: label.clone(),
            number,
            blocks: p.footnote_defs.remove(label).unwrap_or_default(),
        })
        .collect();
    footnotes.sort_by_key(|f| f.number);
    let (offsets, blocks) = fold_with_offsets(offsets.into_iter().zip(blocks).collect()).into_iter().unzip();
    MdDoc { meta: p.meta, blocks, footnotes, offsets, heading_offsets: p.heading_offsets }
}

impl<'a> P<'a> {
    /// 讀取區塊直到遇到 `end`（會吃掉 end 事件）。
    fn blocks(&mut self, end: Option<fn(&TagEnd) -> bool>) -> Vec<Block> {
        self.depth += 1;
        let top = self.depth == 1;
        let mut out = Vec::new();
        let mut loose: Vec<Inline> = Vec::new(); // 緊湊清單中直接出現的行內內容
        let mut loose_start = 0;
        while let Some((ev, range)) = self.events.next() {
            match ev {
                Event::End(ref t) if end.is_some_and(|f| f(t)) => break,
                Event::Start(tag) if is_block_tag(&tag) => {
                    if !loose.is_empty() {
                        out.push(Block::Paragraph(self.finish_inlines(std::mem::take(&mut loose))));
                        if top {
                            self.top_offsets.push(self.map.map(loose_start));
                        }
                    }
                    self.block_start = range.start;
                    if let Some(b) = self.block(tag) {
                        out.push(b);
                        if top {
                            self.top_offsets.push(self.map.map(range.start));
                        }
                    }
                }
                Event::Rule => {
                    out.push(Block::Rule);
                    if top {
                        self.top_offsets.push(self.map.map(range.start));
                    }
                }
                other => {
                    let start = range.start;
                    if let Some(i) = self.inline(other, range) {
                        if loose.is_empty() {
                            loose_start = start;
                        }
                        loose.push(i);
                    }
                }
            }
        }
        if !loose.is_empty() {
            out.push(Block::Paragraph(self.finish_inlines(loose)));
            if top {
                self.top_offsets.push(self.map.map(loose_start));
            }
        }
        self.depth -= 1;
        out
    }

    fn block(&mut self, tag: Tag<'a>) -> Option<Block> {
        let start = self.block_start;
        Some(match tag {
            Tag::Paragraph => {
                let inl = self.inlines(|t| matches!(t, TagEnd::Paragraph));
                // 整段只有一個 $$...$$ → 獨立數學式
                let meaningful: Vec<&Inline> = inl
                    .iter()
                    .filter(|i| !matches!(i, Inline::SoftBreak | Inline::HardBreak))
                    .filter(|i| !matches!(i, Inline::Text(t) if t.trim().is_empty()))
                    .collect();
                if let [Inline::Math { tex, display: true }] = meaningful.as_slice() {
                    return Some(Block::Math(tex.clone()));
                }
                Block::Paragraph(inl)
            }
            Tag::Heading { level, .. } => {
                let content = self.inlines(|t| matches!(t, TagEnd::Heading(_)));
                let base = slugify(&plain_text(&content));
                let n = self.slugs.entry(base.clone()).or_insert(0);
                let id = if *n == 0 { base.clone() } else { format!("{base}-{n}") };
                *n += 1;
                self.heading_offsets.insert(id.clone(), self.map.map(start));
                Block::Heading { level: level_num(level), content, id }
            }
            Tag::BlockQuote(kind) => {
                let blocks = fold_containers(self.blocks(Some(|t| matches!(t, TagEnd::BlockQuote(_)))));
                match kind {
                    // GitHub 風格提示 > [!NOTE] → 提示框
                    Some(k) => {
                        let kind = match k {
                            BlockQuoteKind::Note | BlockQuoteKind::Important => ContainerKind::Info,
                            BlockQuoteKind::Tip => ContainerKind::Success,
                            BlockQuoteKind::Warning => ContainerKind::Warning,
                            BlockQuoteKind::Caution => ContainerKind::Danger,
                        };
                        Block::Container { kind, title: String::new(), open: true, blocks }
                    }
                    None => Block::Quote(blocks),
                }
            }
            Tag::CodeBlock(kind) => {
                let mut code = String::new();
                for (ev, _) in self.events.by_ref() {
                    match ev {
                        Event::Text(t) => code.push_str(&t),
                        Event::End(TagEnd::CodeBlock) => break,
                        _ => {}
                    }
                }
                if code.ends_with('\n') {
                    code.pop();
                }
                let info = match kind {
                    CodeBlockKind::Fenced(info) => info.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                let (lang, line_start) = parse_code_info(&info);
                if matches!(lang.as_str(), "math" | "latex-math" | "katex") {
                    return Some(Block::Math(code));
                }
                Block::Code { lang, code, line_start }
            }
            Tag::HtmlBlock => {
                let mut html = String::new();
                for (ev, _) in self.events.by_ref() {
                    match ev {
                        Event::Html(t) | Event::Text(t) => html.push_str(&t),
                        Event::End(TagEnd::HtmlBlock) => break,
                        _ => {}
                    }
                }
                let t = html.trim();
                let lower = t.to_ascii_lowercase();
                if t == MARK_TOC {
                    Block::Toc
                } else if lower.contains("pagebreak")
                    || lower.contains("page-break")
                    || lower.contains("break-after: page")
                {
                    Block::PageBreak
                } else {
                    Block::Html(html)
                }
            }
            Tag::List(start) => {
                let mut items = Vec::new();
                while let Some((ev, _)) = self.events.next() {
                    match ev {
                        Event::Start(Tag::Item) => {
                            items.push(fold_containers(self.blocks(Some(|t| matches!(t, TagEnd::Item)))))
                        }
                        Event::End(TagEnd::List(_)) => break,
                        _ => {}
                    }
                }
                Block::List { start, items }
            }
            Tag::Table(aligns) => {
                let aligns = aligns
                    .iter()
                    .map(|a| match a {
                        Alignment::None => Align::None,
                        Alignment::Left => Align::Left,
                        Alignment::Center => Align::Center,
                        Alignment::Right => Align::Right,
                    })
                    .collect();
                let mut header = Vec::new();
                let mut rows: Vec<Vec<Vec<Inline>>> = Vec::new();
                let mut in_head = false;
                while let Some((ev, _)) = self.events.next() {
                    match ev {
                        Event::Start(Tag::TableHead) => in_head = true,
                        Event::End(TagEnd::TableHead) => in_head = false,
                        Event::Start(Tag::TableRow) => rows.push(Vec::new()),
                        Event::Start(Tag::TableCell) => {
                            let cell = self.inlines(|t| matches!(t, TagEnd::TableCell));
                            if in_head {
                                header.push(cell);
                            } else if let Some(r) = rows.last_mut() {
                                r.push(cell);
                            }
                        }
                        Event::End(TagEnd::Table) => break,
                        _ => {}
                    }
                }
                Block::Table { aligns, header, rows }
            }
            Tag::FootnoteDefinition(label) => {
                let blocks = self.blocks(Some(|t| matches!(t, TagEnd::FootnoteDefinition)));
                self.footnote_defs.insert(label.to_string(), blocks);
                return None;
            }
            Tag::MetadataBlock(_) => {
                let mut text = String::new();
                for (ev, _) in self.events.by_ref() {
                    match ev {
                        Event::Text(t) => text.push_str(&t),
                        Event::End(TagEnd::MetadataBlock(_)) => break,
                        _ => {}
                    }
                }
                self.meta = parse_front_matter(&text);
                return None;
            }
            Tag::DefinitionList => {
                let mut items: Vec<DefItem> = Vec::new();
                while let Some((ev, _)) = self.events.next() {
                    match ev {
                        Event::Start(Tag::DefinitionListTitle) => {
                            let term = self.inlines(|t| matches!(t, TagEnd::DefinitionListTitle));
                            items.push(DefItem { term, defs: Vec::new() });
                        }
                        Event::Start(Tag::DefinitionListDefinition) => {
                            let def = self.blocks(Some(|t| matches!(t, TagEnd::DefinitionListDefinition)));
                            match items.last_mut() {
                                Some(item) => item.defs.push(def),
                                None => items.push(DefItem { term: Vec::new(), defs: vec![def] }),
                            }
                        }
                        Event::End(TagEnd::DefinitionList) => break,
                        _ => {}
                    }
                }
                Block::DefList(items)
            }
            _ => return None,
        })
    }

    /// 讀取行內內容直到 `end`。
    fn inlines(&mut self, end: fn(&TagEnd) -> bool) -> Vec<Inline> {
        let mut out = Vec::new();
        while let Some((ev, range)) = self.events.next() {
            match ev {
                Event::End(ref t) if end(t) => break,
                other => {
                    if let Some(i) = self.inline(other, range) {
                        out.push(i);
                    }
                }
            }
        }
        self.finish_inlines(out)
    }

    fn inline(&mut self, ev: Event<'a>, range: std::ops::Range<usize>) -> Option<Inline> {
        Some(match ev {
            Event::Text(t) => Inline::Text(t.into_string()),
            Event::Code(t) => Inline::Code(t.into_string()),
            Event::InlineMath(t) => Inline::Math { tex: t.into_string(), display: false },
            Event::DisplayMath(t) => Inline::Math { tex: t.into_string(), display: true },
            Event::SoftBreak => Inline::SoftBreak,
            Event::HardBreak => Inline::HardBreak,
            Event::Html(h) | Event::InlineHtml(h) => {
                let lower = h.trim().to_ascii_lowercase();
                if lower.starts_with("<br") {
                    Inline::HardBreak
                } else {
                    Inline::Html(h.into_string())
                }
            }
            Event::FootnoteReference(label) => {
                let next = self.footnote_numbers.len() + 1;
                let number = *self.footnote_numbers.entry(label.to_string()).or_insert(next);
                Inline::FootnoteRef { label: label.into_string(), number }
            }
            Event::TaskListMarker(checked) => Inline::Task { checked, offset: self.map.map(range.start) },
            Event::Start(tag) => {
                let wrap = |p: &mut Self, f: fn(Vec<Inline>) -> Inline, end: fn(&TagEnd) -> bool| f(p.inlines(end));
                match tag {
                    Tag::Emphasis => wrap(self, Inline::Emph, |t| matches!(t, TagEnd::Emphasis)),
                    Tag::Strong => wrap(self, Inline::Strong, |t| matches!(t, TagEnd::Strong)),
                    Tag::Strikethrough => wrap(self, Inline::Strike, |t| matches!(t, TagEnd::Strikethrough)),
                    Tag::Superscript => wrap(self, Inline::Sup, |t| matches!(t, TagEnd::Superscript)),
                    Tag::Subscript => wrap(self, Inline::Sub, |t| matches!(t, TagEnd::Subscript)),
                    Tag::Link { dest_url, title, .. } => {
                        let children = self.inlines(|t| matches!(t, TagEnd::Link));
                        Inline::Link { url: dest_url.into_string(), title: title.into_string(), children }
                    }
                    Tag::Image { dest_url, title, .. } => {
                        let alt = plain_text(&self.inlines(|t| matches!(t, TagEnd::Image)));
                        let (title, width, height) = split_size(&title);
                        Inline::Image { url: dest_url.into_string(), alt, title, width, height }
                    }
                    _ => return None,
                }
            }
            _ => return None,
        })
    }

    /// 行內後處理：合併文字、==/++ 配對、表情符號、HackMD 換行。
    fn finish_inlines(&self, inlines: Vec<Inline>) -> Vec<Inline> {
        let merged = merge_text(inlines);
        let paired = pair_markers(merged);
        let mut out = Vec::with_capacity(paired.len());
        for i in paired {
            match i {
                Inline::Text(t) => split_sup_sub(&replace_emoji(&t), &mut out),
                Inline::SoftBreak if self.meta.breaks => out.push(Inline::HardBreak),
                other => out.push(other),
            }
        }
        out
    }
}

fn is_block_tag(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::CodeBlock(_)
            | Tag::HtmlBlock
            | Tag::List(_)
            | Tag::Table(_)
            | Tag::FootnoteDefinition(_)
            | Tag::MetadataBlock(_)
            | Tag::DefinitionList
    )
}

/// ` ```js=101 ` → ("js", Some(101))；` ```js= ` → ("js", Some(1))
fn parse_code_info(info: &str) -> (String, Option<usize>) {
    let info = info.trim();
    let end =
        info.find(|c: char| c.is_whitespace() || c == '=' || c == '!' || c == '{' || c == ',').unwrap_or(info.len());
    let lang = info[..end].to_string();
    let rest = &info[end..];
    let line_start = rest.strip_prefix('=').map(|n| {
        let n = n.trim_start_matches('+');
        n.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(1)
    });
    (lang, line_start)
}

/// 圖片標題中的尺寸：`"說明 =200x100"` → ("說明", 200, 100)
fn split_size(title: &str) -> (String, Option<u32>, Option<u32>) {
    let t = title.trim();
    let (head, size) = match t.rfind('=') {
        Some(i)
            if t[i + 1..].contains('x') && t[i + 1..].chars().all(|c| c.is_ascii_digit() || c == 'x' || c == '%') =>
        {
            (t[..i].trim(), &t[i + 1..])
        }
        _ => return (title.to_string(), None, None),
    };
    let (w, h) = size.split_once('x').unwrap_or((size, ""));
    (head.to_string(), w.trim_end_matches('%').parse().ok(), h.trim_end_matches('%').parse().ok())
}

fn parse_front_matter(text: &str) -> FrontMatter {
    let mut fm = FrontMatter::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let v = v.trim().trim_matches(['"', '\'']);
        match k.trim() {
            "title" => fm.title = Some(v.to_string()).filter(|s| !s.is_empty()),
            "breaks" => fm.breaks = !matches!(v, "false" | "no" | "off"),
            "lang" => fm.lang = Some(v.to_string()),
            "tags" => {
                fm.tags = v
                    .trim_matches(['[', ']'])
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            }
            _ => {}
        }
    }
    fm
}

fn merge_text(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(inlines.len());
    for i in inlines {
        if let (Inline::Text(t), Some(Inline::Text(prev))) = (&i, out.last_mut()) {
            prev.push_str(t);
            continue;
        }
        out.push(i);
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum MarkerKind {
    Mark,
    Ins,
}

enum Tok {
    Node(Inline),
    Marker(MarkerKind),
}

/// 把同一層的 `==…==`、`++…++` 配對成螢光 / 插入。
fn pair_markers(inlines: Vec<Inline>) -> Vec<Inline> {
    if !inlines.iter().any(|i| matches!(i, Inline::Text(t) if t.contains("==") || t.contains("++"))) {
        return inlines;
    }
    let mut toks = Vec::new();
    for i in inlines {
        match i {
            Inline::Text(t) => {
                let mut rest = t.as_str();
                loop {
                    let next = [("==", MarkerKind::Mark), ("++", MarkerKind::Ins)]
                        .iter()
                        .filter_map(|(m, k)| rest.find(m).map(|p| (p, *k)))
                        .min_by_key(|(p, _)| *p);
                    match next {
                        Some((p, k)) => {
                            if p > 0 {
                                toks.push(Tok::Node(Inline::Text(rest[..p].to_string())));
                            }
                            toks.push(Tok::Marker(k));
                            rest = &rest[p + 2..];
                        }
                        None => {
                            if !rest.is_empty() {
                                toks.push(Tok::Node(Inline::Text(rest.to_string())));
                            }
                            break;
                        }
                    }
                }
            }
            other => toks.push(Tok::Node(other)),
        }
    }
    merge_text(build_pairs(toks))
}

fn tok_first_char(t: Option<&Tok>) -> Option<char> {
    match t {
        Some(Tok::Node(Inline::Text(s))) => s.chars().next(),
        Some(Tok::Node(_)) => Some('x'),
        _ => None,
    }
}

fn tok_last_char(t: Option<&Tok>) -> Option<char> {
    match t {
        Some(Tok::Node(Inline::Text(s))) => s.chars().last(),
        Some(Tok::Node(_)) => Some('x'),
        _ => None,
    }
}

fn build_pairs(toks: Vec<Tok>) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut toks: Vec<Option<Tok>> = toks.into_iter().map(Some).collect();
    let mut i = 0;
    while i < toks.len() {
        if let Some(Tok::Marker(k)) = toks[i] {
            let can_open = tok_first_char(toks.get(i + 1).and_then(|t| t.as_ref())).is_some_and(|c| !c.is_whitespace());
            if can_open {
                let close = (i + 2..toks.len()).find(|&j| {
                    matches!(toks[j], Some(Tok::Marker(k2)) if k2 == k)
                        && tok_last_char(toks[j - 1].as_ref()).is_some_and(|c| !c.is_whitespace())
                });
                if let Some(j) = close {
                    let inner: Vec<Tok> = toks[i + 1..j].iter_mut().filter_map(Option::take).collect();
                    let children = merge_text(build_pairs(inner));
                    out.push(match k {
                        MarkerKind::Mark => Inline::Mark(children),
                        MarkerKind::Ins => Inline::Ins(children),
                    });
                    i = j + 1;
                    continue;
                }
            }
            out.push(Inline::Text(if k == MarkerKind::Mark { "==" } else { "++" }.into()));
        } else if let Some(Tok::Node(n)) = toks[i].take() {
            out.push(n);
        }
        i += 1;
    }
    out
}

/// 字中間的上下標（HackMD / markdown-it-sup、sub）：`x^2^`、`H~2~O`。
/// 內容不可含空白；`~~` 是刪除線，不處理。
fn split_sup_sub(text: &str, out: &mut Vec<Inline>) {
    let mut rest = text;
    let mut plain = String::new();
    while let Some(p) = rest.find(['^', '~']) {
        let delim = rest[p..].chars().next().unwrap();
        let after = &rest[p + 1..];
        let doubled = after.starts_with(delim);
        let close = (!doubled).then(|| after.find(delim)).flatten();
        match close {
            Some(c) if c > 0 && !after[..c].contains(char::is_whitespace) => {
                plain.push_str(&rest[..p]);
                if !plain.is_empty() {
                    out.push(Inline::Text(std::mem::take(&mut plain)));
                }
                let inner = vec![Inline::Text(after[..c].to_string())];
                out.push(if delim == '^' { Inline::Sup(inner) } else { Inline::Sub(inner) });
                rest = &after[c + 1..];
            }
            _ => {
                let skip = if doubled { 2 } else { 1 };
                plain.push_str(&rest[..p + skip]);
                rest = &rest[p + skip..];
            }
        }
    }
    plain.push_str(rest);
    if !plain.is_empty() {
        out.push(Inline::Text(plain));
    }
}

/// `:smile:` → 😄
fn replace_emoji(text: &str) -> String {
    if !text.contains(':') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(':') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let name_len = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '+' || c == '-'))
            .unwrap_or(after.len());
        if name_len > 0 && after[name_len..].starts_with(':') {
            if let Some(e) = emojis::get_by_shortcode(&after[..name_len]) {
                out.push_str(e.as_str());
                rest = &after[name_len + 1..];
                continue;
            }
        }
        out.push(':');
        rest = after;
    }
    out.push_str(rest);
    out
}

/// 把 `:::` 標記組成巢狀的提示框。
fn fold_containers(blocks: Vec<Block>) -> Vec<Block> {
    fold_with_offsets(blocks.into_iter().map(|b| (0, b)).collect()).into_iter().map(|(_, b)| b).collect()
}

/// 把 `:::` 提示框的開頭／結尾記號之間的區塊收進一個 `Container`。
/// 每個區塊帶著它在原始文字中的位置；提示框的位置是開頭記號的位置。
fn fold_with_offsets(blocks: Vec<(usize, Block)>) -> Vec<(usize, Block)> {
    if !blocks.iter().any(|(_, b)| matches!(b, Block::Html(h) if h.trim_start().starts_with("<!--shosai:"))) {
        return blocks;
    }
    // （種類、標題、是否展開、開頭記號的位置、裡面的區塊）
    let mut stack: Vec<(ContainerKind, String, bool, usize, Vec<Block>)> = Vec::new();
    let mut out = Vec::new();
    let close = |stack: &mut Vec<(ContainerKind, String, bool, usize, Vec<Block>)>, out: &mut Vec<(usize, Block)>| {
        if let Some((kind, title, open, offset, inner)) = stack.pop() {
            let block = Block::Container { kind, title, open, blocks: inner };
            match stack.last_mut() {
                Some(parent) => parent.4.push(block),
                None => out.push((offset, block)),
            }
        }
    };
    for (offset, b) in blocks {
        if let Block::Html(h) = &b {
            let t = h.trim();
            if let Some(rest) = t.strip_prefix(MARK_OPEN).and_then(|r| r.strip_suffix("-->")) {
                let mut parts = rest.splitn(3, ':');
                let kind = parts.next().and_then(ContainerKind::parse).unwrap_or(ContainerKind::Info);
                let open = parts.next() == Some("1");
                let title = unhex(parts.next().unwrap_or(""));
                stack.push((kind, title, open, offset, Vec::new()));
                continue;
            }
            if t == MARK_END {
                close(&mut stack, &mut out);
                continue;
            }
        }
        match stack.last_mut() {
            Some(parent) => parent.4.push(b),
            None => out.push((offset, b)),
        }
    }
    // 沒有關閉的提示框：直接收尾
    while !stack.is_empty() {
        close(&mut stack, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(md: &str) -> Vec<Inline> {
        match parse(md).blocks.into_iter().next() {
            Some(Block::Paragraph(i)) => i,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn mark_ins_sup_sub() {
        let i = para("a ==亮== b ++底線++ x^2^ H~2~O ~~刪~~");
        assert!(i.contains(&Inline::Mark(vec![Inline::Text("亮".into())])), "{i:?}");
        assert!(i.contains(&Inline::Ins(vec![Inline::Text("底線".into())])), "{i:?}");
        assert!(i.contains(&Inline::Sup(vec![Inline::Text("2".into())])), "{i:?}");
        assert!(i.contains(&Inline::Sub(vec![Inline::Text("2".into())])), "{i:?}");
        assert!(i.contains(&Inline::Strike(vec![Inline::Text("刪".into())])), "{i:?}");
    }

    #[test]
    fn mark_around_strong_and_literal() {
        let i = para("==**粗**== 與 a == b，C++ 與 C++");
        assert!(matches!(&i[0], Inline::Mark(c) if matches!(c[0], Inline::Strong(_))), "{i:?}");
        assert_eq!(plain_text(&i[1..]), " 與 a == b，C++ 與 C++");
    }

    #[test]
    fn math() {
        let i = para("能量 $E=mc^2$ 守恆");
        assert!(i.contains(&Inline::Math { tex: "E=mc^2".into(), display: false }));
        let d = parse("$$\n\\frac{a}{b}\n$$\n\n```math\nx^2\n```");
        assert!(matches!(&d.blocks[0], Block::Math(t) if t.contains("\\frac")), "{:?}", d.blocks);
        assert_eq!(d.blocks[1], Block::Math("x^2".into()));
    }

    #[test]
    fn containers() {
        let d = parse(":::info\n資訊 **粗**\n:::\n\n:::spoiler 點我展開\n秘密\n:::\n\n:::warning\n外層\n:::danger\n內層\n:::\n:::\n");
        assert!(
            matches!(&d.blocks[0], Block::Container { kind: ContainerKind::Info, blocks, .. } if blocks.len() == 1)
        );
        assert!(
            matches!(&d.blocks[1], Block::Container { kind: ContainerKind::Spoiler, title, .. } if title == "點我展開"),
            "{:?}",
            d.blocks
        );
        match &d.blocks[2] {
            Block::Container { kind: ContainerKind::Warning, blocks, .. } => {
                assert!(matches!(blocks[1], Block::Container { kind: ContainerKind::Danger, .. }), "{blocks:?}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn code_inside_fence_untouched() {
        let d = parse("```\n:::info\n[TOC]\n```");
        assert_eq!(
            d.blocks,
            vec![Block::Code { lang: String::new(), code: ":::info\n[TOC]".into(), line_start: None }]
        );
    }

    #[test]
    fn toc_and_headings() {
        let d = parse("[TOC]\n\n# 標題 一\n\n## Hello, World!\n\n## Hello, World!");
        assert_eq!(d.blocks[0], Block::Toc);
        let h = d.headings();
        assert_eq!(h[0].id, "標題-一");
        assert_eq!(h[1].id, "hello-world");
        assert_eq!(h[2].id, "hello-world-1");
    }

    #[test]
    fn source_offsets() {
        let src = "---\ntitle: x\n---\n\n# 一\n\n段落\n\n:::info\n## 框內\n\n內文\n:::\n\n- 清單\n\n---\n\n> ## 引言裡\n\n最後\n";
        let d = parse(src);
        assert_eq!(d.offsets.len(), d.blocks.len());
        assert!(d.offsets.windows(2).all(|w| w[0] < w[1]), "{:?}", d.offsets);
        let at = |i: usize| &src[d.offsets[i]..];
        assert!(matches!(d.blocks[0], Block::Heading { .. }) && at(0).starts_with("# 一"));
        assert!(matches!(d.blocks[1], Block::Paragraph(_)) && at(1).starts_with("段落"));
        // 提示框前面的文字經過前處理（位置會位移），仍要對回原始文字
        assert!(matches!(d.blocks[2], Block::Container { .. }) && at(2).starts_with(":::info"), "{:?}", at(2));
        assert!(matches!(d.blocks[3], Block::List { .. }) && at(3).starts_with("- 清單"), "{:?}", at(3));
        assert!(matches!(d.blocks[4], Block::Rule) && at(4).starts_with("---"));
        assert!(matches!(d.blocks[5], Block::Quote(_)) && at(5).starts_with("> ## 引言裡"));
        assert!(at(6).starts_with("最後"));

        // 標題（含提示框、引言裡的）也各自記得位置
        let h = d.headings();
        let texts: Vec<&str> = h.iter().map(|h| h.text.as_str()).collect();
        assert_eq!(texts, ["一", "框內", "引言裡"]);
        assert!(src[h[0].offset..].starts_with("# 一"));
        assert!(src[h[1].offset..].starts_with("## 框內"));
        assert!(src[h[2].offset..].starts_with("## 引言裡"));
    }

    #[test]
    fn breaks_follow_front_matter() {
        assert!(para("一\n二").contains(&Inline::HardBreak));
        let d = parse("---\nbreaks: false\ntitle: 測試\n---\n一\n二");
        assert!(!d.meta.breaks);
        assert_eq!(d.meta.title.as_deref(), Some("測試"));
        assert!(matches!(&d.blocks[0], Block::Paragraph(i) if i.contains(&Inline::SoftBreak)));
    }

    #[test]
    fn image_size_and_emoji_and_embed() {
        let i = para("![貓](cat.png =200x) :smile: {%youtube abc123 %}");
        assert!(
            matches!(&i[0], Inline::Image { url, width: Some(200), height: None, alt, .. } if url == "cat.png" && alt == "貓"),
            "{i:?}"
        );
        assert!(plain_text(&i).contains('😄'), "{i:?}");
        assert!(i.iter().any(|x| matches!(x, Inline::Link { url, .. } if url.contains("abc123"))), "{i:?}");
        let i = para("![a](b.png \"說明\" =100x50)");
        assert!(matches!(&i[0], Inline::Image { title, width: Some(100), height: Some(50), .. } if title == "說明"));
    }

    #[test]
    fn code_line_numbers() {
        let d = parse("```js=\na\n```\n\n```rust=101\nb\n```");
        assert!(matches!(&d.blocks[0], Block::Code { lang, line_start: Some(1), .. } if lang == "js"));
        assert!(matches!(&d.blocks[1], Block::Code { lang, line_start: Some(101), .. } if lang == "rust"));
    }

    #[test]
    fn task_offsets_map_to_original() {
        let src = ":::info\nx\n:::\n- [ ] 待辦\n";
        let d = parse(src);
        let Block::List { items, .. } = &d.blocks[1] else { panic!("{:?}", d.blocks) };
        let Block::Paragraph(i) = &items[0][0] else { panic!() };
        let Inline::Task { offset, checked: false } = i[0] else { panic!("{i:?}") };
        assert_eq!(&src[offset..offset + 3], "[ ]");
    }

    #[test]
    fn footnotes_and_deflist_and_alerts() {
        let d = parse("文[^a]與[^b]\n\n[^b]: 二\n[^a]: 一\n\n名詞\n: 解釋\n\n> [!WARNING]\n> 小心");
        assert_eq!(d.footnotes[0].label, "a");
        assert_eq!(d.footnotes[1].number, 2);
        assert!(d.blocks.iter().any(|b| matches!(b, Block::DefList(_))), "{:?}", d.blocks);
        assert!(d.blocks.iter().any(|b| matches!(b, Block::Container { kind: ContainerKind::Warning, .. })));
    }
}
