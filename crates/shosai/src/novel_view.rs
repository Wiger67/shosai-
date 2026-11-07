//! 書本模式：以資料夾為單位，像輕小說一樣一頁一頁翻著讀。
//!
//! - 直排（由右至左）或橫排，單頁或雙頁跨頁
//! - 封面、目錄頁，章首換頁，插圖獨立成頁
//! - 行首／行尾禁則（句號、逗號不會出現在行首，開括號不會留在行尾）
//! - 注音（`｜漢字《かんじ》`）、著重號（`*強調*`）、直排時兩位數字橫向並排（縱中橫）
//! - 記住每本書讀到的位置（以字數記錄，改變字級或視窗大小也不會跑掉）

use std::f32::consts::FRAC_PI_2;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CursorIcon, FontId, Key, Rect, RichText, Sense, Shape, Stroke, Ui,
};
use serde::{Deserialize, Serialize};
use shosai_core::novel::{self, NBlock, NovelBook, Span};

use crate::icons::{self, Icon};
use crate::images::{self, ImageCache};
use crate::theme::Palette;

/// 書本模式的偏好設定（全部書共用）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NovelPrefs {
    /// 直排（由右至左）
    pub vertical: bool,
    /// 視窗夠寬時左右兩頁並排
    pub spread: bool,
    /// 內文字級（px）
    pub font_size: f32,
    /// 行距（行與行中心的距離 ÷ 字級）
    pub line_spacing: f32,
    /// 段落首行縮排幾個字
    pub indent: u8,
    /// 段落之間空半行
    pub para_gap: bool,
}

impl Default for NovelPrefs {
    fn default() -> Self {
        Self { vertical: true, spread: true, font_size: 18.0, line_spacing: 1.8, indent: 2, para_gap: false }
    }
}

/// 回報給 App 的操作。
pub enum NovelEvent {
    /// 結束書本模式
    Close,
}

// ---------------------------------------------------------------------------
// 排版資料

const STRONG: u8 = 1;
const EMPH: u8 = 2;
const WEAK: u8 = 4;
const HEAD: u8 = 8;

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    /// 一個字（中日文、標點）
    Char(char),
    /// 連續的西文 / 數字（直排時整段轉 90°，不拆開）
    Word(String),
    /// 縱中橫：直排時兩位以內的數字橫向擠在一格
    Tcy(String),
    /// 有注音的一組字
    Ruby {
        base: String,
        ruby: String,
    },
    Space,
    /// 段內換行
    Newline,
}

#[derive(Debug, Clone)]
struct Tok {
    kind: Kind,
    /// 行進方向的長度
    adv: f32,
    size: f32,
    style: u8,
    /// 在這一章中的字數位置
    src: usize,
}

#[derive(Debug, Clone)]
struct Line {
    items: Vec<(f32, Tok)>,
    /// 行的粗細（垂直於行進方向）
    extent: f32,
    /// 和上一行之間額外的距離（在頁首時忽略）
    gap: f32,
    center: bool,
    src: usize,
    /// 目錄頁：點了跳到第幾章
    link: Option<usize>,
}

#[derive(Debug, Clone)]
enum Page {
    Cover,
    Image {
        path: PathBuf,
        alt: String,
    },
    /// 文字頁：每一行與它在頁中的位置（垂直於行進方向）
    Text(Vec<(usize, f32)>),
}

