//! Markdown → Typst 轉換，並套用「書本」版型。

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Parser, Tag, TagEnd};

use super::{resolve_local_image, Document};
use crate::markdown::{is_cjk, level_num, options};

/// 轉換結果：Typst 原始碼 + 需要的圖片（虛擬路徑 → 實際檔案）。
#[derive(Debug, Clone, Default)]
pub struct TypstOutput {
    pub source: String,
    pub images: Vec<(String, PathBuf)>,
}

/// 將整份文件轉成 Typst 原始碼。
pub fn render(doc: &Document) -> TypstOutput {
    let mut images = Vec::new();
    let mut src = preamble(doc);

    for ch in &doc.chapters {
        let body = markdown_to_typst(&ch.markdown, &ch.base_dir, &mut images);
        let starts_with_h1 = crate::markdown::outline(&ch.markdown).first().is_some_and(|h| h.level == 1);
        if doc.is_book && !starts_with_h1 {
            let _ = writeln!(src, "= {}\n", escape_markup(&ch.title));
        }
        src.push_str(&body);
        src.push('\n');
    }
    TypstOutput { source: src, images }
}

fn preamble(doc: &Document) -> String {
    let l = &doc.layout;
    let mut s = String::new();
    let (lang, region) = split_lang(&doc.language);
    let cjk = matches!(lang.as_str(), "zh" | "ja" | "ko");
    let fonts = l.fonts.iter().map(|f| typst_str(f)).collect::<Vec<_>>().join(", ");
    let leading = (l.line_height - 1.0).max(0.2);
    let spacing = if l.indent { leading } else { leading + 0.5 };

    let _ = writeln!(s, "// 由 書齋 shosai 自動產生");
    let _ = writeln!(
        s,
        "#set document(title: {}{})",
        typst_str(&doc.title),
        if doc.author.is_empty() { String::new() } else { format!(", author: {}", typst_str(&doc.author)) }
    );
    let _ = writeln!(
        s,
        "#set text(font: ({fonts},), size: {}pt, lang: {}{})",
        l.font_size,
        typst_str(&lang),
        region.map(|r| format!(", region: {}", typst_str(&r))).unwrap_or_default()
    );
    let _ = writeln!(
        s,
        "#set page(paper: {}, margin: (inside: 2.2cm, outside: 1.8cm, top: 2.2cm, bottom: 2.2cm))",
        typst_str(l.typst_paper())
    );
    let _ = writeln!(
        s,
        "#set par(justify: true, leading: {leading:.2}em, spacing: {spacing:.2}em{})",
        if l.indent { ", first-line-indent: (amount: 2em, all: true)" } else { "" }
    );
    s.push_str(TEMPLATE_COMMON);

    if l.numbered {
        let chapter =
            if cjk { r#""第" + numbering("一", n.first()) + "章""# } else { r#""Chapter " + str(n.first())"# };
        let _ = writeln!(
            s,
            "#set heading(numbering: (..nums) => {{ let n = nums.pos(); if n.len() == 1 {{ {chapter} }} else {{ numbering(\"1.1\", ..n) }} }})"
        );
    }

    if doc.is_book {
        s.push_str(TEMPLATE_BOOK);
        if l.cover {
            let _ = writeln!(s, "#page(header: none, footer: none)[");
            let _ = writeln!(s, "  #set par(first-line-indent: 0em, justify: false)");
            let _ = writeln!(s, "  #v(28%)");
            let _ = writeln!(s, "  #align(center)[");
            let _ = writeln!(s, "    #text(size: 2.3em, weight: \"bold\")[{}]", escape_markup(&doc.title));
            if !doc.subtitle.is_empty() {
                let _ = writeln!(
                    s,
                    "    #v(0.6em)\n    #text(size: 1.2em, fill: luma(90))[{}]",
                    escape_markup(&doc.subtitle)
                );
            }
            let _ = writeln!(s, "    #v(1.2em)\n    #line(length: 25%, stroke: 0.6pt + luma(140))");
            if !doc.author.is_empty() {
                let _ = writeln!(s, "    #v(2.5em)\n    #text(size: 1.1em)[{}]", escape_markup(&doc.author));
            }
            let _ = writeln!(s, "  ]\n]");
        }
        if l.toc {
            let title = if cjk { "目錄" } else { "Contents" };
            let _ = writeln!(
                s,
                "#page(header: none, footer: none)[\n  #outline(title: {}, depth: {})\n]",
                typst_str(title),
                l.toc_depth.clamp(1, 6)
            );
        }
        let _ = writeln!(s, "#counter(page).update(1)\n");
    } else {
        s.push_str(TEMPLATE_ARTICLE);
    }
    s
}

const TEMPLATE_COMMON: &str = r##"#show raw: set text(font: ("DejaVu Sans Mono", "Cascadia Mono", "Consolas", "Menlo", "Noto Sans Mono CJK TC", "Sarasa Mono TC", "WenQuanYi Zen Hei Mono"), size: 1.12em)
#show raw.where(block: true): it => block(fill: luma(246), inset: 9pt, radius: 3pt, width: 100%, stroke: 0.4pt + luma(225), it)
#show quote.where(block: true): it => block(stroke: (left: 2pt + luma(185)), inset: (left: 1em, y: 0.25em), text(fill: luma(70), it.body))
#show link: set text(fill: rgb("#2a5db0"))
#set table(stroke: 0.5pt + luma(160), inset: 6pt)
#show table: set par(first-line-indent: 0em, justify: false)
#show list: set par(first-line-indent: 0em)
#show enum: set par(first-line-indent: 0em)
#show figure: set par(first-line-indent: 0em)
#show figure.caption: set text(size: 0.85em, fill: luma(90))
#show heading: set block(above: 1.8em, below: 1em)
#show heading.where(level: 2): set text(size: 1.25em)
#show heading.where(level: 3): set text(size: 1.1em)
#show footnote.entry: set par(first-line-indent: 0em)
#let md-image(path, alt: none) = layout(size => {
  let img = image(path, alt: alt)
  if measure(img).width > size.width { image(path, width: 100%, alt: alt) } else { img }
})
#let md-check(done) = box(width: 0.8em, height: 0.8em, baseline: 0.1em, stroke: 0.6pt + luma(80), radius: 1.5pt, fill: if done { luma(90) } else { none })
"##;

