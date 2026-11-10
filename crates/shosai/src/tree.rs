//! 側欄的檔案樹：自繪的列。
//!
//! 為了讓結構一眼就看得懂：
//! - 不同類型用不同顏色的圖示（資料夾、書本、Markdown、PDF）
//! - 展開的資料夾畫出縮排導引線
//! - 資料夾與書本右側顯示裡面有幾個檔案
//! - 書本章節用編號徽章標示順序
//! - 拖曳列可以調整順序（自訂排序）或移到別的資料夾
//! - 鍵盤操作：↑ ↓ 移動、← → 收合／展開、Enter 開啟、F2 重新命名、Delete 移到回收桶

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use eframe::egui::{self, pos2, vec2, Color32, Rect, RichText, Sense, Stroke, TextWrapMode, Ui, WidgetText};
use shosai_core::{Book, Entry, EntryKind};

use crate::app::{Action, Dialog, Format};
use crate::icons::{self, Icon};
use crate::theme::Palette;

const ROW_H: f32 = 28.0;
const INDENT: f32 = 16.0;

/// 拖曳中的項目（檔案或資料夾路徑）。
pub struct DragItem(pub PathBuf);

/// 放下的位置：目標的前面、後面，或放進目標資料夾裡。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Before,
    After,
    Into,
}

/// 檔案樹的鍵盤操作狀態。
#[derive(Default)]
pub struct TreeNav {
    /// 方向鍵目前是不是給檔案樹用（點了資料夾，或從搜尋框按 ↓ 進來）
    pub active: bool,
    /// 鍵盤游標所在的列
    pub cursor: Option<PathBuf>,
    /// 這一幀畫出來的列，由上到下
    pub rows: Vec<NavRow>,
}

pub struct NavRow {
    pub path: PathBuf,
    depth: usize,
    /// 資料夾或書本：目前是否展開；檔案為 None
    open: Option<bool>,
    rect: Rect,
}

impl TreeNav {
    fn has_cursor(&self, path: &Path) -> bool {
        self.active && self.cursor.as_deref() == Some(path)
    }

    fn record(&mut self, path: &Path, depth: usize, open: Option<bool>, resp: &egui::Response) {
        self.rows.push(NavRow { path: path.to_path_buf(), depth, open, rect: resp.rect });
        if resp.clicked() {
            // 點資料夾：之後用方向鍵繼續瀏覽；點檔案：鍵盤交還給文件（捲動、翻頁）
            self.active = open.is_some();
            self.cursor = Some(path.to_path_buf());
        }
    }

    /// 第一個檔案列（搜尋框按 Enter 時開啟）。
    pub fn first_file(&self) -> Option<&Path> {
        self.rows.iter().find(|r| r.open.is_none()).map(|r| r.path.as_path())
    }
}

/// 處理檔案樹的鍵盤操作。在檔案樹的 ScrollArea 裡、畫完所有列之後呼叫。
///
/// 用掉的按鍵會從輸入中移除，後面的閱讀畫面就不會再收到同一個按鍵。
pub fn keyboard(ui: &Ui, nav: &mut TreeNav, expanded: &mut HashSet<PathBuf>, actions: &mut Vec<Action>) {
    use egui::Key;
    if !nav.active || nav.rows.is_empty() {
        return;
    }
    let pressed = |key: Key| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key));
    let last = nav.rows.len() - 1;
    let idx = nav.cursor.as_ref().and_then(|c| nav.rows.iter().position(|r| &r.path == c));
    let mut target = None;
    if pressed(Key::ArrowDown) {
        target = Some(idx.map_or(0, |i| (i + 1).min(last)));
    }
    if pressed(Key::ArrowUp) {
        target = Some(idx.map_or(0, |i| i.saturating_sub(1)));
    }
    if pressed(Key::Home) {
        target = Some(0);
    }
    if pressed(Key::End) {
        target = Some(last);
    }
    let (right, left, enter, rename, delete) = (
        pressed(Key::ArrowRight),
        pressed(Key::ArrowLeft),
        pressed(Key::Enter),
        pressed(Key::F2),
        pressed(Key::Delete),
    );
    if pressed(Key::Escape) {
        nav.active = false;
    }
    if let Some(i) = idx {
        let row = &nav.rows[i];
        if right {
            match row.open {
                Some(false) => {
                    expanded.insert(row.path.clone());
                }
                Some(true) => target = Some((i + 1).min(last)),
                None => {}
            }
        }
        if left {
            if row.open == Some(true) {
                expanded.remove(&row.path);
            } else if let Some(parent) = nav.rows[..i].iter().rposition(|r| r.depth < row.depth) {
                target = Some(parent);
            }
        }
        if enter {
            match row.open {
                Some(_) => toggle(expanded, &row.path),
                None => actions.push(Action::Open(row.path.clone())),
            }
        }
        if rename {
            let name = row.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            actions.push(Action::Dialog(Dialog::Rename { path: row.path.clone(), name }));
        }
        if delete {
            actions.push(Action::Dialog(Dialog::Trash { path: row.path.clone() }));
        }
    }
    if let Some(t) = target {
        let row = &nav.rows[t];
        nav.cursor = Some(row.path.clone());
        // 上下各多留一列，看得到接下來是什麼
        ui.scroll_to_rect(row.rect.expand2(vec2(0.0, ROW_H)), None);
    }
}

