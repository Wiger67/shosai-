//! 輸出為單一、自含（圖片內嵌）的 HTML 檔，可直接在瀏覽器閱讀或「列印 → 另存 PDF」。

use std::fmt::Write as _;
use std::path::Path;

use pulldown_cmark::{html, CowStr, Event, Parser, Tag};

use super::{resolve_local_image, Document};
use crate::markdown::options;

pub fn render(doc: &Document) -> String {
    let mut body = String::new();
    let mut toc = String::new();

    for (i, ch) in doc.chapters.iter().enumerate() {
        let id = format!("ch{}", i + 1);
        let _ = writeln!(toc, "<li><a href=\"#{id}\">{}</a></li>", escape(&ch.title));
        let _ = writeln!(body, "<section class=\"chapter\" id=\"{id}\">");
        let starts_with_h1 = crate::markdown::outline(&ch.markdown).first().is_some_and(|h| h.level == 1);
        if doc.is_book && !starts_with_h1 {
            let _ = writeln!(body, "<h1>{}</h1>", escape(&ch.title));
        }
        body.push_str(&markdown_to_html(&ch.markdown, &ch.base_dir));
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

/// Markdown → HTML 片段；本機圖片轉成 data URI 內嵌。
pub fn markdown_to_html(md: &str, base_dir: &Path) -> String {
    let events = Parser::new_ext(md, options()).map(|ev| match ev {
        Event::Start(Tag::Image { link_type, dest_url, title, id }) => {
            let url = match resolve_local_image(base_dir, &dest_url).and_then(|p| data_uri(&p)) {
                Some(uri) => CowStr::from(uri),
                None => dest_url,
            };
            Event::Start(Tag::Image { link_type, dest_url: url, title, id })
        }
        other => other,
    });
    let mut out = String::new();
    html::push_html(&mut out, events);
    out
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
