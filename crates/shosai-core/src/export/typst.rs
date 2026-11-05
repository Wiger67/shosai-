//! Markdown → Typst 轉換，並套用「書本」版型。

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::{resolve_local_image, Document};
use crate::hmd::{self, Align, Block, ContainerKind, Inline, MdDoc};
use crate::markdown::is_cjk;
use crate::math;

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
        let md = hmd::parse(&ch.markdown);
        let body = doc_to_typst(&md, &ch.base_dir, &mut images);
        if doc.is_book && !md.starts_with_h1() {
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
    s.push_str(math::MITEX_IMPORT);
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
#let md-image(path, alt: none, width: auto) = layout(size => {
  let img = image(path, alt: alt, width: width)
  if measure(img).width > size.width { image(path, width: 100%, alt: alt) } else { img }
})
#let md-code(body, start: none) = {
  if start == none { body } else {
    show raw.line: it => box(width: 2.4em, align(right, text(fill: luma(150), str(it.number + start - 1)))) + h(0.9em) + it.body
    body
  }
}
#let md-box-colors = (
  info: (rgb("#d9edf7"), rgb("#bce8f1"), rgb("#31708f")),
  success: (rgb("#dff0d8"), rgb("#d6e9c6"), rgb("#3c763d")),
  warning: (rgb("#fcf8e3"), rgb("#faebcc"), rgb("#8a6d3b")),
  danger: (rgb("#f2dede"), rgb("#ebccd1"), rgb("#a94442")),
)
#let md-box(kind, title: none, body) = {
  let (bg, border, fg) = md-box-colors.at(kind)
  block(fill: bg, stroke: 0.6pt + border, inset: (x: 11pt, y: 9pt), radius: 3pt, width: 100%, breakable: true, {
    set par(first-line-indent: 0em)
    set text(fill: fg)
    if title != none { block(below: 0.7em, text(weight: "bold", title)) }
    body
  })
}
#let md-spoiler(title, body) = block(stroke: 0.6pt + luma(200), inset: (x: 11pt, y: 9pt), radius: 3pt, width: 100%, {
  set par(first-line-indent: 0em)
  block(below: 0.7em, text(weight: "bold")[▸ #title])
  body
})
#let md-toc(entries) = block(inset: (y: 0.3em), {
  set par(first-line-indent: 0em)
  for (level, body) in entries { block(above: 0.45em, below: 0.45em, pad(left: (level - 1) * 1.2em, body)) }
})
#show math.equation: set text(font: ("New Computer Modern Math", "STIX Two Math", "Cambria Math"))
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

/// 將一段 Markdown（HackMD 語法）轉成 Typst 標記。
pub fn markdown_to_typst(md: &str, base_dir: &Path, images: &mut Vec<(String, PathBuf)>) -> String {
    doc_to_typst(&hmd::parse(md), base_dir, images)
}

/// 將已解析的文件轉成 Typst 標記。
pub fn doc_to_typst(doc: &MdDoc, base_dir: &Path, images: &mut Vec<(String, PathBuf)>) -> String {
    let mut r = Renderer { doc, base_dir, images, out: String::new(), in_heading: false };
    r.blocks(&doc.blocks);
    r.out
}

struct Renderer<'a> {
    doc: &'a MdDoc,
    base_dir: &'a Path,
    images: &'a mut Vec<(String, PathBuf)>,
    out: String,
    in_heading: bool,
}