impl Page {
    fn first_src(&self, lines: &[Line]) -> Option<usize> {
        match self {
            Page::Text(ls) => ls.first().map(|(i, _)| lines[*i].src),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Unit {
    /// 封面與目錄
    Front,
    Chapter(usize),
}

#[derive(Debug, Clone, Copy)]
enum Want {
    First,
    Last,
    Offset(usize),
}

#[derive(Debug, Clone, PartialEq)]
struct LayoutKey {
    unit: Unit,
    grid: [u32; 2],
    prefs: [u32; 5],
    stamp: Option<SystemTime>,
    chapters: usize,
}

struct Layout {
    key: LayoutKey,
    lines: Vec<Line>,
    pages: Vec<Page>,
    /// 實際使用的格子大小（行進方向的長度、可放的行寬總和）
    line_len: f32,
}

/// 書本模式的檢視器。
pub struct NovelView {
    pub book: NovelBook,
    unit: Unit,
    page: usize,
    want: Want,
    layout: Option<Layout>,
    /// 目前這一章的內容（路徑、修改時間、段落）
    blocks: Option<(PathBuf, Option<SystemTime>, Vec<NBlock>)>,
    pub show_toc: bool,
    /// 打開目錄時捲到目前這一章
    toc_scroll: bool,
    images: ImageCache,
    wheel: f32,
    /// 讀完最後一頁時短暫顯示提示
    end_flash: Option<std::time::Instant>,
}

impl NovelView {
    /// 開啟一本書；`at` 是要從哪一章、第幾個字開始（沒有則從封面開始）。
    pub fn open(book: NovelBook, at: Option<(PathBuf, usize)>) -> NovelView {
        let (unit, want) = match at.and_then(|(p, off)| book.index_of(&p).map(|i| (i, off))) {
            Some((i, off)) => (Unit::Chapter(i), Want::Offset(off)),
            None => (Unit::Front, Want::First),
        };
        NovelView {
            book,
            unit,
            page: 0,
            want,
            layout: None,
            blocks: None,
            show_toc: false,
            toc_scroll: true,
            images: ImageCache::default(),
            wheel: 0.0,
            end_flash: None,
        }
    }

    /// 書的章節清單更新了（例如資料夾裡多了檔案）。
    pub fn update_book(&mut self, book: NovelBook) {
        let current = self.current_chapter().map(Path::to_path_buf);
        let offset = self.offset();
        self.book = book;
        match current.and_then(|p| self.book.index_of(&p)) {
            Some(i) => {
                self.unit = Unit::Chapter(i);
                self.want = Want::Offset(offset);
            }
            None if matches!(self.unit, Unit::Chapter(_)) => {
                self.unit = Unit::Front;
                self.want = Want::First;
            }
            None => self.want = Want::First,
        }
        self.layout = None;
    }

    pub fn current_chapter(&self) -> Option<&Path> {
        match self.unit {
            Unit::Chapter(i) => self.book.chapters.get(i).map(|c| c.path.as_path()),
            Unit::Front => None,
        }
    }

    /// 目前頁第一個字在章中的位置。
    fn offset(&self) -> usize {
        match &self.layout {
            Some(l) => l.pages.get(self.page).and_then(|p| p.first_src(&l.lines)).unwrap_or(0),
            None => match self.want {
                Want::Offset(o) => o,
                _ => 0,
            },
        }
    }

    /// 要記住的閱讀位置。
    pub fn position(&self) -> Option<(PathBuf, usize)> {
        self.current_chapter().map(|p| (p.to_path_buf(), self.offset()))
    }

    pub fn goto_chapter(&mut self, path: &Path) -> bool {
        match self.book.index_of(path) {
            Some(i) => {
                self.go(Unit::Chapter(i), Want::First);
                true
            }
            None => false,
        }
    }

    fn go(&mut self, unit: Unit, want: Want) {
        if self.unit != unit {
            self.unit = unit;
            self.layout = None;
        }
        self.want = want;
        self.apply_want();
    }

    fn step(&self, spread: bool) -> usize {
        if spread {
            2
        } else {
            1
        }
    }

    fn next_page(&mut self, spread: bool) {
        let n = self.layout.as_ref().map(|l| l.pages.len()).unwrap_or(0);
        if self.page + self.step(spread) < n {
            self.page += self.step(spread);
            return;
        }
        let next = match self.unit {
            Unit::Front => Some(0),
            Unit::Chapter(i) => Some(i + 1),
        };
        match next.filter(|i| *i < self.book.chapters.len()) {
            Some(i) => self.go(Unit::Chapter(i), Want::First),
            None => self.end_flash = Some(std::time::Instant::now()),
        }
    }

    fn prev_page(&mut self, spread: bool) {
        if self.page >= self.step(spread) {
            self.page -= self.step(spread);
            return;
        }
        match self.unit {
            Unit::Chapter(0) => self.go(Unit::Front, Want::Last),
            Unit::Chapter(i) => self.go(Unit::Chapter(i - 1), Want::Last),
            Unit::Front => {}
        }
    }

    fn apply_want(&mut self) {
        let Some(l) = &self.layout else { return };
        let n = l.pages.len().max(1);
        self.page = match self.want {
            Want::First => 0,
            Want::Last => n - 1,
            Want::Offset(o) => {
                let mut best = 0;
                for (i, p) in l.pages.iter().enumerate() {
                    match p.first_src(&l.lines) {
                        Some(s) if s <= o => best = i,
                        Some(_) => break,
                        None => {}
                    }
                }
                best
            }
        };
    }

    /// 全書進度（0–1）。
    fn progress(&self) -> f32 {
        let n = self.book.chapters.len().max(1) as f32;
        let pages = self.layout.as_ref().map(|l| l.pages.len()).unwrap_or(1).max(1) as f32;
        match self.unit {
            Unit::Front => 0.0,
            Unit::Chapter(i) => ((i as f32) + (self.page as f32 + 1.0) / pages) / n,
        }
    }

    /// 狀態列的說明文字：左邊（位置）與右邊（頁數）。
    pub fn status(&self) -> (String, String) {
        let pages = self.layout.as_ref().map(|l| l.pages.len()).unwrap_or(0);
        let left = match self.unit {
            Unit::Front => format!("{}　·　封面與目錄", self.book.title),
            Unit::Chapter(i) => {
                let ch = &self.book.chapters[i];
                match &ch.volume {
                    Some(v) => format!("{}　·　{v}　·　{}", self.book.title, ch.title),
                    None => format!("{}　·　{}", self.book.title, ch.title),
                }
            }
        };
        let right = if pages == 0 {
            String::new()
        } else {
            format!("{} / {} 頁　·　全書 {:.0}%", self.page + 1, pages, self.progress() * 100.0)
        };
        (left, right)
    }

    // -----------------------------------------------------------------------
    // 工具列

    pub fn toolbar_left(&mut self, ui: &mut Ui, prefs: &mut NovelPrefs, events: &mut Vec<NovelEvent>) {
        if icons::button(ui, Icon::ChevronLeft, "結束書本模式（Esc）").clicked() {
            events.push(NovelEvent::Close);
        }
        let toc = if self.show_toc { "隱藏目錄（T）" } else { "目錄（T）" };
        if icons::button(ui, Icon::List, toc).clicked() {
            self.show_toc = !self.show_toc;
        }
        ui.add_space(8.0);
        egui::Frame::new().fill(ui.visuals().faint_bg_color).corner_radius(8).inner_margin(2).show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (v, label) in [(true, "直排"), (false, "橫排")] {
                let b =
                    egui::Button::selectable(prefs.vertical == v, label).corner_radius(6).min_size(vec2(48.0, 24.0));
                if ui.add(b).clicked() {
                    prefs.vertical = v;
                }
            }
        });
        ui.add_space(4.0);
        let tip = if prefs.spread { "改為單頁" } else { "改為雙頁（視窗夠寬時）" };
        let resp = icons::button(ui, if prefs.spread { Icon::Spread } else { Icon::FitPage }, tip);
        if resp.clicked() {
            prefs.spread = !prefs.spread;
        }
    }

    pub fn toolbar_right(&mut self, ui: &mut Ui, prefs: &mut NovelPrefs) {
        // right_to_left 版面：先加的在最右邊
        let resp = icons::button(ui, Icon::Text, "文字設定");
        egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_min_width(240.0);
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(RichText::new("字級").small().weak());
            ui.add(egui::Slider::new(&mut prefs.font_size, 12.0..=32.0).step_by(1.0).suffix(" px"));
            ui.label(RichText::new("行距").small().weak());
            ui.add(egui::Slider::new(&mut prefs.line_spacing, 1.3..=2.6).step_by(0.1).suffix(" 倍"));
            ui.label(RichText::new("段首縮排").small().weak());
            ui.horizontal(|ui| {
                for (n, label) in [(0, "不縮排"), (1, "一字"), (2, "兩字")] {
                    ui.selectable_value(&mut prefs.indent, n, label);
                }
            });
            ui.checkbox(&mut prefs.para_gap, "段落之間空半行");
        });
        if icons::button(ui, Icon::Plus, "放大字級（Ctrl+=）").clicked() {
            prefs.font_size = (prefs.font_size + 1.0).min(32.0);
        }
        if icons::button(ui, Icon::Minus, "縮小字級（Ctrl+-）").clicked() {
            prefs.font_size = (prefs.font_size - 1.0).max(12.0);
        }
    }

    // -----------------------------------------------------------------------
    // 主畫面

    pub fn show(&mut self, ui: &mut Ui, pal: &Palette, prefs: &NovelPrefs, events: &mut Vec<NovelEvent>) {
        if !self.show_toc {
            self.toc_scroll = true;
        } else {
            egui::Panel::left("novel-toc")
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(pal.chrome).inner_margin(egui::Margin::symmetric(10, 10)))
                .resizable(false)
                .default_size(250.0)
                .show(ui, |ui| self.toc_panel(ui, pal));
        }
        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, 0.0, pal.desk);
        let resp = ui.allocate_rect(area, Sense::click());

        // 頁面大小：B6 左右的比例，視窗夠寬才並排兩頁
        let outer = area.shrink2(vec2(20.0, 18.0));
        let ratio = 0.7;
        let page_h = (outer.height() - 14.0).max(120.0);
        let gutter = 0.0;
        let single_w = (page_h * ratio).min(outer.width()).max(120.0);
        let spread = prefs.spread && outer.width() >= 2.0 * page_h * 0.52 + gutter;
        let page_w = if spread { (page_h * ratio).min((outer.width() - gutter) / 2.0) } else { single_w };
        let page_size = vec2(page_w, page_h);
        let total_w = if spread { page_w * 2.0 + gutter } else { page_w };
        let top_left = pos2(outer.center().x - total_w / 2.0, outer.top());
        let margin = vec2((page_w * 0.1).clamp(16.0, 56.0), (page_h * 0.085).clamp(22.0, 64.0));
        let content = vec2(page_w - 2.0 * margin.x, page_h - 2.0 * margin.y);

        self.ensure_layout(ui, content, prefs);
        if spread {
            self.page &= !1;
        }

        // 輸入
        self.input(ui, &resp, prefs, spread, events);
        let Some(layout) = &self.layout else { return };
        let n = layout.pages.len();

        // 頁面（直排由右至左：第一頁在右）
        let mut slots: Vec<(Rect, usize)> = Vec::new();
        let first = Rect::from_min_size(top_left, page_size);
        if spread {
            let second = first.translate(vec2(page_w + gutter, 0.0));
            let (a, b) = if prefs.vertical { (second, first) } else { (first, second) };
            slots.push((a, self.page));
            if self.page + 1 < n {
                slots.push((b, self.page + 1));
            } else {
                paint_blank(ui, b, pal);
            }
        } else {
            slots.push((first, self.page));
        }
        let mut links: Vec<(Rect, usize)> = Vec::new();
        for (rect, idx) in &slots {
            let Some(page) = layout.pages.get(*idx) else { continue };
            paint_paper(ui, *rect, pal);
            let inner = Rect::from_min_size(rect.min + margin, content);
            match page {
                Page::Cover => self.paint_cover(ui, *rect, pal),
                Page::Image { path, alt } => paint_image(ui, inner, path, alt, &self.images, pal),
                Page::Text(lines) => {
                    paint_text(ui, inner, layout, lines, prefs, pal, &mut links);
                }
            }
            // 頁首（書名 / 章名）與頁碼
            if !matches!(page, Page::Cover) {
                let small = FontId::proportional((prefs.font_size * 0.62).clamp(10.0, 13.0));
                let head = match self.unit {
                    Unit::Front => "目　錄".to_string(),
                    Unit::Chapter(i) => self.book.chapters[i].title.clone(),
                };
                // 雙頁時：一邊印書名、另一邊印章名（像實體書的書眉）
                let head = if !spread || (*idx % 2 == 1) == !prefs.vertical { head } else { self.book.title.clone() };
                let p = ui.painter();
                p.text(
                    pos2(rect.center().x, rect.top() + margin.y * 0.5),
                    Align2::CENTER_CENTER,
                    head,
                    small.clone(),
                    pal.weak,
                );
                p.text(
                    pos2(rect.center().x, rect.bottom() - margin.y * 0.5),
                    Align2::CENTER_CENTER,
                    format!("{}", idx + 1),
                    small,
                    pal.weak,
                );
            }
        }
        if spread && slots.len() == 2 {
            // 書脊的陰影
            let x = first.right();
            let shade = Color32::from_black_alpha(if ui.visuals().dark_mode { 60 } else { 22 });
            for i in 0..8 {
                let a = shade.gamma_multiply(1.0 - i as f32 / 8.0);
                ui.painter().vline(x - i as f32, first.y_range(), Stroke::new(1.0, a));
                ui.painter().vline(x + i as f32, first.y_range(), Stroke::new(1.0, a));
            }
        }

        // 目錄頁的連結
        let hover_link = ui.input(|i| i.pointer.hover_pos()).and_then(|p| links.iter().find(|(r, _)| r.contains(p)));
        if let Some((r, _)) = hover_link {
            ui.painter().rect_filled(r.expand(2.0), 4.0, pal.hover);
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        let clicked_link = resp.clicked().then_some(hover_link).flatten().map(|(_, i)| *i);

        // 進度條
        let bar_y = area.bottom() - 7.0;
        let bar = Rect::from_min_max(pos2(top_left.x, bar_y - 1.5), pos2(top_left.x + total_w, bar_y + 1.5));
        ui.painter().rect_filled(bar, 1.5, pal.guide);
        let done = Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * self.progress(), bar.bottom()));
        ui.painter().rect_filled(done, 1.5, pal.accent);

        if let Some(t) = self.end_flash {
            if t.elapsed().as_secs_f32() < 2.0 {
                let r = Rect::from_center_size(pos2(area.center().x, area.bottom() - 40.0), vec2(160.0, 32.0));
                ui.painter().rect_filled(r, 8.0, pal.chrome);
                ui.painter().text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    "已經是最後一頁",
                    FontId::proportional(14.0),
                    pal.ink,
                );
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
            } else {
                self.end_flash = None;
            }
        }

        if let Some(ch) = clicked_link {
            self.go(Unit::Chapter(ch), Want::First);
        } else if resp.clicked() {
            // 點頁面左右兩側翻頁，中間開關目錄
            if let Some(p) = resp.interact_pointer_pos() {
                let t = (p.x - area.left()) / area.width();
                let forward_on_right = !prefs.vertical;
                if t < 0.35 {
                    if forward_on_right {
                        self.prev_page(spread)
                    } else {
                        self.next_page(spread)
                    }
                } else if t > 0.65 {
                    if forward_on_right {
                        self.next_page(spread)
                    } else {
                        self.prev_page(spread)
                    }
                } else {
                    self.show_toc = !self.show_toc;
                }
            }
        }
    }

    fn input(
        &mut self,
        ui: &Ui,
        resp: &egui::Response,
        prefs: &NovelPrefs,
        spread: bool,
        events: &mut Vec<NovelEvent>,
    ) {
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        if !typing {
            let (fwd_h, back_h, fwd, back, home, end, esc, toc) = ui.input(|i| {
                (
                    i.key_pressed(Key::ArrowRight),
                    i.key_pressed(Key::ArrowLeft),
                    i.key_pressed(Key::PageDown)
                        || i.key_pressed(Key::ArrowDown)
                        || (i.key_pressed(Key::Space) && !i.modifiers.shift),
                    i.key_pressed(Key::PageUp)
                        || i.key_pressed(Key::ArrowUp)
                        || (i.key_pressed(Key::Space) && i.modifiers.shift),
                    i.key_pressed(Key::Home),
                    i.key_pressed(Key::End),
                    i.key_pressed(Key::Escape),
                    i.key_pressed(Key::T),
                )
            });
            // 直排的書由右往左翻：← 是下一頁
            let (fwd_h, back_h) = if prefs.vertical { (back_h, fwd_h) } else { (fwd_h, back_h) };
            if fwd || fwd_h {
                self.next_page(spread);
            }
            if back || back_h {
                self.prev_page(spread);
            }
            if home {
                self.page = 0;
            }
            if end {
                let n = self.layout.as_ref().map(|l| l.pages.len()).unwrap_or(1);
                self.page = n.saturating_sub(1);
            }
            if esc {
                events.push(NovelEvent::Close);
            }
            if toc {
                self.show_toc = !self.show_toc;
            }
        }
        if resp.contains_pointer() {
            let d = ui.input(|i| i.smooth_scroll_delta);
            let d = if d.y.abs() > d.x.abs() {
                d.y
            } else if prefs.vertical {
                d.x
            } else {
                -d.x
            };
            self.wheel += d;
            if self.wheel <= -60.0 {
                self.wheel = 0.0;
                self.next_page(spread);
            } else if self.wheel >= 60.0 {
                self.wheel = 0.0;
                self.prev_page(spread);
            }
        }
    }

    fn toc_panel(&mut self, ui: &mut Ui, pal: &Palette) {
        ui.label(RichText::new(&self.book.title).strong().size(15.0).color(pal.ink));
        if !self.book.author.is_empty() {
            ui.label(RichText::new(&self.book.author).small().color(pal.weak));
        }
        ui.add_space(6.0);
        let mut goto = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let front = egui::Button::selectable(self.unit == Unit::Front, "封面與目錄")
                .min_size(vec2(ui.available_width(), 26.0));
            if ui.add(front).clicked() {
                goto = Some(Unit::Front);
            }
            let mut last_volume: Option<&str> = None;
            for (i, ch) in self.book.chapters.iter().enumerate() {
                if ch.volume.as_deref() != last_volume {
                    last_volume = ch.volume.as_deref();
                    if let Some(v) = last_volume {
                        ui.add_space(6.0);
                        ui.label(RichText::new(v).small().strong().color(pal.weak));
                    }
                }
                let current = self.unit == Unit::Chapter(i);
                let text = if ch.volume.is_some() { format!("　{}", ch.title) } else { ch.title.clone() };
                let b = egui::Button::selectable(current, text).min_size(vec2(ui.available_width(), 26.0));
                let r = ui.add(b);
                if current && self.toc_scroll {
                    r.scroll_to_me(Some(egui::Align::Center));
                    self.toc_scroll = false;
                }
                if r.clicked() {
                    goto = Some(Unit::Chapter(i));
                }
            }
        });
        if let Some(u) = goto {
            self.go(u, Want::First);
        }
    }

    fn paint_cover(&self, ui: &Ui, rect: Rect, pal: &Palette) {
        if let Some(cover) = &self.book.cover {
            if let Some(Ok((tex, [w, h]))) = images::get(&self.images, cover, ui.ctx()) {
                let size = vec2(w as f32, h as f32);
                let scale = (rect.width() / size.x).max(rect.height() / size.y);
                let shown = size * scale;
                // 填滿整頁（超出的部分裁掉），像實體書的封面
                let uv_w = (rect.width() / shown.x).min(1.0);
                let uv_h = (rect.height() / shown.y).min(1.0);
                let uv = Rect::from_center_size(pos2(0.5, 0.5), vec2(uv_w, uv_h));
                ui.painter().image(tex, rect, uv, Color32::WHITE);
                return;
            }
        }
        // 沒有封面圖：用書名排一個簡單的封面
        let p = ui.painter();
        let band = Rect::from_min_max(
            pos2(rect.left(), rect.top() + rect.height() * 0.3),
            pos2(rect.right(), rect.top() + rect.height() * 0.3 + 4.0),
        );
        p.rect_filled(band, 0.0, pal.accent);
        let title = p.layout(
            self.book.title.clone(),
            FontId::proportional((rect.width() * 0.09).clamp(18.0, 40.0)),
            pal.ink,
            rect.width() * 0.8,
        );
        let tpos = pos2(rect.center().x - title.size().x / 2.0, rect.top() + rect.height() * 0.36);
        let th = title.size().y;
        p.galley(tpos, title, pal.ink);
        if !self.book.author.is_empty() {
            p.text(
                pos2(rect.center().x, tpos.y + th + 24.0),
                Align2::CENTER_TOP,
                &self.book.author,
                FontId::proportional(15.0),
                pal.weak,
            );
        }
        p.text(
            pos2(rect.center().x, rect.bottom() - 40.0),
            Align2::CENTER_CENTER,
            format!("共 {} 章", self.book.chapters.len()),
            FontId::proportional(12.0),
            pal.weak,
        );
    }

    // -----------------------------------------------------------------------
    // 排版

    fn ensure_layout(&mut self, ui: &Ui, content: egui::Vec2, prefs: &NovelPrefs) {
        // 章節檔案：讀取並快取段落
        let chapter = self.current_chapter().map(Path::to_path_buf);
        let stamp = chapter.as_deref().and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        if let Some(path) = &chapter {
            let stale = self.blocks.as_ref().is_none_or(|(p, s, _)| p != path || *s != stamp);
            if stale {
                let text = std::fs::read_to_string(path).unwrap_or_else(|e| format!("無法讀取這一章：{e}"));
                self.blocks = Some((path.clone(), stamp, novel::blocks(&text)));
            }
        }
        let key = LayoutKey {
            unit: self.unit,
            grid: [content.x.round() as u32, content.y.round() as u32],
            prefs: [
                prefs.font_size.to_bits(),
                prefs.line_spacing.to_bits(),
                u32::from(prefs.vertical),
                u32::from(prefs.indent),
                u32::from(prefs.para_gap),
            ],
            stamp,
            chapters: self.book.chapters.len(),
        };
        if self.layout.as_ref().is_some_and(|l| l.key == key) {
            return;
        }
        // 同一章重新排版（字級、視窗大小改變）時保持在同一個位置
        if let Some(old) = &self.layout {
            if old.key.unit == key.unit {
                self.want = Want::Offset(old.pages.get(self.page).and_then(|p| p.first_src(&old.lines)).unwrap_or(0));
            }
        }
        let base_dir = chapter.as_deref().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
        let blocks = match self.unit {
            Unit::Front => toc_blocks(&self.book),
            Unit::Chapter(_) => self.blocks.as_ref().map(|(_, _, b)| b.clone()).unwrap_or_default(),
        };
        let mut layout = ui.ctx().fonts_mut(|f| {
            let mut measure = |c: char, size: f32| {
                if is_wide(c) {
                    size
                } else {
                    let w = f.glyph_width(&FontId::proportional(size), c);
                    if w > 0.0 {
                        w
                    } else {
                        size * 0.55
                    }
                }
            };
            build(&blocks, content, prefs, &base_dir, &mut measure)
        });
        if self.unit == Unit::Front {
            layout.pages.insert(0, Page::Cover);
        }
        self.layout = Some(Layout { key, lines: layout.lines, pages: layout.pages, line_len: layout.line_len });
        self.apply_want();
    }
}