const TEMPLATE_BOOK: &str = r##"#set page(
  footer: context {
    set text(size: 0.8em, fill: luma(110))
    align(center, counter(page).display())
  },
  header: context {
    let p = here().page()
    let opening = query(heading.where(level: 1)).any(h => h.location().page() == p)
    let prev = query(heading.where(level: 1).before(here()))
    if not opening and prev.len() > 0 {
      set text(size: 0.75em, fill: luma(120))
      align(if calc.even(p) { left } else { right }, prev.last().body)
    }
  },
)
#show heading.where(level: 1, outlined: true): it => {
  pagebreak(weak: true)
  set par(first-line-indent: 0em, justify: false)
  v(14%)
  if it.numbering != none {
    block(text(size: 1em, fill: luma(120), counter(heading).display(it.numbering)))
  }
  block(text(size: 1.9em, weight: "bold", it.body))
  v(0.4em)
  line(length: 28%, stroke: 0.6pt + luma(150))
  v(2.2em)
}
#show heading.where(level: 1, outlined: false): it => block(above: 2em, below: 1.6em, text(size: 1.7em, weight: "bold", it.body))
#show outline.entry.where(level: 1): set text(weight: "bold")
#show outline.entry.where(level: 1): set block(above: 1.1em)
"##;

const TEMPLATE_ARTICLE: &str = r##"#set page(numbering: "1")
#show heading.where(level: 1): set text(size: 1.6em)
"##;

fn split_lang(tag: &str) -> (String, Option<String>) {
    let mut parts = tag.split(['-', '_']);
    let lang = parts.next().unwrap_or("zh").to_ascii_lowercase();
    let region = parts.next().filter(|r| r.len() == 2).map(|r| r.to_ascii_lowercase());
    let lang = if lang.is_empty() { "zh".into() } else { lang };
    (lang, region)
}