impl Renderer<'_> {
    /// 在獨立緩衝區中渲染（用於清單項目、註腳等），回傳結果。
    fn capture(&mut self, f: impl FnOnce(&mut Self)) -> String {
        let saved = std::mem::take(&mut self.out);
        f(self);
        let inner = std::mem::replace(&mut self.out, saved);
        inner.trim().to_string()
    }

    fn blocks(&mut self, blocks: &[Block]) {
        for b in blocks {
            self.block(b);
        }
    }

    fn end_block(&mut self) {
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        self.out.push_str("\n\n");
    }

    fn block(&mut self, b: &Block) {
        match b {
            Block::Paragraph(inl) => {
                if let Some(fig) = self.figure(inl) {
                    self.out.push_str(&fig);
                } else {
                    self.inlines(inl);
                }
                self.end_block();
            }
            Block::Heading { level, content, .. } => {
                let _ = write!(self.out, "{} ", "=".repeat(*level as usize));
                self.in_heading = true;
                self.inlines(content);
                self.in_heading = false;
                self.end_block();
            }
            Block::Quote(bs) => {
                let inner = self.capture(|r| r.blocks(bs));
                let _ = write!(self.out, "#quote(block: true)[{inner}]");
                self.end_block();
            }
            Block::Code { lang, code, line_start } => {
                let lang = if lang.is_empty() { String::new() } else { format!(", lang: {}", typst_str(lang)) };
                let start = line_start.map(|n| format!(", start: {n}")).unwrap_or_default();
                let _ = write!(self.out, "#md-code(raw({}, block: true{lang}){start})", typst_str(code));
                self.end_block();
            }
            Block::Math(tex) => {
                self.out.push_str(&equation(tex, true));
                self.end_block();
            }
            Block::List { start, items } => {
                let tight = items.iter().all(|i| i.len() <= 1 || !matches!(i[1], Block::Paragraph(_)));
                match start {
                    Some(n) => {
                        let _ = write!(self.out, "#enum(start: {n}, tight: {tight}, ");
                    }
                    None => {
                        let _ = write!(self.out, "#list(tight: {tight}, ");
                    }
                }
                for item in items {
                    let inner = self.capture(|r| r.blocks(item));
                    let _ = write!(self.out, "[{inner}], ");
                }
                self.out.push(')');
                self.end_block();
            }
            Block::Table { aligns, header, rows } => {
                let cols = header.len().max(rows.iter().map(Vec::len).max().unwrap_or(0)).max(1);
                let a = (0..cols)
                    .map(|i| match aligns.get(i) {
                        Some(Align::Center) => "center",
                        Some(Align::Right) => "right",
                        _ => "left",
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(self.out, "#table(columns: {cols}, align: ({a},),");
                if !header.is_empty() {
                    self.out.push_str("table.header(");
                    for cell in header {
                        let c = self.capture(|r| r.inlines(cell));
                        let _ = write!(self.out, "[*{c}*], ");
                    }
                    self.out.push_str("),\n");
                }
                for row in rows {
                    for i in 0..cols {
                        let c = row.get(i).map(|cell| self.capture(|r| r.inlines(cell))).unwrap_or_default();
                        let _ = write!(self.out, "[{c}], ");
                    }
                    self.out.push('\n');
                }
                self.out.push(')');
                self.end_block();
            }
            Block::Rule => {
                self.out.push_str("#align(center, line(length: 40%, stroke: 0.5pt + luma(150)))");
                self.end_block();
            }
            Block::Container { kind, title, blocks, .. } => {
                let inner = self.capture(|r| r.blocks(blocks));
                let title_c = if title.is_empty() { String::new() } else { escape_markup(title) };
                match kind {
                    ContainerKind::Spoiler => {
                        let t = if title_c.is_empty() { "詳細內容".to_string() } else { title_c };
                        let _ = write!(self.out, "#md-spoiler([{t}])[{inner}]");
                    }
                    k => {
                        let t = if title.is_empty() { String::new() } else { format!(", title: [{title_c}]") };
                        let _ = write!(self.out, "#md-box({}{t})[{inner}]", typst_str(k.name()));
                    }
                }
                self.end_block();
            }
            Block::DefList(items) => {
                self.out.push_str("#terms(tight: false, ");
                for item in items {
                    let term = self.capture(|r| r.inlines(&item.term));
                    let defs: Vec<String> = item.defs.iter().map(|d| self.capture(|r| r.blocks(d))).collect();
                    let _ = write!(self.out, "terms.item[{term}][{}], ", defs.join("\n\n"));
                }
                self.out.push(')');
                self.end_block();
            }
            Block::Toc => {
                let headings = self.doc.headings();
                if headings.is_empty() {
                    return;
                }
                let min = headings.iter().map(|h| h.level).min().unwrap_or(1);
                let entries: Vec<String> =
                    headings.iter().map(|h| format!("({}, [{}])", h.level - min + 1, escape_markup(&h.text))).collect();
                let _ = write!(self.out, "#md-toc(({},))", entries.join(", "));
                self.end_block();
            }
            Block::PageBreak => {
                self.out.push_str("#pagebreak()");
                self.end_block();
            }
            Block::Html(_) => {}
        }
    }

    /// 段落中只有一張本機圖片 → 置中圖表，替代文字當圖說。
    fn figure(&mut self, inl: &[Inline]) -> Option<String> {
        let mut meaningful = inl.iter().filter(|i| !matches!(i, Inline::Text(t) if t.trim().is_empty()));
        let (Some(Inline::Image { url, alt, width, .. }), None) = (meaningful.next(), meaningful.next()) else {
            return None;
        };
        let vpath = self.image_path(url)?;
        let caption = if alt.is_empty() { String::new() } else { format!(", caption: [{}]", escape_markup(alt)) };
        Some(format!("#figure({}{caption})", image_call(&vpath, alt, *width)))
    }

    fn image_path(&mut self, url: &str) -> Option<String> {
        let path = resolve_local_image(self.base_dir, url).filter(|p| is_supported_image(p))?;
        if let Some((v, _)) = self.images.iter().find(|(_, real)| *real == path) {
            return Some(v.clone());
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png").to_ascii_lowercase();
        let v = format!("/__img/{}.{ext}", self.images.len() + 1);
        self.images.push((v.clone(), path));
        Some(v)
    }

    fn inlines(&mut self, inl: &[Inline]) {
        for (i, x) in inl.iter().enumerate() {
            match x {
                Inline::SoftBreak => {
                    // 中文之間的換行不加空白
                    let prev = self.out.chars().last();
                    let next = match inl.get(i + 1) {
                        Some(Inline::Text(t)) => t.chars().next(),
                        _ => None,
                    };
                    if !(prev.is_some_and(is_cjk) && next.is_some_and(is_cjk)) {
                        self.out.push(' ');
                    }
                }
                other => self.inline(other),
            }
        }
    }

    fn wrapped(&mut self, open: &str, children: &[Inline]) {
        let inner = self.capture_inline(children);
        let _ = write!(self.out, "{open}[{inner}]");
    }

    /// 行內內容不可 trim（空白有意義），所以另外處理。
    fn capture_inline(&mut self, children: &[Inline]) -> String {
        let saved = std::mem::take(&mut self.out);
        self.inlines(children);
        std::mem::replace(&mut self.out, saved)
    }

    fn inline(&mut self, x: &Inline) {
        match x {
            Inline::Text(t) => self.out.push_str(&escape_markup(t)),
            Inline::Code(t) => {
                let _ = write!(self.out, "#raw({})", typst_str(t));
            }
            Inline::Math { tex, display } => self.out.push_str(&equation(tex, *display)),
            Inline::Emph(c) => self.wrapped("#emph", c),
            Inline::Strong(c) => self.wrapped("#strong", c),
            Inline::Strike(c) => self.wrapped("#strike", c),
            Inline::Mark(c) => self.wrapped("#highlight(fill: rgb(\"#fff3a3\"))", c),
            Inline::Ins(c) => self.wrapped("#underline", c),
            Inline::Sup(c) => self.wrapped("#super", c),
            Inline::Sub(c) => self.wrapped("#sub", c),
            Inline::Link { url, children, .. } => {
                if url.contains("://") || url.starts_with("mailto:") {
                    self.wrapped(&format!("#link({})", typst_str(url)), children);
                } else {
                    self.wrapped("#", children);
                }
            }
            Inline::Image { url, alt, width, .. } => match self.image_path(url) {
                Some(v) => self.out.push_str(&format!("#box({})", image_call(&v, alt, *width))),
                None => {
                    let label = if alt.is_empty() { "圖片" } else { alt };
                    let _ = write!(self.out, "#text(fill: luma(120))[\\[{}\\]]", escape_markup(label));
                }
            },
            Inline::FootnoteRef { label, number } => match self.doc.footnote(*number) {
                Some(f) if !f.blocks.is_empty() => {
                    let blocks = f.blocks.clone();
                    let inner = self.capture(|r| r.blocks(&blocks));
                    let _ = write!(self.out, "#footnote[{inner}]");
                }
                _ => {
                    let _ = write!(self.out, "#super[{}]", escape_markup(label));
                }
            },
            Inline::Task { checked, .. } => {
                let _ = write!(self.out, "#md-check({checked}) ");
            }
            Inline::SoftBreak => self.out.push(' '),
            Inline::HardBreak => {
                if self.in_heading {
                    self.out.push(' ');
                } else {
                    self.out.push_str(" \\\n");
                }
            }
            Inline::Html(_) => {}
        }
    }
}

fn image_call(vpath: &str, alt: &str, width: Option<u32>) -> String {
    let alt = if alt.is_empty() { String::new() } else { format!(", alt: {}", typst_str(alt)) };
    // HackMD 的寬度是 CSS 像素，1px = 0.75pt
    let width = width.map(|w| format!(", width: {}pt", w as f32 * 0.75)).unwrap_or_default();
    format!("md-image({}{alt}{width})", typst_str(vpath))
}

/// 數學式：可以排版就用 Typst 數學，否則以紅字顯示原始 LaTeX，避免整份 PDF 失敗。
fn equation(tex: &str, display: bool) -> String {
    if math::check(tex, display) {
        math::typst_equation(tex, display)
    } else {
        math::fallback(tex, display)
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
        let t = conv("# 標題\n\n一段*強調*文字。\n\n```rust=\nfn main() {}\n```\n");
        assert!(t.contains("= 標題"), "{t}");
        assert!(t.contains("#emph[強調]"), "{t}");
        assert!(t.contains("#md-code(raw(\"fn main() {}\", block: true, lang: \"rust\"), start: 1)"), "{t}");
    }

    #[test]
    fn hackmd_breaks_and_cjk_soft_break() {
        assert!(conv("第一行\n第二行").contains("第一行 \\\n第二行"));
        let t = conv("---\nbreaks: false\n---\n第一行\n第二行\nabc\ndef");
        assert!(t.contains("第一行第二行 abc def"), "{t}");
    }

    #[test]
    fn hackmd_inline_and_blocks() {
        let t = conv("==亮== ++底++ x^2^\n\n:::warning 注意\n小心\n:::\n\n[TOC]\n\n## 小節");
        assert!(t.contains("#highlight(fill: rgb(\"#fff3a3\"))[亮]"), "{t}");
        assert!(t.contains("#underline[底]"), "{t}");
        assert!(t.contains("#super[2]"), "{t}");
        assert!(t.contains("#md-box(\"warning\", title: [注意])[小心]"), "{t}");
        assert!(t.contains("#md-toc(((1, [小節]),))"), "{t}");
    }

    #[test]
    fn math_and_footnotes() {
        let t = conv("$E=mc^2$ 與註[^1]\n\n$$\n\\frac{1}{2}\n$$\n\n[^1]: 註腳內容\n");
        assert!(t.contains("#math.equation(block: false"), "{t}");
        assert!(t.contains("#math.equation(block: true"), "{t}");
        assert!(t.contains("#footnote[註腳內容]"), "{t}");
    }

    #[test]
    fn missing_image_is_placeholder() {
        let t = conv("![貓](不存在.png)");
        assert!(t.contains("貓") && !t.contains("md-image"), "{t}");
    }
}