pub struct TreeCtx<'a> {
    pub current: Option<&'a Path>,
    pub nav: &'a mut TreeNav,
    pub books: &'a HashMap<PathBuf, Book>,
    pub actions: &'a mut Vec<Action>,
    pub expanded: &'a mut HashSet<PathBuf>,
    pub pal: Palette,
    pub show_pdfs: bool,
    pub show_counts: bool,
    pub lib_root: &'a Path,
}

struct RowSpec<'a> {
    depth: usize,
    chevron: Option<bool>,
    icon: Option<(Icon, Color32)>,
    /// 章節編號徽章
    badge: Option<String>,
    text: WidgetText,
    /// 右側的淡色資訊（檔案數）
    trailing: Option<String>,
    selected: bool,
    /// 目前檔案所在的資料夾：箭頭用強調色
    on_path: bool,
    hover: Option<&'a str>,
    /// 可以拖曳
    draggable: bool,
    /// 鍵盤游標在這一列
    cursor: bool,
}

fn row(ui: &mut Ui, spec: RowSpec, pal: &Palette) -> egui::Response {
    let width = ui.available_width();
    let sense = if spec.draggable { Sense::click_and_drag() } else { Sense::click() };
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, ROW_H), sense);
    if let Some(h) = spec.hover {
        resp = resp.on_hover_text(h);
    }
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = ui.painter();
    if spec.selected {
        p.rect_filled(rect, 6.0, pal.selected);
        // 左側強調條
        let bar = Rect::from_min_size(rect.min + vec2(0.0, 6.0), vec2(3.0, rect.height() - 12.0));
        p.rect_filled(bar, 1.5, pal.accent);
    } else if resp.is_pointer_button_down_on() {
        // 按下的瞬間就有回饋
        p.rect_filled(rect, 6.0, pal.selected);
    } else if resp.hovered() {
        p.rect_filled(rect, 6.0, pal.hover);
    }
    if spec.cursor {
        p.rect_stroke(rect, 6.0, Stroke::new(1.5, pal.accent), egui::StrokeKind::Inside);
    }

    let mut x = rect.left() + 8.0 + spec.depth as f32 * INDENT;
    let cy = rect.center().y;
    if let Some(open) = spec.chevron {
        let r = Rect::from_center_size(pos2(x + 5.0, cy), vec2(10.0, 10.0));
        let color = if spec.on_path { pal.accent } else { pal.weak };
        icons::paint(p, r, if open { Icon::ChevronDown } else { Icon::ChevronRight }, color);
    }
    x += 14.0;
    if let Some((icon, color)) = spec.icon {
        let r = Rect::from_center_size(pos2(x + 8.0, cy), vec2(16.0, 16.0));
        icons::paint(p, r, icon, color);
        x += 23.0;
    }
    if let Some(badge) = &spec.badge {
        let font = egui::FontId::proportional(11.0);
        let color = if spec.selected { pal.accent } else { pal.weak };
        let galley = p.layout_no_wrap(badge.clone(), font, color);
        let w = galley.size().x.max(10.0) + 10.0;
        let r = Rect::from_center_size(pos2(x + w / 2.0, cy), vec2(w, 18.0));
        p.rect_stroke(
            r,
            5.0,
            Stroke::new(1.0, if spec.selected { pal.accent } else { pal.guide }),
            egui::StrokeKind::Inside,
        );
        p.galley(r.center() - galley.size() / 2.0, galley, color);
        x += w + 8.0;
    }

    let mut right = rect.right() - 8.0;
    if let Some(t) = &spec.trailing {
        let galley = p.layout_no_wrap(t.clone(), egui::FontId::proportional(11.5), pal.weak);
        right -= galley.size().x;
        p.galley(pos2(right, cy - galley.size().y / 2.0), galley, pal.weak);
        right -= 8.0;
    }

    let fg = if spec.selected { pal.ink } else { ui.visuals().text_color() };
    let max_w = (right - x).max(10.0);
    let galley = spec.text.into_galley(ui, Some(TextWrapMode::Truncate), max_w, egui::TextStyle::Body);
    ui.painter().galley(pos2(x, cy - galley.size().y / 2.0), galley, fg);
    resp
}