/// 目錄頁的內容：書名、各卷與各章（每一章一行，可點）。
fn toc_blocks(book: &NovelBook) -> Vec<NBlock> {
    let mut out =
        vec![NBlock::Heading { level: 1, spans: vec![Span { text: "目　錄".into(), ..Default::default() }] }];
    let mut last: Option<&str> = None;
    for (i, ch) in book.chapters.iter().enumerate() {
        if ch.volume.as_deref() != last {
            last = ch.volume.as_deref();
            if let Some(v) = last {
                out.push(NBlock::Pre(format!("\u{1}{v}")));
            }
        }
        let indent = if ch.volume.is_some() { "　" } else { "" };
        // 用控制字元標記連結目標（只在目錄頁使用）
        out.push(NBlock::Pre(format!("\u{2}{i}\u{2}{indent}{}", ch.title)));
    }
    out
}

struct Built {
    lines: Vec<Line>,
    pages: Vec<Page>,
    line_len: f32,
}

const NO_START: &str =
    "、。，．,.:;!?)]}）］｝」』】〕〉》〙〗’”…‥ー～〜ゝゞ々ぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮヵヶ！？：；・";
const NO_END: &str = "([{（［｛「『【〔〈《〘〖‘“";

/// 把段落排成行，再把行分配到頁。
fn build(
    blocks: &[NBlock],
    content: egui::Vec2,
    prefs: &NovelPrefs,
    base_dir: &Path,
    measure: &mut dyn FnMut(char, f32) -> f32,
) -> Built {
    let size = prefs.font_size;
    let vertical = prefs.vertical;
    // 行進方向的長度，取字級的整數倍（整頁像稿紙一樣對齊）
    let raw_len = if vertical { content.y } else { content.x };
    let line_len = ((raw_len / size).floor() * size).max(size * 4.0);
    let capacity = if vertical { content.x } else { content.y };
    let extent = size * prefs.line_spacing;
    let para_gap = if prefs.para_gap { extent * 0.5 } else { 0.0 };

    let mut lines: Vec<Line> = Vec::new();
    let mut pages: Vec<Page> = Vec::new();
    let mut src = 0usize;
    // 每個區塊：先排成行，標記 Page 中斷點
    enum Piece {
        Lines(usize, usize),
        Image(PathBuf, String),
        Break,
    }
    let mut pieces: Vec<Piece> = Vec::new();

    for (bi, b) in blocks.iter().enumerate() {
        let start = lines.len();
        match b {
            NBlock::Heading { level, spans } => {
                let hs = size * if *level <= 1 { 1.4 } else { 1.15 };
                let toks = tokenize(spans, hs, HEAD | STRONG, vertical, line_len, &mut src, measure);
                let lead = if vertical { size * 2.0 } else { 0.0 };
                let mut ls = break_lines(&toks, line_len, lead, lead, hs * prefs.line_spacing);
                if let Some(first) = ls.first_mut() {
                    first.gap = if bi == 0 { 0.0 } else { extent * 1.5 };
                }
                lines.extend(ls);
                // 標題後空一行
                if let Some(last) = lines.last_mut() {
                    last.extent += extent * 0.6;
                }
            }
            NBlock::Para(spans) | NBlock::Quote(spans) => {
                let quote = matches!(b, NBlock::Quote(_));
                let toks = tokenize(spans, size, 0, vertical, line_len, &mut src, measure);
                let rest = if quote { size * 2.0 } else { 0.0 };
                let first = rest + size * f32::from(prefs.indent);
                let starts_with_space = spans.first().is_some_and(|s| s.text.starts_with(['　', ' ']));
                let first = if starts_with_space || quote { rest } else { first };
                let mut ls = break_lines(&toks, line_len, first, rest, extent);
                if let Some(l) = ls.first_mut() {
                    l.gap = para_gap;
                }
                lines.extend(ls);
            }
            NBlock::Pre(text) => {
                // 目錄頁用的標記
                let (link, text, strong) = if let Some(rest) = text.strip_prefix('\u{2}') {
                    let (n, t) = rest.split_once('\u{2}').unwrap_or(("", rest));
                    (n.parse::<usize>().ok(), t.to_string(), false)
                } else if let Some(v) = text.strip_prefix('\u{1}') {
                    (None, v.to_string(), true)
                } else {
                    (None, text.clone(), false)
                };
                let style = if strong {
                    STRONG
                } else if link.is_some() {
                    0
                } else {
                    WEAK
                };
                let ps = if link.is_some() || strong { size } else { size * 0.85 };
                let span = Span { text, ..Default::default() };
                let toks = tokenize(std::slice::from_ref(&span), ps, style, vertical, line_len, &mut src, measure);
                let mut ls = break_lines(&toks, line_len, 0.0, 0.0, ps * prefs.line_spacing);
                for l in &mut ls {
                    l.link = link;
                }
                if strong {
                    if let Some(l) = ls.first_mut() {
                        l.gap = extent * 0.5;
                    }
                }
                lines.extend(ls);
            }
            NBlock::Scene => {
                let t = Tok { kind: Kind::Char('◇'), adv: size, size, style: WEAK, src };
                let items = (0..3).map(|i| (i as f32 * size * 2.0, t.clone())).collect();
                lines.push(Line { items, extent, gap: extent * 0.5, center: true, src, link: None });
                if let Some(l) = lines.last_mut() {
                    l.extent += extent * 0.5;
                }
            }
            NBlock::Image { url, alt } => {
                if images::is_local_raster(url) {
                    pieces.push(Piece::Image(images::resolve(base_dir, url), alt.clone()));
                }
                continue;
            }
            NBlock::PageBreak => {
                pieces.push(Piece::Break);
                continue;
            }
        }
        pieces.push(Piece::Lines(start, lines.len()));
    }

    // 分頁
    let mut cur: Vec<(usize, f32)> = Vec::new();
    let mut pos = 0.0f32;
    let flush = |cur: &mut Vec<(usize, f32)>, pos: &mut f32, pages: &mut Vec<Page>| {
        if !cur.is_empty() {
            pages.push(Page::Text(std::mem::take(cur)));
        }
        *pos = 0.0;
    };
    for piece in pieces {
        match piece {
            Piece::Lines(a, b) => {
                for (i, l) in lines.iter().enumerate().take(b).skip(a) {
                    let gap = if cur.is_empty() { 0.0 } else { l.gap };
                    // 只要字本身放得下就好（行距的上下留白可以落在頁邊）
                    let ls = line_size(l, size);
                    let need = gap + ls * prefs.line_spacing / 2.0 + ls / 2.0;
                    if !cur.is_empty() && pos + need > capacity + 0.5 {
                        flush(&mut cur, &mut pos, &mut pages);
                        cur.push((i, 0.0));
                        pos = l.extent;
                    } else {
                        cur.push((i, pos + gap));
                        pos += gap + l.extent;
                    }
                }
            }
            Piece::Image(path, alt) => {
                flush(&mut cur, &mut pos, &mut pages);
                pages.push(Page::Image { path, alt });
            }
            Piece::Break => flush(&mut cur, &mut pos, &mut pages),
        }
    }
    flush(&mut cur, &mut pos, &mut pages);
    if pages.is_empty() {
        pages.push(Page::Text(Vec::new()));
    }
    Built { lines, pages, line_len }
}

