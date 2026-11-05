//! 端到端：把 examples/範例書 輸出成 HTML / Typst / PDF。

use std::path::PathBuf;

use shosai_core::export::{self, Document};
use shosai_core::Book;

fn sample() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/範例書")
}

fn out_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("export");
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn sample_book_to_html_and_typst() {
    let book = Book::open(&sample()).unwrap();
    assert_eq!(book.chapters.len(), 4);
    let doc = book.to_document().unwrap();
    let html = export::html::render(&doc);
    assert!(html.contains("data:image/svg+xml;base64,"));
    assert!(html.contains("書齋使用手冊"));
    assert!(html.contains("class=\"alert alert-warning\""));
    assert!(html.contains("<mark>螢光筆標記</mark>"));
    #[cfg(feature = "pdf")]
    assert!(html.contains("class=\"math\""), "數學式應轉成 SVG");
    let typ = export::typst::render(&doc);
    assert_eq!(typ.images.len(), 1);
    std::fs::write(out_dir().join("sample.typ"), &typ.source).unwrap();
    std::fs::write(out_dir().join("sample.html"), html).unwrap();
}

#[cfg(feature = "pdf")]
#[test]
fn sample_book_to_pdf() {
    let book = Book::open(&sample()).unwrap();
    let doc = book.to_document().unwrap();
    let pdf = match export::pdf::render(&doc) {
        Ok(p) => p,
        Err(e) => panic!("{e}\n\n{}", export::typst::render(&doc).source),
    };
    assert!(pdf.starts_with(b"%PDF"));
    std::fs::write(out_dir().join("sample.pdf"), pdf).unwrap();
}

#[cfg(feature = "pdf")]
#[test]
fn single_note_to_pdf() {
    let path = sample().join("02-語法展示.md");
    let text = std::fs::read_to_string(&path).unwrap();
    let doc = Document::from_markdown(&path, text);
    let pdf = export::pdf::render(&doc).unwrap_or_else(|e| panic!("{e}"));
    assert!(pdf.starts_with(b"%PDF"));
}