/// 拖放：這一列可以被拖走，也可以接收別的列。`into` 表示可以放進去（資料夾、書本）。
fn dnd(ui: &Ui, resp: &egui::Response, depth: usize, path: &Path, into: bool, cx: &mut TreeCtx) {
    resp.dnd_set_drag_payload(DragItem(path.to_path_buf()));
    let Some(item) = resp.dnd_hover_payload::<DragItem>() else { return };
    if item.0 == path || path.starts_with(&item.0) {
        return;
    }
    let Some(pos) = ui.ctx().pointer_interact_pos() else { return };
    let r = resp.rect;
    let t = (pos.y - r.top()) / r.height().max(1.0);
    let place = match (into, t) {
        (true, t) if t < 0.25 => Place::Before,
        (true, t) if t > 0.75 => Place::After,
        (true, _) => Place::Into,
        (false, t) if t < 0.5 => Place::Before,
        (false, _) => Place::After,
    };
    let left = r.left() + 8.0 + depth as f32 * INDENT;
    let stroke = Stroke::new(2.0, cx.pal.accent);
    let p = ui.painter();
    match place {
        Place::Into => {
            // 畫在文字上面，所以只用很淡的底色
            p.rect_filled(r, 6.0, cx.pal.accent.gamma_multiply(0.10));
            p.rect_stroke(r.shrink(1.0), 6.0, stroke, egui::StrokeKind::Inside);
        }
        Place::Before | Place::After => {
            let y = if place == Place::Before { r.top() } else { r.bottom() };
            p.hline(left..=r.right() - 4.0, y, stroke);
            p.circle_stroke(pos2(left, y), 3.0, stroke);
        }
    }
    if resp.dnd_release_payload::<DragItem>().is_some() {
        cx.actions.push(Action::Drop { src: item.0.clone(), target: path.to_path_buf(), place });
    }
}

/// 縮排導引線：從 `top` 畫到 `bottom`，位置對齊上層的展開箭頭。
fn guide(ui: &Ui, left: f32, depth: usize, top: f32, bottom: f32, pal: &Palette) {
    if bottom - top < 2.0 {
        return;
    }
    let x = left + 8.0 + depth as f32 * INDENT + 5.0;
    ui.painter().vline(x, top + 2.0..=bottom - 4.0, Stroke::new(1.0, pal.guide));
}

fn toggle(expanded: &mut HashSet<PathBuf>, path: &Path) {
    if !expanded.remove(path) {
        expanded.insert(path.to_path_buf());
    }
}

fn count_files(list: &[Entry], pdfs: bool) -> usize {
    list.iter()
        .map(|e| match e.kind {
            EntryKind::Markdown => 1,
            EntryKind::Pdf => usize::from(pdfs),
            _ => count_files(&e.children, pdfs),
        })
        .sum()
}

fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().into_owned()
}