fn line_size(l: &Line, default: f32) -> f32 {
    l.items.iter().map(|(_, t)| t.size).fold(0.0, f32::max).max(if l.items.is_empty() { default } else { 0.0 })
}

/// 全形字（中日韓文字、全形標點）。
fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE10..=0xFE19 | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x20000..=0x3FFFD | 0x25A0..=0x25FF | 0x2605..=0x2606)
}

/// 直排時要轉 90° 的全形符號（括號、破折號、刪節號、長音…）。
fn rotate_in_vertical(c: char) -> bool {
    "「」『』（）［］｛｝〔〕〈〉《》【】〖〗〘〙…‥—―─～〜ー－＝→←↑↓".contains(c)
}

fn tokenize(
    spans: &[Span],
    size: f32,
    base_style: u8,
    vertical: bool,
    line_len: f32,
    src: &mut usize,
    measure: &mut dyn FnMut(char, f32) -> f32,
) -> Vec<Tok> {
    let mut out = Vec::new();
    for s in spans {
        let style = base_style
            | if s.strong { STRONG } else { 0 }
            | if s.emph { EMPH } else { 0 }
            | if s.weak { WEAK } else { 0 };
        if let Some(ruby) = &s.ruby {
            // 直排時每個字佔一格
            let adv: f32 = if vertical {
                s.text.chars().count() as f32 * size
            } else {
                s.text.chars().map(|c| measure(c, size)).sum()
            };
            out.push(Tok {
                kind: Kind::Ruby { base: s.text.clone(), ruby: ruby.clone() },
                adv,
                size,
                style,
                src: *src,
            });
            *src += s.text.chars().count();
            continue;
        }
        let mut word = String::new();
        let mut word_src = *src;
        let flush =
            |word: &mut String, word_src: usize, out: &mut Vec<Tok>, measure: &mut dyn FnMut(char, f32) -> f32| {
                if word.is_empty() {
                    return;
                }
                let w = std::mem::take(word);
                let n = w.chars().count();
                if vertical && n <= 2 && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '!' || c == '?') {
                    out.push(Tok { kind: Kind::Tcy(w), adv: size, size, style, src: word_src });
                    return;
                }
                let adv: f32 = w.chars().map(|c| measure(c, size)).sum();
                if adv > line_len * 0.8 {
                    // 太長的字串（網址等）拆成單字，讓它可以換行
                    for (k, c) in w.chars().enumerate() {
                        out.push(Tok {
                            kind: Kind::Word(c.to_string()),
                            adv: measure(c, size),
                            size,
                            style,
                            src: word_src + k,
                        });
                    }
                } else {
                    out.push(Tok { kind: Kind::Word(w), adv, size, style, src: word_src });
                }
            };
        for c in s.text.chars() {
            if c == '\n' {
                flush(&mut word, word_src, &mut out, measure);
                out.push(Tok { kind: Kind::Newline, adv: 0.0, size, style, src: *src });
            } else if c == ' ' || c == '\t' {
                flush(&mut word, word_src, &mut out, measure);
                out.push(Tok { kind: Kind::Space, adv: measure(' ', size), size, style, src: *src });
            } else if is_wide(c) || c == '…' || c == '—' {
                flush(&mut word, word_src, &mut out, measure);
                let adv = if c == '…' || c == '—' { size } else { measure(c, size) };
                out.push(Tok { kind: Kind::Char(c), adv, size, style, src: *src });
            } else if !c.is_control() {
                if word.is_empty() {
                    word_src = *src;
                }
                word.push(c);
            }
            *src += 1;
        }
        flush(&mut word, word_src, &mut out, measure);
    }
    out
}