/// Typst 字串常值（含引號）。
pub fn typst_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 跳脫 Typst 標記語法中的特殊字元，讓文字原樣輸出。
pub fn escape_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' | '#' | '*' | '_' | '`' | '$' | '<' | '>' | '@' | '[' | ']' | '~' | '/' | '=' | '-' | '+' | '.'
            | '(' | ')' | ';' | ':' | '\'' | '"' => {
                out.push('\\');
                out.push(c);
            }
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// 將一段 Markdown 轉成 Typst 標記。
pub fn markdown_to_typst(md: &str, base_dir: &Path, images: &mut Vec<(String, PathBuf)>) -> String {
    let events: Vec<Event> = Parser::new_ext(md, options()).collect();

    // 先把註腳定義抽出來，引用處再以 #footnote[...] 內嵌
    let mut footnotes = HashMap::new();
    let mut body = Vec::with_capacity(events.len());
    let mut iter = events.into_iter();
    while let Some(ev) = iter.next() {
        if let Event::Start(Tag::FootnoteDefinition(label)) = ev {
            let mut inner = Vec::new();
            for e in iter.by_ref() {
                if matches!(e, Event::End(TagEnd::FootnoteDefinition)) {
                    break;
                }
                inner.push(e);
            }
            let mut sub = Converter::new(base_dir, images, HashMap::new());
            sub.run(inner);
            footnotes.insert(label.to_string(), sub.out.trim().to_string());
        } else {
            body.push(ev);
        }
    }

    let mut conv = Converter::new(base_dir, images, footnotes);
    conv.run(body);
    conv.out
}

struct Converter<'a> {
    out: String,
    base_dir: &'a Path,
    images: &'a mut Vec<(String, PathBuf)>,
    footnotes: HashMap<String, String>,
    pending_soft: bool,
    /// 程式碼區塊內容（收集中時為 Some）
    code: Option<(String, String)>,
    /// 圖片替代文字（收集中時為 Some）
    image: Option<(Option<String>, String)>,
    /// 目前段落起點，以及段落是否只有一張圖
    para: Option<(usize, ParaContent)>,
    in_heading: bool,
    in_table_head: bool,
}

#[derive(Clone, PartialEq)]
enum ParaContent {
    Empty,
    OnlyImage(String, String),
    Mixed,
}

impl<'a> Converter<'a> {
    fn new(base_dir: &'a Path, images: &'a mut Vec<(String, PathBuf)>, footnotes: HashMap<String, String>) -> Self {
        Converter {
            out: String::new(),
            base_dir,
            images,
            footnotes,
            pending_soft: false,
            code: None,
            image: None,
            para: None,
            in_heading: false,
            in_table_head: false,
        }
    }

    fn run(&mut self, events: Vec<Event>) {
        for ev in events {
            self.event(ev);
        }
    }

    fn mark_mixed(&mut self) {
        if let Some((_, content)) = self.para.as_mut() {
            *content = ParaContent::Mixed;
        }
    }

    /// 輸出 Typst 語法片段（非純文字）。
    fn raw(&mut self, s: &str) {
        self.flush_soft(None);
        self.out.push_str(s);
    }

    fn flush_soft(&mut self, next: Option<char>) {
        if !self.pending_soft {
            return;
        }
        self.pending_soft = false;
        let prev = self.out.chars().last();
        let both_cjk = prev.is_some_and(is_cjk) && next.is_some_and(is_cjk);
        if !both_cjk {
            self.out.push(' ');
        }
    }

    fn text(&mut self, t: &str) {
        if t.is_empty() {
            return;
        }
        if !t.trim().is_empty() {
            self.mark_mixed();
        }
        self.flush_soft(t.chars().next());
        self.out.push_str(&escape_markup(t));
    }

    fn block_end(&mut self) {
        self.pending_soft = false;
        if self.in_heading {
            return;
        }
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        self.out.push_str("\n\n");
    }