/// 一般資料夾內容。
pub fn entries(ui: &mut Ui, list: &[Entry], depth: usize, cx: &mut TreeCtx) {
    for e in list {
        match e.kind {
            EntryKind::Markdown => file(ui, e, depth, cx),
            EntryKind::Pdf if cx.show_pdfs => file(ui, e, depth, cx),
            EntryKind::Pdf => {}
            EntryKind::Dir => {
                let open = cx.expanded.contains(&e.path);
                let on_path = cx.current.is_some_and(|c| c.starts_with(&e.path));
                let count = count_files(&e.children, cx.show_pdfs);
                let hover = rel(&e.path, cx.lib_root);
                let spec = RowSpec {
                    depth,
                    chevron: Some(open),
                    icon: Some((if open { Icon::FolderOpen } else { Icon::Folder }, cx.pal.folder)),
                    badge: None,
                    text: RichText::new(&e.name).color(cx.pal.ink).into(),
                    trailing: (cx.show_counts && count > 0).then(|| count.to_string()),
                    selected: false,
                    on_path,
                    hover: Some(&hover),
                    draggable: true,
                    cursor: cx.nav.has_cursor(&e.path),
                };
                let resp = row(ui, spec, &cx.pal);
                cx.nav.record(&e.path, depth, Some(open), &resp);
                dnd(ui, &resp, depth, &e.path, true, cx);
                if resp.clicked() {
                    toggle(cx.expanded, &e.path);
                }
                resp.context_menu(|ui| {
                    new_menu(ui, &e.path, cx.actions);
                    ui.separator();
                    novel_item(ui, &e.path, cx.actions);
                    ui.separator();
                    common_menu(ui, &e.path, cx.actions);
                });
                if open {
                    let left = ui.available_rect_before_wrap().left();
                    let top = ui.cursor().top();
                    if count == 0 && e.children.iter().all(|c| c.kind != EntryKind::Dir && c.kind != EntryKind::Book) {
                        empty_row(ui, depth + 1, "（沒有檔案）", &cx.pal);
                    }
                    entries(ui, &e.children, depth + 1, cx);
                    guide(ui, left, depth, top, ui.cursor().top(), &cx.pal);
                }
            }
            EntryKind::Book => {
                let Some(book) = cx.books.get(&e.path) else { continue };
                let open = cx.expanded.contains(&e.path);
                let on_path = cx.current.is_some_and(|c| c.starts_with(&e.path));
                let hover = format!("{}　（{} 章）", rel(&e.path, cx.lib_root), book.chapters.len());
                let spec = RowSpec {
                    depth,
                    chevron: Some(open),
                    icon: Some((Icon::Book, cx.pal.book)),
                    badge: None,
                    text: RichText::new(&book.meta.title).strong().color(cx.pal.ink).into(),
                    trailing: cx.show_counts.then(|| format!("{} 章", book.chapters.len())),
                    selected: false,
                    on_path,
                    hover: Some(&hover),
                    draggable: true,
                    cursor: cx.nav.has_cursor(&e.path),
                };
                let resp = row(ui, spec, &cx.pal);
                cx.nav.record(&e.path, depth, Some(open), &resp);
                dnd(ui, &resp, depth, &e.path, true, cx);
                if resp.clicked() {
                    toggle(cx.expanded, &e.path);
                }
                resp.context_menu(|ui| {
                    novel_item(ui, &e.path, cx.actions);
                    ui.separator();
                    book_menu(ui, book, cx.actions);
                    ui.separator();
                    common_menu(ui, &e.path, cx.actions);
                });
                if open {
                    let left = ui.available_rect_before_wrap().left();
                    let top = ui.cursor().top();
                    book_contents(ui, book, e, depth + 1, cx);
                    guide(ui, left, depth, top, ui.cursor().top(), &cx.pal);
                }
            }
        }
    }
}

fn empty_row(ui: &mut Ui, depth: usize, text: &str, pal: &Palette) {
    let spec = RowSpec {
        depth,
        chevron: None,
        icon: None,
        badge: None,
        text: RichText::new(text).small().color(pal.weak).into(),
        trailing: None,
        selected: false,
        on_path: false,
        hover: None,
        draggable: false,
        cursor: false,
    };
    row(ui, spec, pal);
}