fn is_char_in(t: &Tok, set: &str) -> bool {
    match &t.kind {
        Kind::Char(c) => set.contains(*c),
        Kind::Word(w) => w.chars().count() == 1 && w.chars().all(|c| set.contains(c)),
        _ => false,
    }
}

/// 斷行：盡量填滿一行。
///
/// - 行首禁則（句號、逗號、閉括號…）：掛在上一行的行尾，最多兩個
/// - 行尾禁則（開括號）：帶到下一行
fn break_lines(toks: &[Tok], line_len: f32, first: f32, rest: f32, extent: f32) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut cur: Vec<(f32, Tok)> = Vec::new();
    let mut x = first;
    let mut line_src = toks.first().map(|t| t.src).unwrap_or(0);
    let finish = |cur: &mut Vec<(f32, Tok)>, lines: &mut Vec<Line>, src: usize| {
        lines.push(Line { items: std::mem::take(cur), extent, gap: 0.0, center: false, src, link: None });
    };
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        i += 1;
        match t.kind {
            Kind::Newline => {
                finish(&mut cur, &mut lines, line_src);
                x = first;
                line_src = t.src + 1;
                continue;
            }
            Kind::Space if cur.is_empty() && x <= rest + 0.01 => continue,
            _ => {}
        }
        if cur.is_empty() {
            line_src = t.src;
        }
        if x + t.adv > line_len + 0.5 && !cur.is_empty() {
            if is_char_in(t, NO_START) {
                // 掛在行尾（連續的標點如「。」」一起掛）
                cur.push((x, t.clone()));
                x += t.adv;
                while i < toks.len() && is_char_in(&toks[i], NO_START) && cur.len() < 400 {
                    cur.push((x, toks[i].clone()));
                    x += toks[i].adv;
                    i += 1;
                }
                finish(&mut cur, &mut lines, line_src);
                x = rest;
                continue;
            }
            // 行尾不能是開括號：把它帶到下一行
            let mut carry = Vec::new();
            while cur.len() > 1 && cur.last().is_some_and(|(_, l)| is_char_in(l, NO_END)) {
                carry.push(cur.pop().unwrap().1);
            }
            finish(&mut cur, &mut lines, line_src);
            x = rest;
            line_src = carry.last().map(|c| c.src).unwrap_or(t.src);
            for c in carry.into_iter().rev() {
                cur.push((x, c.clone()));
                x += c.adv;
            }
            if matches!(t.kind, Kind::Space) {
                continue;
            }
        }
        cur.push((x, t.clone()));
        x += t.adv;
    }
    if !cur.is_empty() || lines.is_empty() {
        finish(&mut cur, &mut lines, line_src);
    }
    lines
}

