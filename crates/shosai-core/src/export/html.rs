//! 輸出為單一、自含（圖片內嵌）的 HTML 檔，可直接在瀏覽器閱讀或「列印 → 另存 PDF」。

use std::fmt::Write as _;
use std::path::Path;

use super::{resolve_local_image, Document};
use crate::hmd::{self, Align, Block, ContainerKind, Inline, MdDoc};

pub fn render(doc: &Document) -> String {
    let mut body = String::new();
    let mut toc = String::new();

    for (i, ch) in doc.chapters.iter().enumerate() {
        let id = format!("ch{}", i + 1);
        let _ = writeln!(toc, "<li><a href=\"#{id}\">{}</a></li>", escape(&ch.title));
        let _ = writeln!(body, "<section class=\"chapter\" id=\"{id}\">");
        let md = hmd::parse(&ch.markdown);
        if doc.is_book && !md.starts_with_h1() {
            let _ = writeln!(body, "<h1>{}</h1>", escape(&ch.title));
        }
        let prefix = if doc.chapters.len() > 1 { format!("{id}-") } else { String::new() };
        body.push_str(&doc_to_html(&md, &ch.base_dir, &prefix, doc.layout.font_size));
        let _ = writeln!(body, "</section>");
    }

    let l = &doc.layout;
    let mut out = String::new();
    let _ = write!(
        out,
        "<!doctype html>\n<html lang=\"{}\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{}</title>\n<style>\n:root {{ --font-size: {}pt; --line-height: {}; --indent: {}; }}\n@page {{ size: {}; margin: 2cm 1.8cm; }}\n{CSS}</style>\n</head>\n<body>\n",
        escape(&doc.language),
        escape(&doc.title),
        l.font_size,
        l.line_height,
        if l.indent { "2em" } else { "0" },
        l.css_paper(),
    );
    if doc.is_book && l.cover {
        let _ = write!(out, "<header class=\"cover\"><h1 class=\"book-title\">{}</h1>", escape(&doc.title));
        if !doc.subtitle.is_empty() {
            let _ = write!(out, "<p class=\"subtitle\">{}</p>", escape(&doc.subtitle));
        }
        let _ = write!(out, "<hr>");
        if !doc.author.is_empty() {
            let _ = write!(out, "<p class=\"author\">{}</p>", escape(&doc.author));
        }
        out.push_str("</header>\n");
    }
    if doc.is_book && l.toc && doc.chapters.len() > 1 {
        let _ = write!(out, "<nav class=\"toc\"><h2>目錄</h2><ol>\n{toc}</ol></nav>\n");
    }
    let _ = write!(
        out,
        "<main class=\"{}\">\n{body}</main>\n</body>\n</html>\n",
        if doc.is_book { "book" } else { "article" }
    );
    out
}

/// Markdown（HackMD 語法）→ HTML 片段；本機圖片內嵌，數學式轉成 SVG。
pub fn markdown_to_html(md: &str, base_dir: &Path) -> String {
    doc_to_html(&hmd::parse(md), base_dir, "", 10.5)
}

/// `prefix`：標題錨點前綴（整本書時避免各章重複）；`font_pt`：數學式字級。
pub fn doc_to_html(doc: &MdDoc, base_dir: &Path, prefix: &str, font_pt: f32) -> String {
    let mut h = Html { doc, base_dir, prefix, font_pt, out: String::new() };
    h.blocks(&doc.blocks);
    if !doc.footnotes.is_empty() {
        h.out.push_str("<section class=\"footnotes\"><hr><ol>\n");
        for f in &doc.footnotes {
            let _ = write!(h.out, "<li id=\"{prefix}fn-{}\">", f.number);
            h.blocks(&f.blocks);
            let _ = writeln!(h.out, " <a href=\"#{prefix}fnref-{}\" class=\"fn-back\">↩</a></li>", f.number);
        }
        h.out.push_str("</ol></section>\n");
    }
    h.out
}

struct Html<'a> {
    doc: &'a MdDoc,
    base_dir: &'a Path,
    prefix: &'a str,
    font_pt: f32,
    out: String,
}

