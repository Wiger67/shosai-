//! 側欄的檔案樹：自繪的列（圖示、縮排、整列 hover / 選取），取代 egui 預設的折疊標題。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use eframe::egui::{self, pos2, vec2, Rect, RichText, Sense, TextWrapMode, Ui, WidgetText};
use shosai_core::{Book, Entry, EntryKind};

use crate::app::{Action, Dialog, Format};
use crate::icons::{self, Icon};
use crate::theme::Palette;

const ROW_H: f32 = 26.0;
const INDENT: f32 = 14.0;

pub struct TreeCtx<'a> {
    pub current: Option<&'a Path>,
    pub books: &'a HashMap<PathBuf, Book>,
    pub actions: &'a mut Vec<Action>,
    pub expanded: &'a mut HashSet<PathBuf>,
    pub pal: Palette,
}

/// 一列：縮排、展開箭頭（資料夾才有）、圖示、文字。
fn row(
    ui: &mut Ui,
    depth: usize,
    chevron: Option<bool>,
    icon: Option<Icon>,
    text: impl Into<WidgetText>,
    selected: bool,
    pal: &Palette,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 6.0, pal.selected);
    } else if resp.hovered() {
        p.rect_filled(rect, 6.0, pal.hover);
    }
    let fg = if selected { pal.ink } else { ui.visuals().text_color() };
    let mut x = rect.left() + 6.0 + depth as f32 * INDENT;
    let cy = rect.center().y;
    if let Some(open) = chevron {
        let r = Rect::from_center_size(pos2(x + 6.0, cy), vec2(11.0, 11.0));
        icons::paint(p, r, if open { Icon::ChevronDown } else { Icon::ChevronRight }, pal.weak);
    }
    x += 14.0;
    if let Some(icon) = icon {
        let r = Rect::from_center_size(pos2(x + 8.0, cy), vec2(15.0, 15.0));
        icons::paint(p, r, icon, if selected { pal.accent } else { pal.weak });
        x += 22.0;
    }
    let max_w = (rect.right() - x - 6.0).max(10.0);
    let galley = text.into().into_galley(ui, Some(TextWrapMode::Truncate), max_w, egui::TextStyle::Body);
    let pos = pos2(x, cy - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, fg);
    resp
}

fn toggle(expanded: &mut HashSet<PathBuf>, path: &Path) {
    if !expanded.remove(path) {
        expanded.insert(path.to_path_buf());
    }
}

/// 一般資料夾內容。
pub fn entries(ui: &mut Ui, list: &[Entry], depth: usize, cx: &mut TreeCtx) {
    for e in list {
        match e.kind {
            EntryKind::Markdown | EntryKind::Pdf => file(ui, e, depth, cx),
            EntryKind::Dir => {
                let open = cx.expanded.contains(&e.path);
                let resp = row(ui, depth, Some(open), Some(Icon::Folder), e.name.as_str(), false, &cx.pal);
                if resp.clicked() {
                    toggle(cx.expanded, &e.path);
                }
                resp.context_menu(|ui| {
                    new_menu(ui, &e.path, cx.actions);
                    ui.separator();
                    common_menu(ui, &e.path, cx.actions);
                });
                if open {
                    if e.children.is_empty() {
                        row(ui, depth + 1, None, None, RichText::new("（空資料夾）").weak(), false, &cx.pal);
                    }
                    entries(ui, &e.children, depth + 1, cx);
                }
            }
            EntryKind::Book => {
                let Some(book) = cx.books.get(&e.path) else { continue };
                let open = cx.expanded.contains(&e.path);
                let resp = row(
                    ui,
                    depth,
                    Some(open),
                    Some(Icon::Book),
                    RichText::new(&book.meta.title).strong(),
                    false,
                    &cx.pal,
                )
                .on_hover_text(e.path.display().to_string());
                if resp.clicked() {
                    toggle(cx.expanded, &e.path);
                }
                resp.context_menu(|ui| {
                    book_menu(ui, book, cx.actions);
                    ui.separator();
                    common_menu(ui, &e.path, cx.actions);
                });
                if open {
                    book_contents(ui, book, e, depth + 1, cx);
                }
            }
        }
    }
}