// ---------------------------------------------------------------------------
// 繪製

fn paint_paper(ui: &Ui, rect: Rect, pal: &Palette) {
    let p = ui.painter();
    let dark = ui.visuals().dark_mode;
    p.add(
        egui::Shadow {
            offset: [0, 4],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(if dark { 70 } else { 26 }),
        }
        .as_shape(rect, 2.0),
    );
    p.rect_filled(rect, 2.0, pal.paper);
}

fn paint_blank(ui: &Ui, rect: Rect, pal: &Palette) {
    paint_paper(ui, rect, pal);
}

fn paint_image(ui: &Ui, inner: Rect, path: &Path, alt: &str, cache: &ImageCache, pal: &Palette) {
    match images::get(cache, path, ui.ctx()) {
        Some(Ok((tex, [w, h]))) => {
            let size = vec2(w as f32, h as f32);
            let scale = (inner.width() / size.x).min(inner.height() / size.y);
            let rect = Rect::from_center_size(inner.center(), size * scale);
            ui.painter().image(tex, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        Some(Err(e)) => {
            let label = if alt.is_empty() { "插圖" } else { alt };
            ui.painter().text(
                inner.center(),
                Align2::CENTER_CENTER,
                format!("［{label}］\n{e}"),
                FontId::proportional(13.0),
                pal.weak,
            );
        }
        None => {
            ui.painter().text(
                inner.center(),
                Align2::CENTER_CENTER,
                "載入插圖中…",
                FontId::proportional(13.0),
                pal.weak,
            );
        }
    }
}

fn color_of(style: u8, pal: &Palette) -> Color32 {
    if style & WEAK != 0 {
        pal.weak
    } else {
        pal.ink
    }
}

fn paint_text(
    ui: &Ui,
    inner: Rect,
    layout: &Layout,
    page_lines: &[(usize, f32)],
    prefs: &NovelPrefs,
    pal: &Palette,
    links: &mut Vec<(Rect, usize)>,
) {
    let p = ui.painter();
    let vertical = prefs.vertical;
    // 行進方向置中對齊格子
    let slack = if vertical { inner.height() - layout.line_len } else { inner.width() - layout.line_len };
    let lead = (slack / 2.0).max(0.0);
    for (li, bpos) in page_lines {
        let line = &layout.lines[*li];
        let line_w = line.items.last().map(|(x, t)| x + t.adv).unwrap_or(0.0);
        let shift = if line.center { ((layout.line_len - line_w) / 2.0).max(0.0) } else { 0.0 };
        // 行中心（垂直於行進方向）與行進方向的起點
        let thick = line_size(line, prefs.font_size) * prefs.line_spacing;
        let (cross, along0) = if vertical {
            (inner.right() - bpos - thick / 2.0, inner.top() + lead + shift)
        } else {
            (inner.top() + bpos + thick / 2.0, inner.left() + lead + shift)
        };
        if let Some(target) = line.link {
            let r = if vertical {
                Rect::from_min_max(pos2(cross - thick / 2.0, along0), pos2(cross + thick / 2.0, along0 + line_w))
            } else {
                Rect::from_min_max(pos2(along0, cross - thick / 2.0), pos2(along0 + line_w, cross + thick / 2.0))
            };
            links.push((r, target));
        }
        if vertical {
            paint_vertical_line(p, line, cross, along0, pal);
        } else {
            paint_horizontal_line(p, line, cross, along0, pal);
        }
    }
}

fn paint_glyphs(p: &egui::Painter, pos: egui::Pos2, anchor: Align2, text: &str, size: f32, style: u8, pal: &Palette) {
    let color = color_of(style, pal);
    let font = FontId::proportional(size);
    p.text(pos, anchor, text, font.clone(), color);
    if style & STRONG != 0 {
        // 假粗體：錯開一點再畫一次
        p.text(pos + vec2(size * 0.035, 0.0), anchor, text, font, color);
    }
}

fn paint_horizontal_line(p: &egui::Painter, line: &Line, cy: f32, x0: f32, pal: &Palette) {
    // 同樣式的連續字合併成一串一起畫
    let mut i = 0;
    let items = &line.items;
    while i < items.len() {
        let (x, t) = &items[i];
        match &t.kind {
            Kind::Ruby { base, ruby } => {
                paint_glyphs(p, pos2(x0 + x, cy), Align2::LEFT_CENTER, base, t.size, t.style, pal);
                let rs = t.size * 0.5;
                p.text(
                    pos2(x0 + x + t.adv / 2.0, cy - t.size * 0.55),
                    Align2::CENTER_BOTTOM,
                    ruby,
                    FontId::proportional(rs),
                    color_of(t.style, pal),
                );
                i += 1;
            }
            Kind::Space | Kind::Newline => i += 1,
            _ => {
                let mut text = String::new();
                let mut j = i;
                let mut next_x = *x;
                while j < items.len() {
                    let (ux, u) = &items[j];
                    // 樣式不同或位置不連續（例如置中的場景分隔符號）就分開畫
                    if u.style != t.style || u.size != t.size || (ux - next_x).abs() > 0.5 {
                        break;
                    }
                    next_x = ux + u.adv;
                    match &u.kind {
                        Kind::Char(c) => text.push(*c),
                        Kind::Word(w) | Kind::Tcy(w) => text.push_str(w),
                        Kind::Space => text.push(' '),
                        _ => break,
                    }
                    j += 1;
                }
                paint_glyphs(p, pos2(x0 + x, cy), Align2::LEFT_CENTER, &text, t.size, t.style, pal);
                if t.style & EMPH != 0 {
                    for (ux, u) in &items[i..j] {
                        if matches!(u.kind, Kind::Char(c) if is_wide(c)) {
                            p.circle_filled(
                                pos2(x0 + ux + u.adv / 2.0, cy + u.size * 0.66),
                                (u.size * 0.07).max(1.2),
                                color_of(u.style, pal),
                            );
                        }
                    }
                }
                i = j.max(i + 1);
            }
        }
    }
}

fn paint_vertical_line(p: &egui::Painter, line: &Line, cx: f32, y0: f32, pal: &Palette) {
    for (y, t) in &line.items {
        let top = y0 + y;
        let color = color_of(t.style, pal);
        match &t.kind {
            Kind::Char(c) if CENTERED_PUNCT.contains(*c) => {
                ink_centered(p, *c, pos2(cx, top + t.adv / 2.0), t.size, color)
            }
            Kind::Char(c) if rotate_in_vertical(*c) => {
                rotated(p, &c.to_string(), pos2(cx, top + t.adv / 2.0), t.size, color)
            }
            Kind::Char(c) => {
                paint_glyphs(
                    p,
                    pos2(cx, top + t.adv / 2.0),
                    Align2::CENTER_CENTER,
                    &c.to_string(),
                    t.size,
                    t.style,
                    pal,
                );
                if t.style & EMPH != 0 {
                    p.circle_filled(pos2(cx + t.size * 0.66, top + t.adv / 2.0), (t.size * 0.07).max(1.2), color);
                }
            }
            Kind::Word(w) => rotated(p, w, pos2(cx, top + t.adv / 2.0), t.size, color),
            Kind::Tcy(w) => {
                let s = if w.chars().count() > 1 { t.size * 0.78 } else { t.size };
                paint_glyphs(p, pos2(cx, top + t.adv / 2.0), Align2::CENTER_CENTER, w, s, t.style, pal);
            }
            Kind::Ruby { base, ruby } => {
                let mut yy = top;
                for c in base.chars() {
                    paint_glyphs(
                        p,
                        pos2(cx, yy + t.size / 2.0),
                        Align2::CENTER_CENTER,
                        &c.to_string(),
                        t.size,
                        t.style,
                        pal,
                    );
                    yy += t.size;
                }
                let rs = t.size * 0.5;
                let n = ruby.chars().count() as f32;
                let mut ry = top + t.adv / 2.0 - n * rs / 2.0;
                for c in ruby.chars() {
                    let pos = pos2(cx + t.size * 0.5 + rs * 0.6, ry + rs / 2.0);
                    if rotate_in_vertical(c) {
                        rotated(p, &c.to_string(), pos, rs, color);
                    } else {
                        p.text(pos, Align2::CENTER_CENTER, c, FontId::proportional(rs), color);
                    }
                    ry += rs;
                }
            }
            Kind::Space | Kind::Newline => {}
        }
    }
}

/// 直排時置中的標點（台灣的標點習慣：句號、逗號放在格子正中央）。
const CENTERED_PUNCT: &str = "、。，．：；！？";

/// 把一個字的筆畫中心對齊 `center`。
///
/// 有些字型把逗號、句號畫在格子的左下角（橫排的位置），直排時看起來會歪掉，
/// 所以依字形實際的筆畫範圍來置中，不管用哪種字型都一樣。
fn ink_centered(p: &egui::Painter, c: char, center: egui::Pos2, size: f32, color: Color32) {
    let galley = p.layout_no_wrap(c.to_string(), FontId::proportional(size), color);
    let ink = galley.rows.first().and_then(|r| {
        r.row
            .glyphs
            .first()
            .filter(|g| !g.uv_rect.is_nothing())
            .map(|g| Rect::from_min_size(r.pos + g.pos.to_vec2() + g.uv_rect.offset, g.uv_rect.size))
    });
    let anchor = ink.map(|r| r.center()).unwrap_or(galley.rect.center());
    p.galley(center - anchor.to_vec2(), galley, color);
}

/// 把文字轉 90° 畫在 `center`（直排中的括號、西文）。
fn rotated(p: &egui::Painter, text: &str, center: egui::Pos2, size: f32, color: Color32) {
    let galley = p.layout_no_wrap(text.to_string(), FontId::proportional(size), color);
    let pos = center - galley.rect.center().to_vec2();
    let shape =
        egui::epaint::TextShape::new(pos, galley, color).with_angle_and_anchor(FRAC_PI_2, Align2::CENTER_CENTER);
    p.add(Shape::Text(shape));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(c: char, size: f32) -> f32 {
        if is_wide(c) {
            size
        } else {
            size * 0.5
        }
    }

    fn layout(md: &str, w: f32, h: f32, vertical: bool) -> Built {
        let prefs = NovelPrefs { vertical, font_size: 10.0, line_spacing: 2.0, indent: 0, ..Default::default() };
        build(&novel::blocks(md), vec2(w, h), &prefs, Path::new("."), &mut measure)
    }

    fn line_text(l: &Line) -> String {
        l.items
            .iter()
            .map(|(_, t)| match &t.kind {
                Kind::Char(c) => c.to_string(),
                Kind::Word(w) | Kind::Tcy(w) => w.clone(),
                Kind::Ruby { base, .. } => base.clone(),
                _ => String::new(),
            })
            .collect()
    }

    #[test]
    fn kinsoku() {
        // 一行 5 個字：句號不能在行首，要掛在上一行尾
        let b = layout("一二三四五。六七", 50.0, 100.0, false);
        assert_eq!(line_text(&b.lines[0]), "一二三四五。");
        assert_eq!(line_text(&b.lines[1]), "六七");
        // 開括號不能在行尾
        let b = layout("一二三四「五六」", 50.0, 100.0, false);
        assert_eq!(line_text(&b.lines[0]), "一二三四");
        assert_eq!(line_text(&b.lines[1]), "「五六」");
    }

    #[test]
    fn pagination_and_offsets() {
        // 每頁 3 行（高 60、行距 20），每行 5 字
        let text = "一二三四五六七八九十".repeat(3);
        let b = layout(&text, 50.0, 60.0, false);
        assert_eq!(b.lines.len(), 6);
        assert_eq!(b.pages.len(), 2);
        assert_eq!(b.pages[1].first_src(&b.lines), Some(15));
    }

    #[test]
    fn vertical_tcy_and_words() {
        let b = layout("第12話 Hello", 200.0, 200.0, true);
        let kinds: Vec<_> = b.lines[0].items.iter().map(|(_, t)| t.kind.clone()).collect();
        assert!(kinds.contains(&Kind::Tcy("12".into())));
        assert!(kinds.contains(&Kind::Word("Hello".into())));
    }

    #[test]
    fn images_get_own_page() {
        let b = layout("文字\n\n![](a.png)\n\n更多", 50.0, 60.0, false);
        assert_eq!(b.pages.len(), 3);
        assert!(matches!(b.pages[1], Page::Image { .. }));
    }
}