impl Html<'_> {
    fn blocks(&mut self, blocks: &[Block]) {
        for b in blocks {
            self.block(b);
        }
    }

    fn block(&mut self, b: &Block) {
        match b {
            Block::Paragraph(inl) => {
                let mut meaningful = inl.iter().filter(|i| !matches!(i, Inline::Text(t) if t.trim().is_empty()));
                if let (Some(img @ Inline::Image { alt, .. }), None) = (meaningful.next(), meaningful.next()) {
                    self.out.push_str("<figure>");
                    self.inline(img);
                    if !alt.is_empty() {
                        let _ = write!(self.out, "<figcaption>{}</figcaption>", escape(alt));
                    }
                    self.out.push_str("</figure>\n");
                } else {
                    self.out.push_str("<p>");
                    self.inlines(inl);
                    self.out.push_str("</p>\n");
                }
            }
            Block::Heading { level, content, id } => {
                let _ = write!(self.out, "<h{level} id=\"{}{}\">", self.prefix, escape(id));
                self.inlines(content);
                let _ = writeln!(self.out, "</h{level}>");
            }
            Block::Quote(bs) => {
                self.out.push_str("<blockquote>\n");
                self.blocks(bs);
                self.out.push_str("</blockquote>\n");
            }
            Block::Code { lang, code, line_start } => {
                let class =
                    if lang.is_empty() { String::new() } else { format!(" class=\"language-{}\"", escape(lang)) };
                match line_start {
                    Some(start) => {
                        let _ = write!(self.out, "<pre class=\"code numbered\"><code{class}>");
                        for (i, line) in code.split('\n').enumerate() {
                            let _ = writeln!(self.out, "<span class=\"ln\">{}</span>{}", start + i, escape(line));
                        }
                        self.out.push_str("</code></pre>\n");
                    }
                    None => {
                        let _ = writeln!(self.out, "<pre class=\"code\"><code{class}>{}</code></pre>", escape(code));
                    }
                }
            }
            Block::Math(tex) => {
                let _ = writeln!(self.out, "<div class=\"math-block\">{}</div>", self.math(tex, true));
            }
            Block::List { start, items } => {
                let tag = if start.is_some() { "ol" } else { "ul" };
                match start {
                    Some(n) if *n != 1 => {
                        let _ = writeln!(self.out, "<ol start=\"{n}\">");
                    }
                    _ => {
                        let _ = writeln!(self.out, "<{tag}>");
                    }
                }
                for item in items {
                    let task = matches!(item.first(), Some(Block::Paragraph(i)) if matches!(i.first(), Some(Inline::Task { .. })));
                    self.out.push_str(if task { "<li class=\"task\">" } else { "<li>" });
                    // 只有一段文字的項目不包 <p>
                    match item.as_slice() {
                        [Block::Paragraph(inl)] => self.inlines(inl),
                        _ => self.blocks(item),
                    }
                    self.out.push_str("</li>\n");
                }
                let _ = writeln!(self.out, "</{tag}>");
            }
            Block::Table { aligns, header, rows } => {
                let style = |i: usize| match aligns.get(i) {
                    Some(Align::Center) => " style=\"text-align:center\"",
                    Some(Align::Right) => " style=\"text-align:right\"",
                    Some(Align::Left) => " style=\"text-align:left\"",
                    _ => "",
                };
                self.out.push_str("<table>\n<thead><tr>");
                for (i, cell) in header.iter().enumerate() {
                    let _ = write!(self.out, "<th{}>", style(i));
                    self.inlines(cell);
                    self.out.push_str("</th>");
                }
                self.out.push_str("</tr></thead>\n<tbody>\n");
                for row in rows {
                    self.out.push_str("<tr>");
                    for (i, cell) in row.iter().enumerate() {
                        let _ = write!(self.out, "<td{}>", style(i));
                        self.inlines(cell);
                        self.out.push_str("</td>");
                    }
                    self.out.push_str("</tr>\n");
                }
                self.out.push_str("</tbody></table>\n");
            }
            Block::Rule => self.out.push_str("<hr>\n"),
            Block::Container { kind: ContainerKind::Spoiler, title, open, blocks } => {
                let title = if title.is_empty() { "詳細內容".to_string() } else { escape(title) };
                let _ = writeln!(self.out, "<details{}><summary>{title}</summary>", if *open { " open" } else { "" });
                self.blocks(blocks);
                self.out.push_str("</details>\n");
            }
            Block::Container { kind, title, blocks, .. } => {
                let _ = writeln!(self.out, "<div class=\"alert alert-{}\">", kind.name());
                if !title.is_empty() {
                    let _ = writeln!(self.out, "<p class=\"alert-title\">{}</p>", escape(title));
                }
                self.blocks(blocks);
                self.out.push_str("</div>\n");
            }
            Block::DefList(items) => {
                self.out.push_str("<dl>\n");
                for item in items {
                    self.out.push_str("<dt>");
                    self.inlines(&item.term);
                    self.out.push_str("</dt>\n");
                    for d in &item.defs {
                        self.out.push_str("<dd>");
                        match d.as_slice() {
                            [Block::Paragraph(inl)] => self.inlines(inl),
                            _ => self.blocks(d),
                        }
                        self.out.push_str("</dd>\n");
                    }
                }
                self.out.push_str("</dl>\n");
            }
            Block::Toc => {
                let headings = self.doc.headings();
                let min = headings.iter().map(|h| h.level).min().unwrap_or(1);
                self.out.push_str("<nav class=\"md-toc\"><ul>\n");
                for h in headings {
                    let _ = writeln!(
                        self.out,
                        "<li class=\"lv{}\"><a href=\"#{}{}\">{}</a></li>",
                        h.level - min + 1,
                        self.prefix,
                        escape(&h.id),
                        escape(&h.text)
                    );
                }
                self.out.push_str("</ul></nav>\n");
            }
            Block::PageBreak => self.out.push_str("<div class=\"pagebreak\"></div>\n"),
            Block::Html(h) => self.out.push_str(h),
        }
    }

    fn inlines(&mut self, inl: &[Inline]) {
        for i in inl {
            self.inline(i);
        }
    }

    fn wrap(&mut self, tag: &str, children: &[Inline]) {
        let _ = write!(self.out, "<{tag}>");
        self.inlines(children);
        let _ = write!(self.out, "</{tag}>");
    }

    fn inline(&mut self, i: &Inline) {
        match i {
            Inline::Text(t) => self.out.push_str(&escape(t)),
            Inline::Code(t) => {
                let _ = write!(self.out, "<code>{}</code>", escape(t));
            }
            Inline::Math { tex, display } => {
                let m = self.math(tex, *display);
                if *display {
                    let _ = write!(self.out, "<span class=\"math-block\">{m}</span>");
                } else {
                    self.out.push_str(&m);
                }
            }
            Inline::Emph(c) => self.wrap("em", c),
            Inline::Strong(c) => self.wrap("strong", c),
            Inline::Strike(c) => self.wrap("del", c),
            Inline::Mark(c) => self.wrap("mark", c),
            Inline::Ins(c) => self.wrap("ins", c),
            Inline::Sup(c) => self.wrap("sup", c),
            Inline::Sub(c) => self.wrap("sub", c),
            Inline::Link { url, title, children } => {
                let href = match url.strip_prefix('#') {
                    Some(anchor) => format!("#{}{}", self.prefix, anchor),
                    None => url.clone(),
                };
                let title = if title.is_empty() { String::new() } else { format!(" title=\"{}\"", escape(title)) };
                let _ = write!(self.out, "<a href=\"{}\"{title}>", escape(&href));
                self.inlines(children);
                self.out.push_str("</a>");
            }
            Inline::Image { url, alt, title, width, height } => {
                let src =
                    resolve_local_image(self.base_dir, url).and_then(|p| data_uri(&p)).unwrap_or_else(|| url.clone());
                let mut attrs = String::new();
                if let Some(w) = width {
                    let _ = write!(attrs, " width=\"{w}\"");
                }
                if let Some(h) = height {
                    let _ = write!(attrs, " height=\"{h}\"");
                }
                if !title.is_empty() {
                    let _ = write!(attrs, " title=\"{}\"", escape(title));
                }
                let _ = write!(self.out, "<img src=\"{}\" alt=\"{}\"{attrs}>", escape(&src), escape(alt));
            }
            Inline::FootnoteRef { number, .. } => {
                let p = self.prefix;
                let _ = write!(
                    self.out,
                    "<sup class=\"fn\" id=\"{p}fnref-{number}\"><a href=\"#{p}fn-{number}\">[{number}]</a></sup>"
                );
            }
            Inline::Task { checked, .. } => {
                let _ =
                    write!(self.out, "<input type=\"checkbox\" disabled{}> ", if *checked { " checked" } else { "" });
            }
            Inline::SoftBreak => self.out.push('\n'),
            Inline::HardBreak => self.out.push_str("<br>\n"),
            Inline::Html(h) => self.out.push_str(h),
        }
    }

    /// 數學式 → 內嵌 SVG 圖片（`pdf` feature）；否則顯示原始 LaTeX。
    fn math(&self, tex: &str, display: bool) -> String {
        #[cfg(feature = "pdf")]
        if let Ok(svg) = crate::math::render_svg(tex, display, self.font_pt, "2b2620") {
            return format!(
                "<img class=\"math\" alt=\"{}\" src=\"data:image/svg+xml;base64,{}\">",
                escape(tex),
                base64(svg.as_bytes())
            );
        }
        let _ = self.font_pt;
        let d = if display { "$$" } else { "$" };
        format!("<code class=\"math-src\">{d}{}{d}</code>", escape(tex))
    }
}