/// 書本內容：依章節順序列出（編號徽章），再列出書中的 PDF（例如輸出的成品）。
pub fn book_contents(ui: &mut Ui, book: &Book, entry: &Entry, depth: usize, cx: &mut TreeCtx) {
    let n = book.chapters.len();
    for (i, ch) in book.chapters.iter().enumerate() {
        let selected = cx.current == Some(ch.path.as_path());
        let spec = RowSpec {
            depth,
            chevron: None,
            icon: None,
            badge: Some(format!("{}", i + 1)),
            text: ch.title.clone().into(),
            trailing: None,
            selected,
            on_path: false,
            hover: Some(&ch.rel),
            draggable: true,
            cursor: cx.nav.has_cursor(&ch.path),
        };
        let resp = row(ui, spec, &cx.pal);
        cx.nav.record(&ch.path, depth, None, &resp);
        dnd(ui, &resp, depth, &ch.path, false, cx);
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
            if ui.button("輸出這一章為 PDF…").clicked() {
                cx.actions.push(Action::Export { source: ch.path.clone(), format: Format::Pdf });
                ui.close();
            }
            ui.separator();
            common_menu(ui, &ch.path, cx.actions);
        });
    }
    if cx.show_pdfs {
        let mut pdfs = Vec::new();
        collect_pdfs(&entry.children, &mut pdfs);
        for pdf in pdfs {
            file(ui, pdf, depth, cx);
        }
    }
    let spec = RowSpec {
        depth,
        chevron: None,
        icon: Some((Icon::Plus, cx.pal.weak)),
        badge: None,
        text: RichText::new("新增章節").color(cx.pal.weak).into(),
        trailing: None,
        selected: false,
        on_path: false,
        hover: None,
        draggable: false,
        cursor: false,
    };
    if row(ui, spec, &cx.pal).clicked() {
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
    let (icon, color) = if is_pdf { (Icon::Pdf, cx.pal.pdf) } else { (Icon::File, cx.pal.markdown) };
    let hover = rel(path, cx.lib_root);
    let spec = RowSpec {
        depth,
        chevron: None,
        icon: Some((icon, color)),
        badge: None,
        text: label.into(),
        trailing: None,
        selected,
        on_path: false,
        hover: Some(&hover),
        draggable: true,
        cursor: cx.nav.has_cursor(path),
    };
    let resp = row(ui, spec, &cx.pal);
    cx.nav.record(path, depth, None, &resp);
    dnd(ui, &resp, depth, path, false, cx);
    if resp.clicked() && !selected {
        cx.actions.push(Action::Open(path.to_path_buf()));
    }
    resp.context_menu(|ui| {
        if is_pdf {
            if ui.button("用預設程式開啟").clicked() {
                cx.actions.push(Action::Reveal(path.to_path_buf()));
                ui.close();
            }
        } else if ui.button("輸出 PDF…").clicked() {
            cx.actions.push(Action::Export { source: path.to_path_buf(), format: Format::Pdf });
            ui.close();
        }
        ui.separator();
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

/// 「以書本模式閱讀」：把資料夾當成一本書，一頁一頁翻。
pub fn novel_item(ui: &mut Ui, dir: &Path, actions: &mut Vec<Action>) {
    if ui.button("以書本模式閱讀").clicked() {
        actions.push(Action::OpenNovel { root: dir.to_path_buf(), chapter: None });
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Key;

    fn press(key: Key) -> egui::RawInput {
        let key = egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        egui::RawInput { events: vec![key], ..Default::default() }
    }

    fn nav_row(path: &str, depth: usize, open: Option<bool>) -> NavRow {
        NavRow { path: PathBuf::from(path), depth, open, rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, ROW_H)) }
    }

    struct Harness {
        ctx: egui::Context,
        nav: TreeNav,
        expanded: HashSet<PathBuf>,
        actions: Vec<Action>,
        /// was the key still visible to later widgets after the tree handled it
        leaked: bool,
    }

    impl Harness {
        fn key(&mut self, key: Key) {
            self.nav.rows = vec![
                nav_row("dir", 0, Some(self.expanded.contains(Path::new("dir")))),
                nav_row("dir/a.md", 1, None),
                nav_row("b.md", 0, None),
            ];
            let mut out = self.ctx.run_ui(press(key), |ui| {
                keyboard(ui, &mut self.nav, &mut self.expanded, &mut self.actions);
                self.leaked = ui.input(|i| i.key_pressed(key));
            });
            // egui panics if texture updates are dropped unhandled
            out.textures_delta.clear();
        }

        fn cursor(&self) -> Option<&str> {
            self.nav.cursor.as_deref().and_then(Path::to_str)
        }
    }

    #[test]
    fn keyboard_navigation() {
        let mut h = Harness {
            ctx: egui::Context::default(),
            nav: TreeNav { active: true, ..Default::default() },
            expanded: HashSet::new(),
            actions: Vec::new(),
            leaked: false,
        };
        h.key(Key::ArrowDown);
        assert_eq!(h.cursor(), Some("dir"));
        assert!(!h.leaked, "the reader must not see a key the tree used");
        h.key(Key::ArrowRight);
        assert!(h.expanded.contains(Path::new("dir")));
        h.key(Key::ArrowRight);
        assert_eq!(h.cursor(), Some("dir/a.md"));
        h.key(Key::Enter);
        assert!(matches!(h.actions.pop(), Some(Action::Open(p)) if p == Path::new("dir/a.md")));
        h.key(Key::F2);
        assert!(matches!(h.actions.pop(), Some(Action::Dialog(Dialog::Rename { name, .. })) if name == "a.md"));
        h.key(Key::Delete);
        assert!(matches!(h.actions.pop(), Some(Action::Dialog(Dialog::Trash { .. }))));
        h.key(Key::ArrowLeft);
        assert_eq!(h.cursor(), Some("dir"), "left on a file goes to its folder");
        h.key(Key::ArrowLeft);
        assert!(!h.expanded.contains(Path::new("dir")));
        h.key(Key::End);
        assert_eq!(h.cursor(), Some("b.md"));
        h.key(Key::ArrowDown);
        assert_eq!(h.cursor(), Some("b.md"), "stays on the last row");
        h.key(Key::Escape);
        assert!(!h.nav.active);
        // not active: keys are left alone
        h.key(Key::ArrowUp);
        assert_eq!(h.cursor(), Some("b.md"));
        assert!(h.leaked);
    }
}
