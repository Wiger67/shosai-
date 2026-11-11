//! 書本模式：以資料夾為單位，像輕小說一樣一頁一頁翻著讀。
//!
//! - 直排（由右至左）或橫排，單頁或雙頁跨頁
//! - 封面、目錄頁，章首換頁，插圖獨立成頁
//! - 行首／行尾禁則（句號、逗號不會出現在行首，開括號不會留在行尾）
//! - 注音（`｜漢字《かんじ》`）、著重號（`*強調*`）、直排時兩位數字橫向並排（縱中橫）
//! - 記住每本書讀到的位置（以字數記錄，改變字級或視窗大小也不會跑掉）
//! - 子資料夾是「卷」：目錄依卷分組、可收合，並統計每章、每卷與全書的字數
//! - 翻頁動畫：翻動的那張紙沿直線對摺過去，只用平移和裁切，不需要另外的貼圖

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::f32::consts::FRAC_PI_2;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver};
use std::time::SystemTime;

use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, CursorIcon, FontId, Key, Rangef, Rect, RichText, Sense, Shape, Stroke,
    TextWrapMode, Ui, Vec2,
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
    /// 翻頁時播放翻頁動畫
    pub turn_anim: bool,
}

impl Default for NovelPrefs {
    fn default() -> Self {
        Self {
            vertical: true,
            spread: true,
            font_size: 18.0,
            line_spacing: 1.8,
            indent: 2,
            para_gap: false,
            turn_anim: true,
        }
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
/// 觸控板（以點數回報的捲動）滑多少點翻一頁
const WHEEL_POINTS_PER_PAGE: f32 = 120.0;
/// 翻頁動畫的長度（秒）
const TURN_SECS: f64 = 0.28;
/// 距離上一次翻頁不到這麼久又翻（秒）：直接換頁，不播動畫，連續快翻才跟得上手
const TURN_RAPID_SECS: f64 = 0.4;

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

/// 畫面上攤開的一面：哪一份排版的第幾頁（雙頁時是兩頁中的第一頁）。
#[derive(Clone)]
struct View {
    layout: Rc<Layout>,
    unit: Unit,
    page: usize,
}

/// 進行中的翻頁動畫。
struct Turn {
    /// 翻頁之前的那一面
    from: View,
    forward: bool,
    spread: bool,
    /// 開始的時間與要翻到哪裡。第一次畫的時候才定：新的一章排版花的時間不算進動畫裡
    start: Option<(f64, (Unit, usize))>,
}

/// 畫頁面時共用的東西。
struct Draw<'a> {
    p: egui::Painter,
    prefs: &'a NovelPrefs,
    pal: &'a Palette,
    margin: Vec2,
    content: Vec2,
    spread: bool,
    dark: bool,
}

impl Draw<'_> {
    /// 只畫在 `clip` 範圍內。
    fn clipped(&self, clip: Rect) -> Draw<'_> {
        Draw { p: self.p.with_clip_rect(clip), ..*self }
    }
}