fn data_uri(path: &Path) -> Option<String> {
    let mime = match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => return None,
    };
    let bytes = std::fs::read(path).ok()?;
    Some(format!("data:{mime};base64,{}", base64(&bytes)))
}

pub(crate) fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const CSS: &str = r#"
* { box-sizing: border-box; }
html { background: #ece6da; }
body { margin: 0; color: #2b2620; font-family: "Libertinus Serif", "Noto Serif CJK TC", "Source Han Serif TC", "Songti TC", "PMingLiU", serif; font-size: var(--font-size); line-height: var(--line-height); }
.cover, nav.toc, main { max-width: 42em; margin: 2rem auto; background: #fbf8f1; padding: 3.5em 4em; box-shadow: 0 1px 3px rgba(0,0,0,.12), 0 8px 24px rgba(0,0,0,.08); }
.cover { text-align: center; padding-top: 30vh; min-height: 90vh; }
.cover .book-title { font-size: 2.4em; margin: 0; }
.cover .subtitle { font-size: 1.2em; color: #6b6255; text-indent: 0; }
.cover hr { width: 25%; margin: 2em auto; }
.cover .author { font-size: 1.1em; margin-top: 3em; text-indent: 0; }
nav.toc ol { padding-left: 1.2em; }
nav.toc a { color: inherit; text-decoration: none; }
nav.toc a:hover { text-decoration: underline; }
.book p { text-indent: var(--indent); }
p { margin: 0 0 .6em; text-align: justify; }
li p, blockquote p, td p { text-indent: 0; }
.book section.chapter + section.chapter { margin-top: 5em; }
.book section.chapter > h1:first-child { margin-top: 2em; font-size: 2em; padding-bottom: .4em; border-bottom: 1px solid #c9bfae; }
h1, h2, h3, h4 { line-height: 1.35; margin: 1.6em 0 .7em; }
a { color: #2a5db0; }
img { max-width: 100%; display: block; margin: 1em auto; }
blockquote { margin: 1em 0; padding: .2em 1em; border-left: 3px solid #c9bfae; color: #5a5146; }
code { font-family: "DejaVu Sans Mono", "Sarasa Mono TC", monospace; font-size: .9em; background: #f0ebe0; padding: .1em .3em; border-radius: 3px; }
pre { background: #f3efe6; padding: .9em 1em; border-radius: 4px; overflow-x: auto; line-height: 1.5; }
pre code { background: none; padding: 0; }
table { border-collapse: collapse; margin: 1em 0; }
th, td { border: 1px solid #c9bfae; padding: .35em .7em; }
hr { border: 0; border-top: 1px solid #c9bfae; width: 40%; margin: 2em auto; }
mark { background: #fff3a3; padding: 0 .1em; }
ins { text-decoration: underline; text-decoration-color: #b8a98f; }
img.math { display: inline-block; vertical-align: middle; margin: 0; max-width: none; }
.math-block { display: block; text-align: center; margin: .8em 0; }
.math-block img.math { display: inline-block; }
code.math-src { color: #a94442; }
figure { margin: 1.2em 0; text-align: center; }
figcaption { font-size: .85em; color: #6b6255; }
.alert { border: 1px solid; border-radius: 4px; padding: .7em 1em; margin: 1em 0; }
.alert p { text-indent: 0; }
.alert > :last-child { margin-bottom: 0; }
.alert-title { font-weight: bold; }
.alert-info { background: #d9edf7; border-color: #bce8f1; color: #31708f; }
.alert-success { background: #dff0d8; border-color: #d6e9c6; color: #3c763d; }
.alert-warning { background: #fcf8e3; border-color: #faebcc; color: #8a6d3b; }
.alert-danger { background: #f2dede; border-color: #ebccd1; color: #a94442; }
details { border: 1px solid #c9bfae; border-radius: 4px; padding: .5em 1em; margin: 1em 0; }
details summary { cursor: pointer; font-weight: bold; }
details p { text-indent: 0; }
li.task { list-style: none; margin-left: -1.3em; }
dl dt { font-weight: bold; margin-top: .6em; }
dl dd { margin-left: 2em; }
nav.md-toc ul { list-style: none; padding-left: 0; }
nav.md-toc a { color: inherit; }
nav.md-toc .lv2 { padding-left: 1.2em; } nav.md-toc .lv3 { padding-left: 2.4em; } nav.md-toc .lv4 { padding-left: 3.6em; }
pre.numbered .ln { display: inline-block; width: 2.5em; margin-right: 1em; text-align: right; color: #aaa093; user-select: none; }
.footnotes { font-size: .9em; }
.footnotes p { text-indent: 0; display: inline; }
sup.fn a { text-decoration: none; }
.pagebreak { break-after: page; }
@media (max-width: 720px) { .cover, nav.toc, main { margin: 0; padding: 1.5em 1.2em; box-shadow: none; } }
@media print {
  html, body { background: none; }
  .cover, nav.toc, main { box-shadow: none; margin: 0; padding: 0; max-width: none; background: none; }
  .cover { padding-top: 30vh; break-after: page; }
  nav.toc { break-after: page; }
  .book section.chapter { break-before: page; }
  .book section.chapter + section.chapter { margin-top: 0; }
  a { color: inherit; text-decoration: none; }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }
}