    fn event(&mut self, ev: Event) {
        // 收集程式碼區塊
        if let Some((_, code)) = self.code.as_mut() {
            match ev {
                Event::Text(t) => code.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, code) = self.code.take().unwrap();
                    let code = code.strip_suffix('\n').unwrap_or(&code).to_string();
                    let lang = if lang.is_empty() { String::new() } else { format!(", lang: {}", typst_str(&lang)) };
                    let s = format!("#raw({}, block: true{lang})", typst_str(&code));
                    self.raw(&s);
                    self.block_end();
                }
                _ => {}
            }
            return;
        }
        // 收集圖片替代文字
        if let Some((_, alt)) = self.image.as_mut() {
            match ev {
                Event::Text(t) | Event::Code(t) => alt.push_str(&t),
                Event::End(TagEnd::Image) => self.finish_image(),
                _ => {}
            }
            return;
        }

        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t),
            Event::Code(t) => {
                self.mark_mixed();
                let s = format!("#raw({})", typst_str(&t));
                self.raw(&s);
            }
            Event::InlineMath(t) => {
                let s = format!("#raw({})", typst_str(&t));
                self.raw(&s);
            }
            Event::DisplayMath(t) => {
                let s = format!("#raw({}, block: true)", typst_str(&t));
                self.raw(&s);
            }
            Event::Html(h) | Event::InlineHtml(h) => self.html(&h),
            Event::FootnoteReference(label) => {
                self.mark_mixed();
                let s = match self.footnotes.get(label.as_ref()) {
                    Some(content) => format!("#footnote[{content}]"),
                    None => format!("#super[{}]", escape_markup(&label)),
                };
                self.raw(&s);
            }
            Event::SoftBreak => self.pending_soft = true,
            Event::HardBreak => {
                if self.in_heading {
                    self.raw(" ");
                } else {
                    self.raw(" \\\n");
                }
            }
            Event::Rule => {
                self.raw("#align(center, line(length: 40%, stroke: 0.5pt + luma(150)))");
                self.block_end();
            }
            Event::TaskListMarker(done) => self.raw(&format!("#md-check({done}) ")),
        }
    }

    fn html(&mut self, h: &str) {
        let t = h.trim().to_ascii_lowercase();
        if t.starts_with("<br") {
            self.raw(" \\\n");
        } else if t.contains("pagebreak") || t.contains("page-break") || t.contains("break-after: page") {
            self.raw("#pagebreak()");
            self.block_end();
        }
        // 其他 HTML 不輸出到 PDF
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                self.para = Some((self.out.len(), ParaContent::Empty));
            }
            Tag::Heading { level, .. } => {
                self.raw(&format!("{} ", "=".repeat(level_num(level) as usize)));
                self.in_heading = true;
            }
            Tag::BlockQuote(_) => self.raw("#quote(block: true)["),
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.split([' ', ',', '{']).next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::HtmlBlock => {}
            Tag::List(Some(start)) => self.raw(&format!("#enum(start: {start}, tight: false, ")),
            Tag::List(None) => self.raw("#list(tight: false, "),
            Tag::Item => self.raw("["),
            Tag::Table(aligns) => {
                let a = aligns
                    .iter()
                    .map(|a| match a {
                        Alignment::None | Alignment::Left => "left",
                        Alignment::Center => "center",
                        Alignment::Right => "right",
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                self.raw(&format!("#table(columns: {}, align: ({a},),\n", aligns.len().max(1)));
            }
            Tag::TableHead => {
                self.in_table_head = true;
                self.raw("table.header(");
            }
            Tag::TableRow => {}
            Tag::TableCell => {
                if self.in_table_head {
                    self.raw("[*");
                } else {
                    self.raw("[");
                }
            }
            Tag::Emphasis => {
                self.mark_mixed();
                self.raw("#emph[");
            }
            Tag::Strong => {
                self.mark_mixed();
                self.raw("#strong[");
            }
            Tag::Strikethrough => self.raw("#strike["),
            Tag::Superscript => self.raw("#super["),
            Tag::Subscript => self.raw("#sub["),
            Tag::Link { dest_url, .. } => {
                self.mark_mixed();
                let url = dest_url.as_ref();
                if url.contains("://") || url.starts_with("mailto:") {
                    self.raw(&format!("#link({})[", typst_str(url)));
                } else {
                    // 文件內連結、相對連結：只保留文字
                    self.raw("#[");
                }
            }
            Tag::Image { dest_url, .. } => {
                let path = resolve_local_image(self.base_dir, &dest_url).filter(|p| is_supported_image(p));
                let vpath = path.map(|p| {
                    if let Some((v, _)) = self.images.iter().find(|(_, real)| *real == p) {
                        v.clone()
                    } else {
                        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("png").to_ascii_lowercase();
                        let v = format!("/__img/{}.{ext}", self.images.len() + 1);
                        self.images.push((v.clone(), p));
                        v
                    }
                });
                self.image = Some((vpath, String::new()));
            }
            Tag::FootnoteDefinition(_) | Tag::MetadataBlock(_) => {}
            Tag::DefinitionList | Tag::DefinitionListTitle | Tag::DefinitionListDefinition => {}
        }
    }

    fn finish_image(&mut self) {
        let Some((vpath, alt)) = self.image.take() else { return };
        match vpath {
            Some(v) => {
                let alt_arg = if alt.is_empty() { String::new() } else { format!(", alt: {}", typst_str(&alt)) };
                let s = format!("#md-image({}{alt_arg})", typst_str(&v));
                let was_empty = matches!(self.para, Some((_, ParaContent::Empty)));
                self.raw(&s);
                if let Some((_, content)) = self.para.as_mut() {
                    *content = if was_empty { ParaContent::OnlyImage(v, alt) } else { ParaContent::Mixed };
                }
            }
            None => {
                self.mark_mixed();
                let label = if alt.is_empty() { "圖片".to_string() } else { alt };
                let s = format!("#text(fill: luma(120))[\\[{}\\]]", escape_markup(&label));
                self.raw(&s);
            }
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                if let Some((start, ParaContent::OnlyImage(v, alt))) = self.para.take() {
                    // 獨立成段的圖片 → 置中圖表，替代文字當圖說
                    self.out.truncate(start);
                    let caption =
                        if alt.is_empty() { String::new() } else { format!(", caption: [{}]", escape_markup(&alt)) };
                    let alt_arg = if alt.is_empty() { String::new() } else { format!(", alt: {}", typst_str(&alt)) };
                    let s = format!("#figure(md-image({}{alt_arg}){caption})", typst_str(&v));
                    self.raw(&s);
                }
                self.para = None;
                self.block_end();
            }
            TagEnd::Heading(_) => {
                self.in_heading = false;
                self.pending_soft = false;
                self.block_end();
            }
            TagEnd::BlockQuote(_) => {
                self.trim_trailing_newlines();
                self.raw("]");
                self.block_end();
            }
            TagEnd::List(_) => {
                self.raw(")");
                self.block_end();
            }
            TagEnd::Item => {
                self.trim_trailing_newlines();
                self.raw("],\n");
            }
            TagEnd::Table => {
                self.raw(")");
                self.block_end();
            }
            TagEnd::TableHead => {
                self.in_table_head = false;
                self.raw("),\n");
            }
            TagEnd::TableRow => self.raw("\n"),
            TagEnd::TableCell => {
                if self.in_table_head {
                    self.raw("*], ");
                } else {
                    self.raw("], ");
                }
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::Link => self.raw("]"),
            _ => {}
        }
    }

    fn trim_trailing_newlines(&mut self) {
        self.pending_soft = false;
        while self.out.ends_with('\n') || self.out.ends_with(' ') {
            self.out.pop();
        }
    }
}