/// 書本內容：依章節順序列出，再列出書中的 PDF（例如輸出的成品）。
pub fn book_contents(ui: &mut Ui, book: &Book, entry: &Entry, depth: usize, cx: &mut TreeCtx) {
    let n = book.chapters.len();
    let weak = cx.pal.weak;
    for (i, ch) in book.chapters.iter().enumerate() {
        let selected = cx.current == Some(ch.path.as_path());
        let mut job = egui::text::LayoutJob::default();
        let body = ui.style().text_styles[&egui::TextStyle::Body].clone();
        job.append(&format!("{:>2}  ", i + 1), 0.0, egui::TextFormat::simple(body.clone(), weak));
        job.append(&ch.title, 0.0, egui::TextFormat::simple(body, ui.visuals().text_color()));
        let resp = row(ui, depth, None, None, job, selected, &cx.pal).on_hover_text(&ch.rel);
        if resp.clicked() && !selected {
            cx.actions.push(Action::Open(ch.path.clone()));
        }
        resp.context_menu(|ui| {
            if ui.add_enabled(i > 0, egui::Button::new("上移")).clicked() {
                cx.actions.push(Action::MoveChapter { book: book.root.clone(), index: i, delta: -1 });
                ui.close();
            }
            if ui.add_enabled(i + 1 < n, egui::Button::new("下移")).clicked() {
                cx.actions.push(Action::MoveChapter { book: book.root.clone(), index: i, delta: 1 });
                ui.close();
            }
            ui.separator();
            common_menu(ui, &ch.path, cx.actions);
        });
    }
    let mut pdfs = Vec::new();
    collect_pdfs(&entry.children, &mut pdfs);
    for pdf in pdfs {
        file(ui, pdf, depth, cx);
    }
    let resp = row(ui, depth, None, Some(Icon::Plus), RichText::new("新增章節").color(weak), false, &cx.pal);
    if resp.clicked() {
        cx.actions.push(Action::Dialog(Dialog::NewChapter { book: book.root.clone(), title: String::new() }));
    }
}

fn collect_pdfs<'a>(list: &'a [Entry], out: &mut Vec<&'a Entry>) {
    for e in list {
        match e.kind {
            EntryKind::Pdf => out.push(e),
            EntryKind::Dir => collect_pdfs(&e.children, out),
            _ => {}
        }
    }
}

/// 單一檔案列（Markdown 或 PDF）。
pub fn file(ui: &mut Ui, e: &Entry, depth: usize, cx: &mut TreeCtx) {
    file_labeled(ui, &e.path, &e.name, e.kind == EntryKind::Pdf, depth, cx);
}

pub fn file_labeled(ui: &mut Ui, path: &Path, name: &str, is_pdf: bool, depth: usize, cx: &mut TreeCtx) {
    let selected = cx.current == Some(path);
    let label = if is_pdf { name.to_string() } else { name.trim_end_matches(".md").to_string() };
    let icon = if is_pdf { Icon::Pdf } else { Icon::File };
    let resp = row(ui, depth, None, Some(icon), label, selected, &cx.pal);
    if resp.clicked() && !selected {
        cx.actions.push(Action::Open(path.to_path_buf()));
    }
    resp.context_menu(|ui| {
        if !is_pdf {
            if ui.button("輸出 PDF…").clicked() {
                cx.actions.push(Action::Export { source: path.to_path_buf(), format: Format::Pdf });
                ui.close();
            }
            ui.separator();
        } else {
            if ui.button("用預設程式開啟").clicked() {
                cx.actions.push(Action::Reveal(path.to_path_buf()));
                ui.close();
            }
            ui.separator();
        }
        common_menu(ui, path, cx.actions);
    });
}

pub fn new_menu(ui: &mut Ui, dir: &Path, actions: &mut Vec<Action>) {
    if ui.button("新增筆記").clicked() {
        actions.push(Action::Dialog(Dialog::NewNote { dir: dir.to_path_buf(), name: String::new() }));
        ui.close();
    }
    if ui.button("新增資料夾").clicked() {
        actions.push(Action::Dialog(Dialog::NewFolder { dir: dir.to_path_buf(), name: String::new() }));
        ui.close();
    }
    if ui.button("建立新書").clicked() {
        actions.push(Action::Dialog(Dialog::NewBook {
            dir: dir.to_path_buf(),
            title: String::new(),
            author: String::new(),
        }));
        ui.close();
    }
}

pub fn book_menu(ui: &mut Ui, book: &Book, actions: &mut Vec<Action>) {
    if ui.button("新增章節").clicked() {
        actions.push(Action::Dialog(Dialog::NewChapter { book: book.root.clone(), title: String::new() }));
        ui.close();
    }
    if ui.button("書本設定…").clicked() {
        actions
            .push(Action::Dialog(Dialog::BookSettings { root: book.root.clone(), meta: Box::new(book.meta.clone()) }));
        ui.close();
    }
    ui.separator();
    if ui.button("輸出 PDF…").clicked() {
        actions.push(Action::Export { source: book.root.clone(), format: Format::Pdf });
        ui.close();
    }
    if ui.button("輸出 HTML…").clicked() {
        actions.push(Action::Export { source: book.root.clone(), format: Format::Html });
        ui.close();
    }
}

pub fn common_menu(ui: &mut Ui, path: &Path, actions: &mut Vec<Action>) {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if ui.button("重新命名").clicked() {
        actions.push(Action::Dialog(Dialog::Rename { path: path.to_path_buf(), name }));
        ui.close();
    }
    if ui.button("在檔案管理員中顯示").clicked() {
        let target = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        actions.push(Action::Reveal(target.to_path_buf()));
        ui.close();
    }
    ui.separator();
    if ui.button(RichText::new("移到回收桶").color(ui.visuals().error_fg_color)).clicked() {
        actions.push(Action::Dialog(Dialog::Trash { path: path.to_path_buf() }));
        ui.close();
    }
}