/// 書本模式的檢視器。
pub struct NovelView {
    pub book: NovelBook,
    unit: Unit,
    page: usize,
    want: Want,
    layout: Option<Rc<Layout>>,
    turn: Option<Turn>,
    /// 這一幀的時間、上一次翻頁的時間
    now: f64,
    turned_at: f64,
    /// 要不要播翻頁動畫（設定），以及這一幀是不是按住按鍵連續翻（不播）
    animate: bool,
    hurry: bool,
    /// 目前這一章的內容（路徑、修改時間、段落）
    blocks: Option<(PathBuf, Option<SystemTime>, Vec<NBlock>)>,
    pub show_toc: bool,
    /// 目錄要捲到目前這一章：`Some(Center)` 置中（剛打開目錄），`None` 只在看不到時才捲（翻到下一章）
    toc_scroll: Option<Option<Align>>,
    /// 目錄中收合起來的卷
    collapsed: HashSet<String>,
    /// 各章字數：路徑 →（檔案修改時間、字數）。由背景執行緒統計
    words: HashMap<PathBuf, WordStat>,
    words_rx: Option<Receiver<(PathBuf, WordStat)>>,
    /// 需要（重新）統計字數：剛開啟、章節清單或檔案內容有變動
    recount: bool,
    images: ImageCache,
    /// 滾輪累積量，以「頁」為單位（滿 1 翻一頁）
    wheel: f32,
    /// 上一次滾輪事件的時間（隔太久的殘量不算）
    wheel_at: f64,
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
            turn: None,
            now: 0.0,
            turned_at: f64::NEG_INFINITY,
            animate: true,
            hurry: false,
            blocks: None,
            show_toc: false,
            toc_scroll: Some(Some(Align::Center)),
            collapsed: HashSet::new(),
            words: HashMap::new(),
            words_rx: None,
            recount: true,
            images: ImageCache::default(),
            wheel: 0.0,
            wheel_at: 0.0,
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
        self.recount = true;
    }

    /// 書庫裡有檔案變動（章節清單沒變，但內容可能改了）：重新確認各章字數。
    pub fn files_changed(&mut self) {
        self.recount = true;
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
            // 目錄跟著走：展開這一章所在的卷，看不到的話捲過去
            self.expand_current_volume();
            if self.toc_scroll.is_none() {
                self.toc_scroll = Some(None);
            }
        }
        self.want = want;
        self.apply_want();
    }

    fn expand_current_volume(&mut self) {
        if let Unit::Chapter(i) = self.unit {
            if let Some(v) = self.book.chapters.get(i).and_then(|c| c.volume.as_ref()) {
                self.collapsed.remove(v);
            }
        }
    }

    /// 跳到下一章的開頭。
    fn next_chapter(&mut self) {
        let next = match self.unit {
            Unit::Front => 0,
            Unit::Chapter(i) => i + 1,
        };
        if next < self.book.chapters.len() {
            self.go(Unit::Chapter(next), Want::First);
        } else {
            self.end_flash = Some(std::time::Instant::now());
        }
    }

    /// 回到這一章的開頭；已經在開頭的話回到上一章。
    fn prev_chapter(&mut self) {
        match self.unit {
            _ if self.page > 0 => self.page = 0,
            Unit::Chapter(0) => self.go(Unit::Front, Want::First),
            Unit::Chapter(i) => self.go(Unit::Chapter(i - 1), Want::First),
            Unit::Front => {}
        }
    }

    fn step(&self, spread: bool) -> usize {
        if spread {
            2
        } else {
            1
        }
    }

    fn next_page(&mut self, spread: bool) {
        self.turn_page(spread, true);
    }

    fn prev_page(&mut self, spread: bool) {
        self.turn_page(spread, false);
    }

    /// 翻一頁，並視情況開始翻頁動畫。
    fn turn_page(&mut self, spread: bool, forward: bool) {
        let before = (self.unit, self.page);
        let from = self.layout.clone().map(|layout| View { layout, unit: self.unit, page: self.page });
        if forward {
            self.step_forward(spread);
        } else {
            self.step_back(spread);
        }
        if (self.unit, self.page) == before {
            return;
        }
        // 連續快翻（按住按鍵、滾輪連轉）時直接換頁：動畫還沒播完又要翻，只會拖慢
        let animate = self.animate && !self.hurry && self.now - self.turned_at > TURN_RAPID_SECS;
        self.turned_at = self.now;
        self.turn = from.filter(|_| animate).map(|from| Turn { from, forward, spread, start: None });
    }

    fn step_forward(&mut self, spread: bool) {
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

    fn step_back(&mut self, spread: bool) {
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

    // -----------------------------------------------------------------------
    // 字數

    /// 在背景統計各章字數。只重讀修改時間變了的檔案，所以檔案樹每次更新都可以放心呼叫。
    fn start_count(&mut self, ctx: &egui::Context) {
        let jobs: Vec<(PathBuf, Option<Option<SystemTime>>)> =
            self.book.chapters.iter().map(|c| (c.path.clone(), self.words.get(&c.path).map(|w| w.stamp))).collect();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("novel-words".into()).spawn(move || {
            for (path, known) in jobs {
                let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                if known == Some(stamp) {
                    continue;
                }
                let words = std::fs::read_to_string(&path).map(|t| novel::word_count(&t)).unwrap_or(0);
                // 畫面已經關掉（或開始了新一輪統計）就不用再算了
                if tx.send((path, WordStat { stamp, words })).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
            // 讓畫面知道統計結束了
            ctx.request_repaint();
        });
        self.words_rx = spawned.is_ok().then_some(rx);
    }

    fn poll_words(&mut self) {
        let Some(rx) = &self.words_rx else { return };
        loop {
            match rx.try_recv() {
                Ok((path, stat)) => {
                    self.words.insert(path, stat);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.words_rx = None;
                    break;
                }
            }
        }
    }

    fn chapter_words(&self, i: usize) -> Option<usize> {
        self.book.chapters.get(i).and_then(|c| self.words.get(&c.path)).map(|w| w.words)
    }

    /// 一段章節的字數總和；其中還有沒統計完的章時為 `None`。
    fn words_in(&self, chapters: Range<usize>) -> Option<usize> {
        chapters.map(|i| self.chapter_words(i)).sum()
    }

    fn total_words(&self) -> Option<usize> {
        self.words_in(0..self.book.chapters.len())
    }

    /// 第 `i` 章所在的那一組：連續、同一卷（同一個子資料夾）的章節。
    fn volume_range(&self, i: usize) -> Range<usize> {
        let chapters = &self.book.chapters;
        let Some(volume) = chapters.get(i).map(|c| &c.volume) else { return i..i };
        let start = chapters[..i].iter().rposition(|c| &c.volume != volume).map_or(0, |p| p + 1);
        let end = chapters[i..].iter().position(|c| &c.volume != volume).map_or(chapters.len(), |p| i + p);
        start..end
    }

    /// 全書進度（0–1）。字數統計好了就依字數計算（章節長短不一時才準），否則先以章數估計。
    fn progress(&self) -> f32 {
        let n = self.book.chapters.len().max(1) as f32;
        let pages = self.layout.as_ref().map(|l| l.pages.len()).unwrap_or(1).max(1) as f32;
        match self.unit {
            Unit::Front => 0.0,
            Unit::Chapter(i) => {
                let in_chapter = (self.page as f32 + 1.0) / pages;
                match (self.words_in(0..i), self.chapter_words(i), self.total_words()) {
                    (Some(before), Some(this), Some(total)) if total > 0 => {
                        ((before as f32 + this as f32 * in_chapter) / total as f32).min(1.0)
                    }
                    _ => (i as f32 + in_chapter) / n,
                }
            }
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
        if pages == 0 {
            return (left, String::new());
        }
        let mut right = vec![format!("{} / {} 頁", self.page + 1, pages)];
        match self.unit {
            Unit::Front => right.push(format!("共 {} 章", self.book.chapters.len())),
            Unit::Chapter(i) => {
                if let Some(w) = self.chapter_words(i) {
                    right.push(format!("本章 {} 字", group_digits(w)));
                }
                // 這一章屬於某一卷（子資料夾）時，也列出整卷的字數
                if self.book.chapters[i].volume.is_some() {
                    if let Some(w) = self.words_in(self.volume_range(i)) {
                        right.push(format!("本卷 {} 字", group_digits(w)));
                    }
                }
            }
        }
        right.push(match self.total_words() {
            Some(w) => format!("全書 {} 字", group_digits(w)),
            None => "全書字數統計中…".to_string(),
        });
        if self.unit != Unit::Front {
            right.push(format!("{:.0}%", self.progress() * 100.0));
        }
        (left, right.join("　·　"))
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
            ui.checkbox(&mut prefs.turn_anim, "翻頁動畫");
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
        self.now = ui.input(|i| i.time);
        self.animate = prefs.turn_anim;
        self.poll_words();
        if std::mem::take(&mut self.recount) {
            self.start_count(ui.ctx());
        }
        if !self.show_toc {
            // 下次打開目錄時，目前這一章要看得到
            self.toc_scroll = Some(Some(Align::Center));
            self.expand_current_volume();
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

        // 輸入（按住按鍵連續翻頁時不播動畫）
        self.hurry = ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Key { repeat: true, .. })));
        self.input(ui, &resp, prefs, spread, events);
        self.hurry = false;
        if self.layout.is_none() {
            // 翻到另一章了：馬上排版，這一幀就畫得出新的一頁（翻頁動畫也要用）
            self.ensure_layout(ui, content, prefs);
            if spread {
                self.page &= !1;
            }
        }
        let Some(layout) = self.layout.clone() else { return };
        let here = View { layout, unit: self.unit, page: self.page };

        // 頁面（直排由右至左：第一頁在右）。`far` 放第一頁，`near` 放第二頁（翻動的那一張）
        let first = Rect::from_min_size(top_left, page_size);
        let second = first.translate(vec2(page_w + gutter, 0.0));
        let (far, near) = match (spread, prefs.vertical) {
            (false, _) => (first, None),
            (true, true) => (second, Some(first)),
            (true, false) => (first, Some(second)),
        };
        let draw = Draw { p: ui.painter().clone(), prefs, pal, margin, content, spread, dark: ui.visuals().dark_mode };
        paint_paper(&draw, far);
        if let Some(near) = near {
            paint_paper(&draw, near);
        }
        let mut links: Vec<(Rect, usize)> = Vec::new();
        let turning = self.turn_progress(ui, &here, spread, resp.clicked());
        match (turning, &self.turn) {
            (Some(t), Some(turn)) => {
                // a 是前面的那一面，b 是後面的那一面；往回翻就是同一段動畫倒著播
                let (a, b, tau) = if turn.forward { (&turn.from, &here, t) } else { (&here, &turn.from, 1.0 - t) };
                self.paint_turn(&draw, far, near, a, b, tau);
            }
            _ => {
                self.paint_page(&draw, far, &here, here.page, &mut links);
                if let Some(near) = near {
                    self.paint_page(&draw, near, &here, here.page + 1, &mut links);
                }
            }
        }
        if spread {
            // 書脊的陰影
            let x = first.right();
            let shade = Color32::from_black_alpha(if draw.dark { 60 } else { 22 });
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
        if self.turn.is_some() {
            // 只有動畫播放中才連續重畫；播完就停
            ui.ctx().request_repaint();
        }
    }

    /// 翻頁動畫播到哪了（0–1，已經套用緩動）。沒有在播、播完了、或該中止時回傳 `None`。
    fn turn_progress(&mut self, ui: &Ui, here: &View, spread: bool, interrupt: bool) -> Option<f32> {
        let turn = self.turn.as_mut()?;
        let (now, dt) = ui.input(|i| (i.time, f64::from(i.predicted_dt)));
        let at = (here.unit, here.page);
        // 第一幀就要看得到動靜，所以從「已經過了一幀」算起
        let (start, to) = *turn.start.get_or_insert((now - dt.min(TURN_SECS / 4.0), at));
        let t = ((now - start) / TURN_SECS) as f32;
        let (old, new) = (&turn.from.layout.key, &here.layout.key);
        // 中途跳到別處、按了滑鼠、版面變了（視窗大小、字級、單雙頁）：直接結束
        let stale = to != at || turn.spread != spread || old.grid != new.grid || old.prefs != new.prefs;
        if t >= 1.0 || interrupt || stale {
            self.turn = None;
            return None;
        }
        // 一開始快、收尾慢：按下去馬上有反應，落下時不突兀
        Some(1.0 - (1.0 - t).powi(3))
    }

    /// 把 `view` 的第 `idx` 頁畫在 `rect`（紙張另外畫）。超過最後一頁就是空白頁。
    fn paint_page(&self, d: &Draw, rect: Rect, view: &View, idx: usize, links: &mut Vec<(Rect, usize)>) {
        let Some(page) = view.layout.pages.get(idx) else { return };
        let inner = Rect::from_min_size(rect.min + d.margin, d.content);
        match page {
            Page::Cover => self.paint_cover(d, rect),
            Page::Image { path, alt } => paint_image(d, inner, path, alt, &self.images),
            Page::Text(lines) => paint_text(d, inner, &view.layout, lines, links),
        }
        // 頁首（書名 / 章名）與頁碼
        if matches!(page, Page::Cover) {
            return;
        }
        // 雙頁時：一邊印書名、另一邊印章名（像實體書的書眉）
        let head = if d.spread && (idx % 2 == 1) == d.prefs.vertical {
            self.book.title.as_str()
        } else {
            match view.unit {
                Unit::Front => "目　錄",
                Unit::Chapter(i) => self.book.chapters.get(i).map_or("", |c| c.title.as_str()),
            }
        };
        let small = FontId::proportional((d.prefs.font_size * 0.62).clamp(10.0, 13.0));
        let x = rect.center().x;
        d.p.text(pos2(x, rect.top() + d.margin.y * 0.5), Align2::CENTER_CENTER, head, small.clone(), d.pal.weak);
        d.p.text(pos2(x, rect.bottom() - d.margin.y * 0.5), Align2::CENTER_CENTER, idx + 1, small, d.pal.weak);
    }

    /// 翻頁途中的畫面。`a` 是前面的那一面、`b` 是後面的那一面，
    /// `tau` 是翻過去多少（0 = 還是 a，1 = 已經是 b）。
    ///
    /// 把翻動的那張紙當成沿著一條直線對摺：摺線從書口往裝訂邊移動，摺過去的那一段蓋在對面，
    /// 露出紙的背面（b 的第一頁），摺線後方露出底下的新頁（b 的第二頁）。
    /// 畫面上的每一塊都只是「某一頁平移之後裁切」，文字不必縮放變形，也不需要先畫到貼圖上，
    /// 所以動畫中每一幀要畫的字和平常差不多。
    fn paint_turn(&self, d: &Draw, far: Rect, near: Option<Rect>, a: &View, b: &View, tau: f32) {
        let mut none = Vec::new();
        // 翻動的那張紙：書口（自由邊）在 edge，裝訂邊在 hinge。單頁時裝訂邊就是頁面的另一側
        let leaf = near.unwrap_or(far);
        let (edge, hinge) = if d.prefs.vertical { (leaf.left(), leaf.right()) } else { (leaf.right(), leaf.left()) };
        let dir = (hinge - edge).signum();
        // 對齊實體像素：平移量帶小數的話，字會在動畫中一格一格地抖
        let ppp = d.p.pixels_per_point();
        let fold = ((edge + (hinge - edge) * tau.clamp(0.0, 1.0)) * ppp).round() / ppp;
        // 摺過去那一段的自由邊現在在哪；過了一半就會越過裝訂邊
        let tip = 2.0 * fold - edge;
        let beyond = (tip - hinge) * dir > 0.0;
        let y = leaf.y_range();
        let span = |x0: f32, x1: f32| Rect::from_x_y_ranges(x0.min(x1)..=x0.max(x1), y);
        // 雙頁時翻動的是每一面的第二頁
        let second = usize::from(near.is_some());

        // 摺線後方：底下露出來的新頁
        self.paint_page(&d.clipped(span(edge, fold)), leaf, b, b.page + second, &mut none);
        // 還平放著、也還沒被蓋住的部分
        if !beyond {
            self.paint_page(&d.clipped(span(tip, hinge)), leaf, a, a.page + second, &mut none);
        }
        // 對面那一頁：沒被蓋住的部分
        if near.is_some() {
            let outer = 2.0 * hinge - edge;
            self.paint_page(&d.clipped(span(if beyond { tip } else { hinge }, outer)), far, a, a.page, &mut none);
        }
        // 摺過去的那一段：紙的背面。雙頁時就是 b 的第一頁，從它最後的位置平移過來
        let flap = span(fold, if near.is_none() && beyond { hinge } else { tip });
        let on_flap = d.clipped(flap);
        // 紙翻得多高：頭尾都是 0，所以動畫開始和結束時畫面不會突然跳一下
        let rise = (tau * std::f32::consts::PI).sin().max(0.0);
        if near.is_some() {
            // 深色的紙上影子不明顯，讓翻起來的那一段稍微亮一點（離光比較近）
            let lit = if d.dark { 0.05 * rise } else { 0.0 };
            on_flap.p.rect_filled(flap, 0.0, d.pal.paper.lerp_to_gamma(Color32::WHITE, lit));
            self.paint_page(&on_flap, far.translate(vec2(2.0 * (fold - hinge), 0.0)), b, b.page, &mut none);
        } else {
            // 單頁：背面留白，顏色比紙面暗一點
            on_flap.p.rect_filled(flap, 0.0, d.pal.paper.lerp_to_gamma(d.pal.desk, 0.45));
        }

        // 影子：紙翻得越高越深
        let lift = rise * if d.dark { 2.0 } else { 1.0 };
        let shadow = |alpha: f32| Color32::from_black_alpha((alpha * lift).min(160.0) as u8);
        let book = d.clipped(far.union(leaf));
        // 翻起來的紙落在新頁上的影子
        shade(&book.p, fold, fold - dir * 40.0, y, shadow(46.0), Color32::TRANSPARENT);
        // 摺過去的那一段：靠摺線的地方暗一點，看起來有弧度
        shade(&on_flap.p, fold, fold + dir * (flap.width() * 0.5).min(70.0), y, shadow(30.0), Color32::TRANSPARENT);
        // 自由邊落在底下那一頁的影子
        shade(&book.p, tip, tip + dir * 28.0, y, shadow(52.0), Color32::TRANSPARENT);
        if d.dark {
            // 紙的邊緣再加一道反光，才看得出哪裡是翻起來的那一張
            let glint = Color32::from_white_alpha((lift * 14.0).min(40.0) as u8);
            book.p.vline(tip - dir * 0.5, y, Stroke::new(1.0, glint));
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
            // Ctrl+PageDown／PageUp：下一章／上一章（和閱讀模式一樣）
            let (next_ch, prev_ch) = ui.input(|i| {
                (i.modifiers.command && i.key_pressed(Key::PageDown), i.modifiers.command && i.key_pressed(Key::PageUp))
            });
            if next_ch {
                self.next_chapter();
            }
            if prev_ch {
                self.prev_chapter();
            }
            let (fwd_h, back_h, fwd, back, home, end, esc, toc) = ui.input(|i| {
                (
                    i.key_pressed(Key::ArrowRight),
                    i.key_pressed(Key::ArrowLeft),
                    (i.key_pressed(Key::PageDown) && !i.modifiers.command)
                        || i.key_pressed(Key::ArrowDown)
                        || (i.key_pressed(Key::Space) && !i.modifiers.shift),
                    (i.key_pressed(Key::PageUp) && !i.modifiers.command)
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
            // 直接讀原始的滾輪事件：滑鼠滾輪一格就翻一頁，不必等平滑捲動慢慢累積。
            // （Ctrl+滾輪是調整字級，不翻頁）
            let (lines, points, now) = ui.input(|i| {
                let (mut lines, mut points) = (0.0, 0.0);
                if !i.modifiers.command {
                    for e in &i.events {
                        let egui::Event::MouseWheel { unit, delta, .. } = e else { continue };
                        let d = if delta.y.abs() >= delta.x.abs() {
                            delta.y
                        } else if prefs.vertical {
                            delta.x
                        } else {
                            -delta.x
                        };
                        match unit {
                            egui::MouseWheelUnit::Point => points += d,
                            _ => lines += d,
                        }
                    }
                }
                (lines, points, i.time)
            });
            if lines != 0.0 || points != 0.0 {
                if now - self.wheel_at > 0.4 {
                    self.wheel = 0.0;
                }
                self.wheel_at = now;
                let pages = wheel_pages(&mut self.wheel, lines, points);
                for _ in 0..pages.abs() {
                    // 正值是內容往下移＝往回翻
                    if pages < 0 {
                        self.next_page(spread);
                    } else {
                        self.prev_page(spread);
                    }
                }
            }
        }
    }

    fn toc_panel(&mut self, ui: &mut Ui, pal: &Palette) {
        ui.label(RichText::new(&self.book.title).strong().size(15.0).color(pal.ink));
        if !self.book.author.is_empty() {
            ui.label(RichText::new(&self.book.author).small().color(pal.weak));
        }
        let n = self.book.chapters.len();
        let summary = match self.total_words() {
            Some(w) => format!("共 {n} 章　·　{} 字", group_digits(w)),
            None => format!("共 {n} 章　·　字數統計中…"),
        };
        ui.label(RichText::new(summary).small().color(pal.weak));
        ui.add_space(6.0);
        let mut goto = None;
        let mut toggle = None;
        let scroll = self.toc_scroll;
        let mut scrolled = false;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let front = TocRow { text: "封面與目錄", selected: self.unit == Unit::Front, ..Default::default() };
            if toc_row(ui, pal, front).clicked() {
                goto = Some(Unit::Front);
            }
            // 一卷（一個子資料夾）一組：標題列可以收合，右邊是整卷的字數
            let mut i = 0;
            while i < n {
                let range = self.volume_range(i);
                let volume = self.book.chapters[i].volume.as_deref();
                let open = volume.is_none_or(|v| !self.collapsed.contains(v));
                if let Some(v) = volume {
                    ui.add_space(4.0);
                    let header = TocRow {
                        text: v,
                        trailing: self.words_in(range.clone()).map(|w| format!("{} 字", group_digits(w))),
                        chevron: Some(open),
                        strong: true,
                        ..Default::default()
                    };
                    let r = toc_row(ui, pal, header).on_hover_text(format!("{v}　（{} 章）", range.len()));
                    if r.clicked() {
                        toggle = Some(v.to_string());
                    }
                }
                if open {
                    for j in range.clone() {
                        let ch = &self.book.chapters[j];
                        let current = self.unit == Unit::Chapter(j);
                        let row = TocRow {
                            text: &ch.title,
                            trailing: self.chapter_words(j).map(group_digits),
                            selected: current,
                            indent: if volume.is_some() { 14.0 } else { 0.0 },
                            ..Default::default()
                        };
                        let r = toc_row(ui, pal, row);
                        if current {
                            if let Some(align) = scroll {
                                r.scroll_to_me(align);
                                scrolled = true;
                            }
                        }
                        if r.clicked() {
                            goto = Some(Unit::Chapter(j));
                        }
                    }
                }
                i = range.end.max(i + 1);
            }
        });
        if scrolled || self.unit == Unit::Front {
            self.toc_scroll = None;
        }
        if let Some(v) = toggle {
            if !self.collapsed.remove(&v) {
                self.collapsed.insert(v);
            }
        }
        if let Some(u) = goto {
            self.go(u, Want::First);
        }
    }

    fn paint_cover(&self, d: &Draw, rect: Rect) {
        let (p, pal) = (&d.p, d.pal);
        if let Some(cover) = &self.book.cover {
            if let Some(Ok((tex, [w, h]))) = images::get(&self.images, cover, p.ctx()) {
                let size = vec2(w as f32, h as f32);
                let scale = (rect.width() / size.x).max(rect.height() / size.y);
                let shown = size * scale;
                // 填滿整頁（超出的部分裁掉），像實體書的封面
                let uv_w = (rect.width() / shown.x).min(1.0);
                let uv_h = (rect.height() / shown.y).min(1.0);
                let uv = Rect::from_center_size(pos2(0.5, 0.5), vec2(uv_w, uv_h));
                p.image(tex, rect, uv, Color32::WHITE);
                return;
            }
        }
        // 沒有封面圖：用書名排一個簡單的封面
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
            match self.total_words() {
                Some(w) => format!("共 {} 章　·　{} 字", self.book.chapters.len(), group_digits(w)),
                None => format!("共 {} 章", self.book.chapters.len()),
            },
            FontId::proportional(12.0),
            pal.weak,
        );
    }

    // -----------------------------------------------------------------------
    // 排版

    fn ensure_layout(&mut self, ui: &Ui, content: egui::Vec2, prefs: &NovelPrefs) {
        // 章節檔案：讀取並快取段落
        let chapter = self.current_chapter().map(Path::to_path_buf);
        let stamp = match (&self.turn, &self.layout) {
            // 翻頁動畫播放中不必每一幀都去問檔案系統（播完的那一幀會再確認）
            (Some(_), Some(l)) if l.key.unit == self.unit => l.key.stamp,
            _ => chapter.as_deref().and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok()),
        };
        if let Some(path) = &chapter {
            let stale = self.blocks.as_ref().is_none_or(|(p, s, _)| p != path || *s != stamp);
            if stale {
                let blocks = match std::fs::read_to_string(path) {
                    Ok(text) => {
                        // 反正都讀進來了，順便更新這一章的字數
                        self.words.insert(path.clone(), WordStat { stamp, words: novel::word_count(&text) });
                        novel::blocks(&text)
                    }
                    Err(e) => novel::blocks(&format!("無法讀取這一章：{e}")),
                };
                self.blocks = Some((path.clone(), stamp, blocks));
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
        let blocks: Cow<[NBlock]> = match self.unit {
            Unit::Front => Cow::Owned(toc_blocks(&self.book)),
            Unit::Chapter(_) => self.blocks.as_ref().map(|(_, _, b)| Cow::Borrowed(b.as_slice())).unwrap_or_default(),
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
        self.layout =
            Some(Rc::new(Layout { key, lines: layout.lines, pages: layout.pages, line_len: layout.line_len }));
        self.apply_want();
    }
}

/// 一章的字數，連同統計時檔案的修改時間（檔案改過才需要重算）。
#[derive(Debug, Clone, Copy, PartialEq)]
struct WordStat {
    stamp: Option<SystemTime>,
    words: usize,
}

/// 加上千分位：`1234567` → `1,234,567`。
fn group_digits(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 側邊目錄的一列（文件大綱也用同一種列）。
#[derive(Default)]
pub(crate) struct TocRow<'a> {
    pub text: &'a str,
    /// 右側的淡色資訊（字數）
    pub trailing: Option<String>,
    pub selected: bool,
    pub indent: f32,
    /// 卷的標題列：展開／收合的箭頭
    pub chevron: Option<bool>,
    pub strong: bool,
}

pub(crate) fn toc_row(ui: &mut Ui, pal: &Palette, row: TocRow) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = ui.painter();
    if row.selected {
        p.rect_filled(rect, 6.0, pal.selected);
        let bar = Rect::from_min_size(rect.min + vec2(0.0, 6.0), vec2(3.0, rect.height() - 12.0));
        p.rect_filled(bar, 1.5, pal.accent);
    } else if resp.is_pointer_button_down_on() {
        p.rect_filled(rect, 6.0, pal.selected);
    } else if resp.hovered() {
        p.rect_filled(rect, 6.0, pal.hover);
    }
    let cy = rect.center().y;
    let mut x = rect.left() + 8.0 + row.indent;
    if let Some(open) = row.chevron {
        let r = Rect::from_center_size(pos2(x + 4.0, cy), vec2(10.0, 10.0));
        icons::paint(p, r, if open { Icon::ChevronDown } else { Icon::ChevronRight }, pal.weak);
        x += 16.0;
    }
    let mut right = rect.right() - 8.0;
    if let Some(t) = row.trailing {
        let galley = p.layout_no_wrap(t, FontId::proportional(11.5), pal.weak);
        right -= galley.size().x;
        p.galley(pos2(right, cy - galley.size().y / 2.0), galley, pal.weak);
        right -= 8.0;
    }
    let text = if row.strong { RichText::new(row.text).strong() } else { RichText::new(row.text) };
    let color = if row.selected || row.strong { pal.ink } else { ui.visuals().text_color() };
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(TextWrapMode::Truncate),
        (right - x).max(10.0),
        egui::TextStyle::Body,
    );
    p.galley(pos2(x, cy - galley.size().y / 2.0), galley, color);
    resp
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

fn paint_paper(d: &Draw, rect: Rect) {
    let (p, dark) = (&d.p, d.dark);
    p.add(
        egui::Shadow {
            offset: [0, 4],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(if dark { 70 } else { 26 }),
        }
        .as_shape(rect, 2.0),
    );
    p.rect_filled(rect, 2.0, d.pal.paper);
}

/// 水平方向的漸層（影子）：`x0` 處是 `c0`，`x1` 處是 `c1`。
fn shade(p: &egui::Painter, x0: f32, x1: f32, y: Rangef, c0: Color32, c1: Color32) {
    if x0 == x1 || (c0.a() == 0 && c1.a() == 0) {
        return;
    }
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(pos2(x0, y.min), c0);
    mesh.colored_vertex(pos2(x1, y.min), c1);
    mesh.colored_vertex(pos2(x1, y.max), c1);
    mesh.colored_vertex(pos2(x0, y.max), c0);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(Shape::mesh(mesh));
}

fn paint_image(d: &Draw, inner: Rect, path: &Path, alt: &str, cache: &ImageCache) {
    let (p, pal) = (&d.p, d.pal);
    match images::get(cache, path, p.ctx()) {
        Some(Ok((tex, [w, h]))) => {
            let size = vec2(w as f32, h as f32);
            let scale = (inner.width() / size.x).min(inner.height() / size.y);
            let rect = Rect::from_center_size(inner.center(), size * scale);
            p.image(tex, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        Some(Err(e)) => {
            let label = if alt.is_empty() { "插圖" } else { alt };
            p.text(
                inner.center(),
                Align2::CENTER_CENTER,
                format!("［{label}］\n{e}"),
                FontId::proportional(13.0),
                pal.weak,
            );
        }
        None => {
            p.text(inner.center(), Align2::CENTER_CENTER, "載入插圖中…", FontId::proportional(13.0), pal.weak);
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

fn paint_text(d: &Draw, inner: Rect, layout: &Layout, page_lines: &[(usize, f32)], links: &mut Vec<(Rect, usize)>) {
    let (p, prefs, pal) = (&d.p, d.prefs, d.pal);
    let vertical = prefs.vertical;
    let clip = p.clip_rect();
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
        // 裁切範圍外的行不必畫：翻頁動畫中每一塊只露出一部分，直排時整行整行地省下來
        let across = Rangef::new(cross - thick, cross + thick);
        let along = Rangef::new(along0 - thick, along0 + line_w + thick);
        let (xs, ys) = if vertical { (across, along) } else { (along, across) };
        if !clip.x_range().intersects(xs) || !clip.y_range().intersects(ys) {
            continue;
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

/// 把這一幀的滾輪量加進累積量 `acc`（單位：頁），回傳現在要翻幾頁（負值往後翻，正值往回翻）。
///
/// `lines` 是滑鼠滾輪的格數：一格一頁，同一幀滾了幾格就翻幾頁。
/// `points` 是觸控板回報的點數：一次滑動會送出很多點數（還有慣性），所以一次最多翻一頁。
fn wheel_pages(acc: &mut f32, lines: f32, points: f32) -> i32 {
    *acc += lines + points / WHEEL_POINTS_PER_PAGE;
    if lines != 0.0 {
        let pages = acc.trunc();
        *acc -= pages;
        pages as i32
    } else if acc.abs() >= 1.0 {
        let pages = acc.signum() as i32;
        *acc = 0.0;
        pages
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(1234567), "1,234,567");
    }

    fn chapter(name: &str, volume: Option<&str>) -> novel::NovelChapter {
        novel::NovelChapter { path: PathBuf::from(name), title: name.into(), volume: volume.map(str::to_string) }
    }

    fn view(chapters: Vec<novel::NovelChapter>, words: &[usize]) -> NovelView {
        let book =
            NovelBook { root: PathBuf::from("."), title: "書".into(), author: String::new(), cover: None, chapters };
        let mut v = NovelView::open(book, None);
        for (c, w) in v.book.chapters.iter().zip(words) {
            v.words.insert(c.path.clone(), WordStat { stamp: None, words: *w });
        }
        v
    }

    #[test]
    fn volumes_and_word_totals() {
        let chapters = vec![
            chapter("序", None),
            chapter("一之一", Some("第一卷")),
            chapter("一之二", Some("第一卷")),
            chapter("二之一", Some("第二卷")),
        ];
        let mut v = view(chapters, &[100, 2000, 3000]);
        assert_eq!(v.volume_range(0), 0..1);
        assert_eq!(v.volume_range(1), 1..3);
        assert_eq!(v.volume_range(2), 1..3);
        assert_eq!(v.volume_range(3), 3..4);
        assert_eq!(v.words_in(v.volume_range(2)), Some(5000));
        // 第二卷還沒統計完：整卷與全書都還不知道
        assert_eq!(v.words_in(v.volume_range(3)), None);
        assert_eq!(v.total_words(), None);

        v.words.insert(PathBuf::from("二之一"), WordStat { stamp: None, words: 4900 });
        assert_eq!(v.total_words(), Some(10_000));

        // 進度依字數：讀完第一卷第二章（單頁的章）= (100 + 2000 + 3000) / 10000
        v.unit = Unit::Chapter(2);
        assert!((v.progress() - 0.51).abs() < 1e-6, "{}", v.progress());
        v.unit = Unit::Chapter(3);
        assert_eq!(v.progress(), 1.0);
    }

    #[test]
    fn chapter_jumps_follow_volumes() {
        let chapters = vec![chapter("一之一", Some("第一卷")), chapter("二之一", Some("第二卷"))];
        let mut v = view(chapters, &[]);
        v.collapsed.insert("第二卷".into());
        v.next_chapter();
        assert_eq!(v.unit, Unit::Chapter(0));
        v.next_chapter();
        assert_eq!(v.unit, Unit::Chapter(1));
        assert!(!v.collapsed.contains("第二卷"), "moving into a collapsed volume expands it");
        v.next_chapter();
        assert_eq!(v.unit, Unit::Chapter(1), "stays on the last chapter");
        assert!(v.end_flash.is_some());
        v.prev_chapter();
        assert_eq!(v.unit, Unit::Chapter(0));
        v.prev_chapter();
        assert_eq!(v.unit, Unit::Front);
    }

    /// End to end, headless: the background count finishes, and the whole view
    /// (pages, side table of contents, status text) renders with the totals.
    #[test]
    fn counts_words_in_background() {
        let dir = std::env::temp_dir().join(format!("shosai-novel-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("v1")).unwrap();
        let texts = [
            (
                "a.md",
                "# A

hello world again
",
            ),
            (
                "v1/b.md",
                "# B

one two
",
            ),
            (
                "v1/c.md",
                "# C

three
",
            ),
        ];
        for (name, text) in texts {
            std::fs::write(dir.join(name), text).unwrap();
        }
        let expected: usize = texts.iter().map(|(_, t)| novel::word_count(t)).sum();
        let lib = shosai_core::Library::open(&dir).unwrap();
        let mut v = NovelView::open(NovelBook::new(&dir, &lib.entries, None), None);
        v.show_toc = true;
        assert_eq!(v.total_words(), None);

        let ctx = egui::Context::default();
        let pal = crate::theme::palette(false);
        let prefs = NovelPrefs::default();
        let frame = |v: &mut NovelView| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1100.0, 700.0))),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| v.show(ui, &pal, &prefs, &mut Vec::new()));
            out.textures_delta.clear();
        };
        for _ in 0..400 {
            frame(&mut v);
            if v.words_rx.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(v.words_rx.is_none(), "the counting thread finished");
        assert_eq!(v.total_words(), Some(expected));
        assert!(v.status().1.contains(&format!("{} 字", group_digits(expected))), "{}", v.status().1);

        // into the first chapter of the sub-folder: chapter, volume and book totals all show
        let i = v.book.chapters.iter().position(|c| c.volume.is_some()).unwrap();
        v.go(Unit::Chapter(i), Want::First);
        frame(&mut v);
        let status = v.status().1;
        let volume = v.words_in(v.volume_range(i)).unwrap();
        assert_eq!(v.volume_range(i).len(), 2);
        assert!(status.contains("本章") && status.contains(&format!("本卷 {volume} 字")), "{status}");

        // a chapter edited on disk is recounted
        let path = v.book.chapters[i].path.clone();
        std::fs::write(
            &path,
            "# B

one two three four five six
",
        )
        .unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(10);
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(later).unwrap();
        v.files_changed();
        for _ in 0..400 {
            frame(&mut v);
            if v.words_rx.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(v.total_words(), Some(expected + 4));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 一本有兩章、每章好幾頁的書，不開視窗跑起來。
    struct Stage {
        ctx: egui::Context,
        v: NovelView,
        prefs: NovelPrefs,
        dark: bool,
        time: f64,
        dir: PathBuf,
        textures: crate::shot::Textures,
        /// 上一幀畫了什麼、是否要求馬上再畫一幀
        shapes: Vec<egui::epaint::ClippedShape>,
        repaint: bool,
    }

    const STAGE: egui::Vec2 = vec2(1100.0, 700.0);

    impl Stage {
        fn new(name: &str) -> Stage {
            let dir = std::env::temp_dir().join(format!("shosai-turn-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            for (file, title) in [("1.md", "第一章　出發"), ("2.md", "第二章　歸途")] {
                let mut text = format!("# {title}\n\n");
                for i in 0..40 {
                    text.push_str(&"那一天的雨下得很久，他在窗邊等了一個下午。".repeat(2 + i % 4));
                    text.push_str("「還要多久？」她問，手裡的茶已經涼了。\n\n");
                }
                std::fs::write(dir.join(file), text).unwrap();
            }
            let lib = shosai_core::Library::open(&dir).unwrap();
            let v = NovelView::open(NovelBook::new(&dir, &lib.entries, None), None);
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let mut stage = Stage {
                ctx,
                v,
                prefs: NovelPrefs::default(),
                dark: false,
                time: 0.0,
                dir,
                textures: Default::default(),
                shapes: Vec::new(),
                repaint: false,
            };
            stage.v.go(Unit::Chapter(0), Want::First);
            stage.settle();
            stage
        }

        /// 跑一幀（過了 `dt` 秒），送入這些事件。
        fn frame(&mut self, dt: f64, events: Vec<egui::Event>) {
            self.time += dt;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), STAGE)),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let pal = crate::theme::palette(self.dark);
            let (v, prefs) = (&mut self.v, &self.prefs);
            let mut out = self.ctx.run_ui(input, |ui| v.show(ui, &pal, prefs, &mut Vec::new()));
            self.textures.update(&mut out.textures_delta);
            self.repaint = out.viewport_output[&egui::ViewportId::ROOT].repaint_delay.is_zero();
            self.shapes = out.shapes;
        }

        /// 不送輸入，一直跑到畫面不再要求重畫（背景的字數統計、動畫都結束）。
        fn settle(&mut self) {
            for _ in 0..600 {
                self.frame(1.0 / 60.0, Vec::new());
                if !self.repaint && self.v.words_rx.is_none() {
                    return;
                }
                if self.v.words_rx.is_some() {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
            panic!("the view never came to rest");
        }

        /// 按下按鍵不放。同一個鍵沒放開又按一次，egui 就當成按住時系統送來的重複按鍵。
        fn hold(&mut self, key: Key) {
            self.key(key, true);
        }

        /// 按一下：這一幀按下，下一幀放開。
        fn press(&mut self, key: Key) {
            self.key(key, true);
            self.key(key, false);
        }

        fn key(&mut self, key: Key, pressed: bool) {
            let modifiers = egui::Modifiers::NONE;
            let event = egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
            self.frame(1.0 / 60.0, vec![event]);
        }

        fn at(&self) -> (Unit, usize) {
            (self.v.unit, self.v.page)
        }

        fn save(&self, name: &str) {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots").join(name);
            self.textures.save(&self.ctx, self.shapes.clone(), [STAGE.x as usize, STAGE.y as usize], &path);
        }
    }

    impl Drop for Stage {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn turning_a_page_animates_then_rests() {
        let mut s = Stage::new("basic");
        assert_eq!(s.at(), (Unit::Chapter(0), 0));
        assert!(s.v.turn.is_none() && !s.repaint, "at rest before any input");

        // 直排：← 是下一頁。按下去的那一幀就換頁，動畫只是畫面上的過場
        s.hold(Key::ArrowLeft);
        assert_eq!(s.at(), (Unit::Chapter(0), 2), "a spread turns two pages");
        assert!(s.v.turn.is_some() && s.repaint, "the animation started");
        let (mut frames, mut most) = (1, s.shapes.len());
        s.key(Key::ArrowLeft, false);
        frames += 1;
        while s.v.turn.is_some() {
            s.frame(1.0 / 60.0, Vec::new());
            assert!(s.repaint || s.v.turn.is_none(), "keeps repainting while it plays");
            most = most.max(s.shapes.len());
            frames += 1;
            assert!(frames < 60, "the animation ends");
        }
        let secs = f64::from(frames) / 60.0;
        assert!((TURN_SECS - 0.05..=TURN_SECS + 0.05).contains(&secs), "took {secs} s");
        // egui 在最後一次要求重畫之後會再多畫一幀，然後就停了
        s.frame(1.0 / 60.0, Vec::new());
        assert!(!s.repaint, "and stops repainting");
        // 動畫中要畫的東西不能比平常多太多（裁切範圍外的行都跳過了）
        let still = s.shapes.len();
        assert!(most < still * 3 / 2, "a still frame has {still} shapes, the animation peaked at {most}");

        // 往回翻也一樣
        s.frame(1.0, Vec::new());
        s.press(Key::ArrowRight);
        assert_eq!(s.at(), (Unit::Chapter(0), 0));
        assert!(s.v.turn.as_ref().is_some_and(|t| !t.forward));
        s.settle();
        assert!(s.v.turn.is_none());
    }

    #[test]
    fn rapid_turns_skip_the_animation() {
        let mut s = Stage::new("rapid");
        // 動畫還在播又翻：直接換頁
        s.press(Key::ArrowLeft);
        assert!(s.v.turn.is_some());
        s.frame(0.1, Vec::new());
        s.press(Key::ArrowLeft);
        assert_eq!(s.at(), (Unit::Chapter(0), 4));
        assert!(s.v.turn.is_none(), "the second turn snaps");
        s.settle();

        // 按住按鍵：第一下有動畫，之後系統送來的重複按鍵每一下都直接換頁，
        // 就算隔得夠久也一樣
        s.frame(1.0, Vec::new());
        s.hold(Key::ArrowLeft);
        assert!(s.v.turn.is_some());
        s.frame(1.0, Vec::new());
        s.hold(Key::ArrowLeft);
        assert_eq!(s.at(), (Unit::Chapter(0), 8));
        assert!(s.v.turn.is_none(), "a key repeat snaps");
        s.key(Key::ArrowLeft, false);

        // 停一下再翻，動畫又回來了
        s.frame(1.0, Vec::new());
        s.press(Key::ArrowRight);
        assert!(s.v.turn.is_some());

        // End 這類直接跳頁的動作會中止動畫
        s.press(Key::End);
        assert!(s.v.turn.is_none());
        s.settle();

        // 關掉設定就完全不播
        s.frame(1.0, Vec::new());
        s.prefs.turn_anim = false;
        s.press(Key::ArrowRight);
        assert!(s.v.turn.is_none());
    }

    #[test]
    fn turning_across_chapters_animates_too() {
        let mut s = Stage::new("chapters");
        let last = s.v.layout.as_ref().unwrap().pages.len() - 1;
        s.press(Key::End);
        s.settle();
        assert_eq!(s.at(), (Unit::Chapter(0), last & !1));
        // 最後一面往後翻：進到下一章，舊的那一面還留著給動畫用
        s.frame(1.0, Vec::new());
        s.press(Key::ArrowLeft);
        assert_eq!(s.at(), (Unit::Chapter(1), 0));
        let turn = s.v.turn.as_ref().expect("animates into the next chapter");
        assert_eq!((turn.from.unit, turn.from.page), (Unit::Chapter(0), last & !1));
        s.settle();
        // 往回翻：回到上一章的最後一面
        s.frame(1.0, Vec::new());
        s.press(Key::ArrowRight);
        assert_eq!(s.at(), (Unit::Chapter(0), last & !1));
        assert!(s.v.turn.is_some());
        s.settle();
        assert!(s.v.turn.is_none() && !s.repaint);
    }

    /// 把翻頁動畫的幾個瞬間存成圖片（`target/shots/`），用眼睛檢查：
    /// `cargo test -p shosai turn_shots -- --ignored`
    #[test]
    #[ignore = "writes PNGs for looking at; run by hand"]
    fn turn_shots() {
        let cases = [
            ("spread-vertical", true, true, false),
            ("spread-horizontal", false, true, false),
            ("single-vertical", true, false, false),
            ("spread-vertical-dark", true, true, true),
        ];
        for (name, vertical, spread, dark) in cases {
            let mut s = Stage::new(name);
            // 測試環境沒有中文字型（程式執行時由系統字型補上）：找得到就裝一個，圖片裡才看得到字
            if let Ok(bytes) = std::fs::read("C:/Windows/Fonts/msjh.ttc") {
                let mut fonts = egui::FontDefinitions::default();
                fonts.font_data.insert("cjk".into(), egui::FontData::from_owned(bytes).into());
                fonts.families.entry(egui::FontFamily::Proportional).or_default().push("cjk".into());
                s.ctx.set_fonts(fonts);
            }
            s.prefs.vertical = vertical;
            s.prefs.spread = spread;
            s.dark = dark;
            s.settle();
            s.save(&format!("{name}-0.png"));
            let forward = if vertical { Key::ArrowLeft } else { Key::ArrowRight };
            // 動畫全長 0.28 秒：在幾個時間點各存一張
            s.hold(forward);
            for (i, dt) in [0.02, 0.05, 0.07, 0.1].into_iter().enumerate() {
                s.frame(dt, Vec::new());
                assert!(s.v.turn.is_some(), "{name}: still turning at shot {i}");
                s.save(&format!("{name}-{}.png", i + 1));
            }
            s.settle();
            s.save(&format!("{name}-5.png"));
        }
    }

    #[test]
    fn wheel() {
        // 滑鼠滾輪：一格就翻一頁
        let mut acc = 0.0;
        assert_eq!(wheel_pages(&mut acc, -1.0, 0.0), -1);
        assert_eq!(wheel_pages(&mut acc, -1.0, 0.0), -1);
        assert_eq!(wheel_pages(&mut acc, 3.0, 0.0), 3);
        assert_eq!(acc, 0.0);
        // 高解析度滾輪：半格半格累積
        assert_eq!(wheel_pages(&mut acc, -0.5, 0.0), 0);
        assert_eq!(wheel_pages(&mut acc, -0.5, 0.0), -1);
        // 觸控板：累積到門檻才翻，而且一次只翻一頁
        assert_eq!(wheel_pages(&mut acc, 0.0, -50.0), 0);
        assert_eq!(wheel_pages(&mut acc, 0.0, -50.0), 0);
        assert_eq!(wheel_pages(&mut acc, 0.0, -900.0), -1);
        assert_eq!(acc, 0.0);
    }

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