fn is_supported_image(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "webp")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(md: &str) -> String {
        markdown_to_typst(md, Path::new("."), &mut Vec::new())
    }

    #[test]
    fn escapes_text() {
        assert_eq!(escape_markup("#a *b* [c]"), "\\#a \\*b\\* \\[c\\]");
    }

    #[test]
    fn basic_blocks() {
        let t = conv("# 標題\n\n一段*強調*文字。\n\n```rust\nfn main() {}\n```\n");
        assert!(t.contains("= 標題"), "{t}");
        assert!(t.contains("#emph[強調]"), "{t}");
        assert!(t.contains("#raw(\"fn main() {}\", block: true, lang: \"rust\")"), "{t}");
    }

    #[test]
    fn cjk_soft_break_has_no_space() {
        let t = conv("第一行\n第二行\nabc\ndef");
        assert!(t.contains("第一行第二行 abc def"), "{t}");
    }

    #[test]
    fn footnotes_inline() {
        let t = conv("正文[^1]。\n\n[^1]: 註腳內容\n");
        assert!(t.contains("#footnote[註腳內容]"), "{t}");
    }

    #[test]
    fn missing_image_is_placeholder() {
        let t = conv("![貓](不存在.png)");
        assert!(t.contains("貓") && !t.contains("md-image"), "{t}");
    }
}
