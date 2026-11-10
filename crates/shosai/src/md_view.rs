//! GUI 的 Markdown 渲染器：把 HackMD 語法樹（`shosai_core::hmd`）畫成 egui 介面。
//!
//! 與 PDF / HTML 輸出共用同一份解析結果，所以三者支援的語法一致。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::find::find_all;
use crate::images::{self, ImageCache};
use crate::scroll_map::{self, char_offsets};
use eframe::egui::{self, Align, Color32, RichText, Stroke, TextStyle};
use shosai_core::hmd::{self, Align as CellAlign, Block, ContainerKind, Inline, MdDoc};

/// 使用者在預覽中的操作，交給 App 處理。
pub enum ViewEvent {
    /// 切換待辦清單；值為 `[ ]` 在原始文字中的位置
    ToggleTask(usize),
    /// 點了連結
    OpenLink(String),
}

/// 大綱中的一個標題。
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineItem {
    pub level: u8,
    pub text: String,
    /// 預覽中的錨點
    pub id: String,
    /// 在原始文字中是第幾個字元
    pub chars: usize,
}

#[derive(Default)]
pub struct MdView {
    source_hash: u64,
    doc: Arc<MdDoc>,
    math: HashMap<u64, MathEntry>,
    /// 要捲動到的標題 / 註腳錨點
    scroll_to: Option<String>,
    /// 本機點陣圖（背景解碼、縮小後的貼圖）
    images: ImageCache,
    /// 文件中的標題（大綱）
    outline: Vec<OutlineItem>,
    /// 每個最上層區塊在原始文字中是第幾個字元
    block_chars: Vec<usize>,
    /// 原始文字的總字元數
    text_chars: usize,
    /// 對位點：（原始文字的字元位置、畫在內容裡多高），見 [`scroll_map`]
    anchors: Vec<(f32, f32)>,
    /// 畫面最上緣對應到原始文字的第幾個字（上一次顯示時）
    pub view_top: f32,
    /// 同上，但取畫面上緣往下一點的位置：用來判斷「現在讀到哪個標題」
    pub probe: f32,
    /// 畫面最上緣在內容裡的高度（上一次顯示時）
    view_y: f32,
    /// 這一幀要把原始文字的這個位置捲到畫面最上緣（切換模式時保持位置）
    sync_to: Option<f32>,
    /// 尋找：要標示的字、是否區分大小寫
    find: Option<(String, bool)>,
    /// 目前是第幾個符合的位置；`find_jump` 時把它捲到看得到的地方
    find_current: Option<usize>,
    find_jump: bool,
    /// 畫面上符合的數量（上一次顯示時）
    pub find_count: usize,
}

const FIND_FG: Color32 = Color32::from_rgb(40, 36, 30);

/// 尋找結果的底色：目前這一個用比較深的橘色。
fn find_bg(current: bool) -> Color32 {
    if current {
        Color32::from_rgb(255, 150, 50)
    } else {
        Color32::from_rgb(255, 213, 79)
    }
}

enum MathEntry {
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    Svg {
        uri: String,
        bytes: Arc<[u8]>,
    },
    Failed(String),
}

/// 「現在讀到哪個標題」看的是畫面上緣往下這麼多的位置：
/// 標題剛好捲到最上面時算它自己，而不是上一個標題。
pub const PROBE_BELOW_TOP: f32 = 12.0;

/// 每一幀最多花多少時間排版新的數學式（其餘留到下一幀，避免卡住介面）。
const MATH_BUDGET: Duration = Duration::from_millis(40);

struct Ctx<'a> {
    doc: &'a MdDoc,
    base_uri: String,
    events: Vec<ViewEvent>,
    math: &'a mut HashMap<u64, MathEntry>,
    math_deadline: Instant,
    math_pending: bool,
    scroll_to: &'a mut Option<String>,
    body: f32,
    list_depth: usize,
    base_dir: &'a Path,
    images: &'a ImageCache,
    find: Option<&'a (String, bool)>,
    /// 到目前為止畫了幾個符合的位置
    find_seen: usize,
    find_current: Option<usize>,
    find_jump: bool,
}

impl Ctx<'_> {
    /// 把一段文字依尋找結果切開：（文字、是否符合；符合時再分是不是目前這一個）。
    fn split<'t>(&mut self, t: &'t str) -> Vec<(&'t str, Option<bool>)> {
        let hits = match self.find {
            Some((query, case)) => find_all(t, query, *case),
            None => Vec::new(),
        };
        if hits.is_empty() {
            return vec![(t, None)];
        }
        let mut out = Vec::with_capacity(hits.len() * 2 + 1);
        let mut pos = 0;
        for m in hits {
            if m.bytes.start > pos {
                out.push((&t[pos..m.bytes.start], None));
            }
            out.push((&t[m.bytes.clone()], Some(self.next_hit())));
            pos = m.bytes.end;
        }
        if pos < t.len() {
            out.push((&t[pos..], None));
        }
        out
    }

    /// 記一個符合的位置；回傳它是不是「目前這一個」。
    fn next_hit(&mut self) -> bool {
        let current = self.find_current == Some(self.find_seen);
        self.find_seen += 1;
        current
    }

    /// 目前這一個符合的位置：需要的話捲到畫面中間。
    fn reveal(&mut self, resp: &egui::Response, hit: Option<bool>) {
        if hit == Some(true) && self.find_jump {
            resp.scroll_to_me(Some(Align::Center));
            self.find_jump = false;
        }
    }
}

/// 行內樣式狀態。
#[derive(Clone, Default)]
struct Style {
    strong: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    mark: bool,
    raised: bool,
    small: bool,
    link: Option<String>,
    size: Option<f32>,
}

impl MdView {
    /// 解析 Markdown（內容沒變就不重做）。大綱在不顯示預覽的編輯模式也要用，所以獨立出來。
    pub fn parse(&mut self, source: &str) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        let hash = hasher.finish();
        if hash == self.source_hash && self.source_hash != 0 {
            return;
        }
        let doc = hmd::parse(source);
        let headings = doc.headings();
        let offsets: Vec<usize> = headings.iter().map(|h| h.offset).collect();
        self.outline = headings
            .into_iter()
            .zip(char_offsets(source, &offsets))
            .map(|(h, chars)| OutlineItem { level: h.level, text: h.text, id: h.id, chars })
            .collect();
        self.block_chars = char_offsets(source, &doc.offsets);
        self.text_chars = source.chars().count();
        self.doc = Arc::new(doc);
        self.source_hash = hash;
    }

    pub fn outline(&self) -> &[OutlineItem] {
        &self.outline
    }

    /// 下一次顯示時，把原始文字第 `source` 個字捲到畫面最上緣。
    ///
    /// 只適合捲一次（切換模式）：egui 依差量捲動要隔兩幀才生效，連續呼叫會重複計算。
    /// 要每一幀都跟著捲（分割模式同步）請用 [`MdView::scroll_delta_to`] 自己設定捲動位置。
    pub fn sync_to(&mut self, source: f32) {
        self.sync_to = Some(source);
    }

    /// 依上一次顯示的版面：要讓原始文字第 `source` 個字在畫面最上緣，還要往下捲多少。
    pub fn scroll_delta_to(&self, source: f32) -> f32 {
        scroll_map::y_at(&self.anchors, source) - self.view_y
    }

    /// 設定要標示的尋找結果。`current` 是目前第幾個，`jump` 時把它捲到看得到的地方。
    pub fn set_find(&mut self, find: Option<(&str, bool)>, current: Option<usize>, jump: bool) {
        match (find, &mut self.find) {
            (Some((q, case)), Some((old, old_case))) if old == q && *old_case == case => {}
            (Some((q, case)), _) => self.find = Some((q.to_string(), case)),
            (None, _) => self.find = None,
        }
        self.find_current = current;
        self.find_jump = jump;
    }

    /// 顯示 Markdown。`base_dir` 用來解析相對路徑的圖片。
    pub fn show(&mut self, ui: &mut egui::Ui, source: &str, base_dir: &Path) -> Vec<ViewEvent> {
        self.parse(source);
        let doc = self.doc.clone();
        let find = self.find.clone();
        let mut cx = Ctx {
            doc: &doc,
            base_uri: format!("file://{}/", base_dir.to_string_lossy().replace('\\', "/")),
            events: Vec::new(),
            math: &mut self.math,
            math_deadline: Instant::now() + MATH_BUDGET,
            math_pending: false,
            scroll_to: &mut self.scroll_to,
            body: ui.style().text_styles.get(&TextStyle::Body).map(|f| f.size).unwrap_or(14.0),
            list_depth: 0,
            base_dir,
            images: &self.images,
            find: find.as_ref(),
            find_seen: 0,
            find_current: self.find_current,
            find_jump: self.find_jump,
        };
        // 記下每個最上層區塊畫在多高（相對於內容頂端），之後才能和原始文字的位置對起來
        let origin = ui.cursor().top();
        let mut tops = Vec::with_capacity(doc.blocks.len());
        let inner = ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = cx.body * 0.6;
            for b in &doc.blocks {
                tops.push(ui.cursor().top() - origin);
                block(ui, b, &mut cx);
            }
            if !doc.footnotes.is_empty() {
                ui.add_space(cx.body * 1.5);
                ui.label(RichText::new("註腳").size(cx.body * 0.8).weak());
                for f in &doc.footnotes {
                    let id = format!("fn-{}", f.number);
                    ui.horizontal_top(|ui| {
                        let r = ui.label(RichText::new(format!("{}.", f.number)).weak());
                        scroll_if_target(&r, &id, cx.scroll_to);
                        ui.vertical(|ui| blocks(ui, &f.blocks, &mut cx));
                    });
                }
            }
        });
        if cx.math_pending {
            ui.ctx().request_repaint();
        }
        let events = std::mem::take(&mut cx.events);
        self.find_count = cx.find_seen;
        self.find_jump = false;

        // 對位點：每個區塊一個，最後加上文件結尾
        let bottom = inner.response.rect.bottom() - origin;
        self.anchors.clear();
        // 第一個區塊之前的原始文字（例如 front matter）不會畫出來：在畫面上高度是 0
        if self.block_chars.first().is_some_and(|c| *c > 0) {
            self.anchors.push((0.0, 0.0));
        }
        self.anchors.extend(self.block_chars.iter().zip(&tops).map(|(c, y)| (*c as f32, *y)));
        self.anchors.push((self.text_chars as f32, bottom.max(tops.last().copied().unwrap_or(0.0))));
        let view_y = ui.clip_rect().top() - origin;
        self.view_y = view_y;
        self.view_top = scroll_map::source_at(&self.anchors, view_y);
        self.probe = scroll_map::source_at(&self.anchors, view_y + PROBE_BELOW_TOP);
        if let Some(source) = self.sync_to.take() {
            let delta = scroll_map::y_at(&self.anchors, source) - view_y;
            if delta.abs() > 0.5 {
                // 正值是把內容往下推，所以要往下捲時給負值
                ui.scroll_with_delta_animation(egui::vec2(0.0, -delta), egui::style::ScrollAnimation::none());
                ui.ctx().request_repaint();
            }
        }

        // 清掉太舊的數學式快取（避免一直編輯時無限增長）
        if self.math.len() > 2000 {
            self.math.clear();
        }
        events
    }

    /// 捲動到某個標題（`[TOC]` 或文件內連結）。
    pub fn scroll_to(&mut self, anchor: &str) {
        self.scroll_to = Some(anchor.trim_start_matches('#').to_string());
    }
}

fn scroll_if_target(resp: &egui::Response, id: &str, target: &mut Option<String>) {
    if target.as_deref() == Some(id) {
        resp.scroll_to_me(Some(Align::TOP));
        *target = None;
    }
}

fn blocks(ui: &mut egui::Ui, bs: &[Block], cx: &mut Ctx) {
    for b in bs {
        block(ui, b, cx);
    }
}

fn block(ui: &mut egui::Ui, b: &Block, cx: &mut Ctx) {
    let body = cx.body;
    match b {
        Block::Paragraph(inl) => paragraph(ui, inl, cx, Style::default()),
        Block::Heading { level, content, id } => {
            let scale = match level {
                1 => 1.8,
                2 => 1.45,
                3 => 1.25,
                4 => 1.1,
                _ => 1.0,
            };
            ui.add_space(body * if *level <= 2 { 0.5 } else { 0.2 });
            let resp = ui
                .scope(|ui| {
                    paragraph(ui, content, cx, Style { strong: true, size: Some(body * scale), ..Style::default() });
                })
                .response;
            scroll_if_target(&resp, id, cx.scroll_to);
        }
        Block::Quote(bs) => {
            let color = ui.visuals().weak_text_color();
            let resp = egui::Frame::new()
                .inner_margin(egui::Margin { left: 14, right: 4, top: 2, bottom: 2 })
                .show(ui, |ui| {
                    ui.visuals_mut().override_text_color = Some(color);
                    blocks(ui, bs, cx);
                })
                .response;
            let r = resp.rect;
            ui.painter().vline(
                r.left() + 3.0,
                r.y_range(),
                Stroke::new(3.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
            );
        }
        Block::Code { lang, code, line_start } => code_block(ui, lang, code, *line_start, cx),
        Block::Math(tex) => {
            ui.vertical_centered(|ui| math(ui, tex, true, cx));
        }
        Block::List { start, items } => {
            cx.list_depth += 1;
            let old_spacing = ui.spacing().item_spacing.y;
            ui.spacing_mut().item_spacing.y = body * 0.2;
            let bullet = ["•", "◦", "▪"][(cx.list_depth - 1) % 3];
            for (i, item) in items.iter().enumerate() {
                let is_task =
                    matches!(item.first(), Some(Block::Paragraph(p)) if matches!(p.first(), Some(Inline::Task { .. })));
                ui.horizontal_top(|ui| {
                    let marker = match start {
                        Some(n) => format!("{}.", n + i as u64),
                        None if is_task => String::new(),
                        None => bullet.to_string(),
                    };
                    let w = if start.is_some() { body * 1.6 } else { body * 1.0 };
                    ui.add_sized([w, body * 1.3], egui::Label::new(RichText::new(marker).size(body)));
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = body * 0.3;
                        blocks(ui, item, cx);
                    });
                });
            }
            cx.list_depth -= 1;
            ui.spacing_mut().item_spacing.y = old_spacing;
        }
        Block::Table { aligns, header, rows } => {
            egui::Frame::new()
                .fill(ui.visuals().faint_bg_color)
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    egui::Grid::new(ui.next_auto_id()).striped(false).spacing([body, body * 0.4]).show(ui, |ui| {
                        for (i, cell) in header.iter().enumerate() {
                            table_cell(ui, cell, aligns.get(i), true, cx);
                        }
                        ui.end_row();
                        for row in rows {
                            for (i, cell) in row.iter().enumerate() {
                                table_cell(ui, cell, aligns.get(i), false, cx);
                            }
                            ui.end_row();
                        }
                    });
                });
        }
        Block::Rule => {
            // 分隔線：置中的短線，而非整行
            let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), body), egui::Sense::hover());
            let c = rect.center();
            let half = (rect.width() * 0.15).min(80.0);
            ui.painter().line_segment(
                [egui::pos2(c.x - half, c.y), egui::pos2(c.x + half, c.y)],
                Stroke::new(1.0, ui.visuals().weak_text_color()),
            );
        }
        Block::Container { kind: ContainerKind::Spoiler, title, open, blocks: bs } => {
            let title = if title.is_empty() { "詳細內容" } else { title.as_str() };
            egui::CollapsingHeader::new(RichText::new(title).strong().size(body))
                .id_salt(ui.next_auto_id())
                .default_open(*open)
                .show(ui, |ui| blocks(ui, bs, cx));
        }
        Block::Container { kind, title, blocks: bs, .. } => {
            let (bg, border, fg) = container_colors(*kind, ui.visuals().dark_mode);
            egui::Frame::new()
                .fill(bg)
                .stroke(Stroke::new(1.0, border.gamma_multiply(0.6)))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(12, 9))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.visuals_mut().override_text_color = Some(fg);
                    if !title.is_empty() {
                        ui.label(RichText::new(title).strong().size(body));
                    }
                    blocks(ui, bs, cx);
                });
        }
        Block::DefList(items) => {
            for item in items {
                paragraph(ui, &item.term, cx, Style { strong: true, ..Style::default() });
                for d in &item.defs {
                    ui.horizontal_top(|ui| {
                        ui.add_space(body * 2.0);
                        ui.vertical(|ui| blocks(ui, d, cx));
                    });
                }
            }
        }
        Block::Toc => {
            let headings = cx.doc.headings();
            let min = headings.iter().map(|h| h.level).min().unwrap_or(1);
            egui::Frame::new()
                .fill(ui.visuals().faint_bg_color)
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.label(RichText::new("目錄").size(body * 0.8).weak());
                    ui.spacing_mut().item_spacing.y = 3.0;
                    for h in headings {
                        ui.horizontal(|ui| {
                            ui.add_space((h.level - min) as f32 * body * 1.2);
                            if ui.link(RichText::new(&h.text).size(body * 0.95)).clicked() {
                                *cx.scroll_to = Some(h.id.clone());
                            }
                        });
                    }
                });
        }
        Block::PageBreak => {
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("· 分頁 ·").weak().small());
            });
        }
        Block::Html(_) => {}
    }
}

fn container_colors(kind: ContainerKind, dark: bool) -> (Color32, Color32, Color32) {
    let (bg, border, fg) = match kind {
        ContainerKind::Info => ((217, 237, 247), (188, 232, 241), (49, 112, 143)),
        ContainerKind::Success => ((223, 240, 216), (214, 233, 198), (60, 118, 61)),
        ContainerKind::Warning => ((252, 248, 227), (250, 235, 204), (138, 109, 59)),
        ContainerKind::Danger | ContainerKind::Spoiler => ((242, 222, 222), (235, 204, 209), (169, 68, 66)),
    };
    let c = |(r, g, b): (u8, u8, u8)| Color32::from_rgb(r, g, b);
    if dark {
        // 深色模式：背景壓暗、文字提亮
        let darken = |(r, g, b): (u8, u8, u8)| Color32::from_rgb(r / 5, g / 5, b / 5);
        let lighten = |(r, g, b): (u8, u8, u8)| {
            Color32::from_rgb(r.saturating_add(110), g.saturating_add(110), b.saturating_add(110))
        };
        (darken(bg), c(fg).gamma_multiply(0.6), lighten(fg))
    } else {
        (c(bg), c(border), c(fg))
    }
}

fn table_cell(ui: &mut egui::Ui, cell: &[Inline], align: Option<&CellAlign>, header: bool, cx: &mut Ctx) {
    let layout = match align {
        Some(CellAlign::Center) => egui::Layout::top_down(Align::Center),
        Some(CellAlign::Right) => egui::Layout::top_down(Align::Max),
        _ => egui::Layout::top_down(Align::Min),
    };
    ui.with_layout(layout, |ui| {
        // 表格儲存格不自動換行，讓欄寬依內容決定
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            inlines(ui, cell, cx, &Style { strong: header, ..Style::default() });
        });
    });
}

fn code_block(ui: &mut egui::Ui, lang: &str, code: &str, line_start: Option<usize>, cx: &mut Ctx) {
    let theme = egui_extras::syntax_highlighting::CodeTheme::from_style(ui.style());
    egui::Frame::new()
        .fill(ui.visuals().code_bg_color)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !lang.is_empty() {
                ui.label(RichText::new(lang).weak().small());
            }
            ui.horizontal_top(|ui| {
                if let Some(start) = line_start {
                    let n = code.lines().count().max(1);
                    let numbers: String = (start..start + n).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
                    ui.label(RichText::new(numbers).monospace().weak());
                }
                let mut job = egui_extras::syntax_highlighting::highlight(ui.ctx(), ui.style(), &theme, code, lang);
                // 尋找結果：在語法上色的結果上再疊底色
                let hits = match cx.find {
                    Some((query, case)) if job.text == code => find_all(code, query, *case),
                    _ => Vec::new(),
                };
                let mut current_here = false;
                let marks: Vec<(std::ops::Range<usize>, bool)> = hits
                    .into_iter()
                    .map(|m| {
                        let current = cx.next_hit();
                        current_here |= current;
                        (m.bytes, current)
                    })
                    .collect();
                mark_job(&mut job, &marks);
                let resp = ui.add(egui::Label::new(job).wrap());
                cx.reveal(&resp, current_here.then_some(true));
            });
        });
}

/// 在排好樣式的文字上，把 `marks`（位元組範圍、是否為目前這一個）的部分加上尋找結果的底色。
fn mark_job(job: &mut egui::text::LayoutJob, marks: &[(std::ops::Range<usize>, bool)]) {
    use egui::text::{ByteIndex, LayoutSection};
    if marks.is_empty() {
        return;
    }
    let mut out = Vec::with_capacity(job.sections.len() + marks.len() * 2);
    for sec in std::mem::take(&mut job.sections) {
        let (start, end) = (sec.byte_range.start.0, sec.byte_range.end.0);
        // 縮排只屬於這一段的第一小段
        let mut leading = sec.leading_space;
        let mut push = |a: usize, b: usize, format: egui::TextFormat| {
            out.push(LayoutSection {
                leading_space: std::mem::take(&mut leading),
                byte_range: ByteIndex(a)..ByteIndex(b),
                format,
            });
        };
        let mut pos = start;
        for (m, current) in marks {
            let (a, b) = (m.start.max(pos), m.end.min(end));
            if a >= b {
                continue;
            }
            if a > pos {
                push(pos, a, sec.format.clone());
            }
            let mut format = sec.format.clone();
            format.background = find_bg(*current);
            format.color = FIND_FG;
            push(a, b, format);
            pos = b;
        }
        if pos < end {
            push(pos, end, sec.format.clone());
        }
    }
    job.sections = out;
}

fn paragraph(ui: &mut egui::Ui, inl: &[Inline], cx: &mut Ctx, style: Style) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, cx.body * 0.25);
        inlines(ui, inl, cx, &style);
    });
}

fn inlines(ui: &mut egui::Ui, inl: &[Inline], cx: &mut Ctx, style: &Style) {
    for (i, x) in inl.iter().enumerate() {
        match x {
            Inline::SoftBreak => {
                // 中文之間不加空白
                let prev_cjk = matches!(i.checked_sub(1).and_then(|j| inl.get(j)), Some(Inline::Text(t)) if t.chars().last().is_some_and(shosai_core::markdown::is_cjk));
                let next_cjk = matches!(inl.get(i + 1), Some(Inline::Text(t)) if t.chars().next().is_some_and(shosai_core::markdown::is_cjk));
                if !(prev_cjk && next_cjk) {
                    text(ui, " ", cx, style);
                }
            }
            other => inline(ui, other, cx, style),
        }
    }
}

fn styled(text: &str, style: &Style, cx: &Ctx, ui: &egui::Ui) -> RichText {
    let mut size = style.size.unwrap_or(cx.body);
    if style.small || style.raised {
        size *= 0.75;
    }
    let mut r = RichText::new(text).size(size);
    if style.strong {
        r = r.strong();
    }
    if style.italic {
        r = r.italics();
    }
    if style.strike {
        r = r.strikethrough();
    }
    if style.underline {
        r = r.underline();
    }
    if style.raised {
        r = r.raised();
    }
    if style.mark {
        r = r.background_color(Color32::from_rgb(255, 238, 140)).color(Color32::from_rgb(40, 36, 30));
    }
    if style.link.is_some() {
        r = r.color(ui.visuals().hyperlink_color);
    }
    r
}

fn text(ui: &mut egui::Ui, t: &str, cx: &mut Ctx, style: &Style) {
    for (piece, hit) in cx.split(t) {
        let mut rich = styled(piece, style, cx, ui);
        if let Some(current) = hit {
            rich = rich.background_color(find_bg(current)).color(FIND_FG);
        }
        let resp = match &style.link {
            Some(url) => {
                let resp = ui.add(egui::Link::new(rich)).on_hover_text(url);
                if resp.clicked() {
                    cx.events.push(ViewEvent::OpenLink(url.clone()));
                }
                resp
            }
            None => ui.add(egui::Label::new(rich)),
        };
        cx.reveal(&resp, hit);
    }
}

fn inline(ui: &mut egui::Ui, x: &Inline, cx: &mut Ctx, style: &Style) {
    let with = |f: fn(&mut Style)| {
        let mut s = style.clone();
        f(&mut s);
        s
    };
    match x {
        Inline::Text(t) => text(ui, t, cx, style),
        Inline::Code(t) => {
            let size = style.size.unwrap_or(cx.body) * 0.9;
            for (piece, hit) in cx.split(t) {
                let mut rich = RichText::new(piece).code().size(size);
                if let Some(current) = hit {
                    rich = rich.background_color(find_bg(current)).color(FIND_FG);
                }
                let resp = ui.add(egui::Label::new(rich));
                cx.reveal(&resp, hit);
            }
        }
        Inline::Math { tex, display } => {
            if *display {
                ui.end_row();
                math(ui, tex, true, cx);
                ui.end_row();
            } else {
                math(ui, tex, false, cx);
            }
        }
        Inline::Emph(c) => inlines(ui, c, cx, &with(|s| s.italic = true)),
        Inline::Strong(c) => inlines(ui, c, cx, &with(|s| s.strong = true)),
        Inline::Strike(c) => inlines(ui, c, cx, &with(|s| s.strike = true)),
        Inline::Mark(c) => inlines(ui, c, cx, &with(|s| s.mark = true)),
        Inline::Ins(c) => inlines(ui, c, cx, &with(|s| s.underline = true)),
        Inline::Sup(c) => inlines(ui, c, cx, &with(|s| s.raised = true)),
        Inline::Sub(c) => inlines(ui, c, cx, &with(|s| s.small = true)),
        Inline::Link { url, children, .. } => {
            let mut s = style.clone();
            s.link = Some(url.clone());
            inlines(ui, children, cx, &s);
        }
        Inline::Image { url, alt, width, height, .. } if images::is_local_raster(url) => {
            raster_image(ui, url, alt, *width, *height, cx);
        }
        Inline::Image { url, alt, width, height, .. } => {
            let uri = if url.contains("://") || url.starts_with("data:") {
                url.clone()
            } else {
                format!("{}{}", cx.base_uri, url.trim_start_matches("./"))
            };
            let max_w = ui.available_width().max(50.0);
            // 以原始大小顯示（不受文字列高度限制），再限制最大寬度
            let mut img = egui::Image::new(uri).fit_to_original_size(1.0).max_width(max_w).show_loading_spinner(true);
            match (width, height) {
                (Some(w), Some(h)) => img = img.fit_to_exact_size(egui::vec2(*w as f32, *h as f32)),
                (Some(w), None) => img = img.max_width((*w as f32).min(max_w)),
                (None, Some(h)) => img = img.max_height(*h as f32),
                _ => {}
            }
            let resp = ui.add(img);
            if !alt.is_empty() {
                resp.on_hover_text(alt);
            }
        }
        Inline::FootnoteRef { number, .. } => {
            let rich = RichText::new(format!("[{number}]")).size(cx.body * 0.75).raised();
            if ui.add(egui::Link::new(rich)).clicked() {
                *cx.scroll_to = Some(format!("fn-{number}"));
            }
        }
        Inline::Task { checked, offset } => {
            let mut c = *checked;
            if ui.checkbox(&mut c, "").changed() {
                cx.events.push(ViewEvent::ToggleTask(*offset));
            }
        }
        Inline::SoftBreak => text(ui, " ", cx, style),
        Inline::HardBreak => ui.end_row(),
        Inline::Html(_) => {}
    }
}

/// 本機點陣圖：在背景解碼並縮小（見 [`images`]）。
fn raster_image(ui: &mut egui::Ui, url: &str, alt: &str, width: Option<u32>, height: Option<u32>, cx: &mut Ctx) {
    let path = images::resolve(cx.base_dir, url);
    let state = images::get(cx.images, &path, ui.ctx());
    let max_w = ui.available_width().max(50.0);
    match state {
        Some(Ok((tex, [w, h]))) => {
            let aspect = h as f32 / w.max(1) as f32;
            let mut size = match (width, height) {
                (Some(w), Some(h)) => egui::vec2(w as f32, h as f32),
                (Some(w), None) => egui::vec2(w as f32, w as f32 * aspect),
                (None, Some(h)) => egui::vec2(h as f32 / aspect.max(0.001), h as f32),
                _ => egui::vec2(w as f32, h as f32),
            };
            if size.x > max_w {
                size *= max_w / size.x;
            }
            let resp = ui.add(egui::Image::new((tex, size)));
            if !alt.is_empty() {
                resp.on_hover_text(alt);
            }
        }
        Some(Err(e)) => {
            let label = if alt.is_empty() { "圖片" } else { alt };
            ui.label(RichText::new(format!("[{label}]")).color(ui.visuals().weak_text_color())).on_hover_text(e);
        }
        None => {
            ui.add(egui::Spinner::new().size(cx.body));
        }
    }
}

/// 數學式：以 Typst 排版成 SVG 後顯示（結果會快取）。
fn math(ui: &mut egui::Ui, tex: &str, display: bool, cx: &mut Ctx) {
    let size_px = cx.body * if display { 1.15 } else { 1.0 };
    let color = ui.visuals().text_color();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (tex, display, (size_px * 10.0) as u32, color.to_array()).hash(&mut h);
    let key = h.finish();

    if !cx.math.contains_key(&key) {
        if Instant::now() > cx.math_deadline {
            cx.math_pending = true;
            ui.add(egui::Spinner::new().size(size_px));
            return;
        }
        cx.math.insert(key, render_math(tex, display, size_px, color, key));
    }
    match &cx.math[&key] {
        MathEntry::Svg { uri, bytes } => {
            let img = egui::Image::from_bytes(uri.clone(), egui::load::Bytes::Shared(bytes.clone()))
                .fit_to_original_size(1.0);
            ui.add(img).on_hover_text(RichText::new(tex).monospace());
        }
        MathEntry::Failed(err) => {
            let d = if display { "$$" } else { "$" };
            ui.label(RichText::new(format!("{d}{tex}{d}")).monospace().color(ui.visuals().error_fg_color))
                .on_hover_text(err);
        }
    }
}

#[cfg(feature = "pdf")]
fn render_math(tex: &str, display: bool, size_px: f32, color: Color32, key: u64) -> MathEntry {
    let hex = format!("{:02x}{:02x}{:02x}", color.r(), color.g(), color.b());
    // SVG 以 pt 為單位，egui 以 1pt = 1.333px 換算，所以這裡乘 0.75
    match shosai_core::math::render_svg(tex, display, size_px * 0.75, &hex) {
        Ok(svg) => MathEntry::Svg { uri: format!("bytes://math/{key:016x}.svg"), bytes: Arc::from(svg.into_bytes()) },
        Err(e) => MathEntry::Failed(e),
    }
}

#[cfg(not(feature = "pdf"))]
fn render_math(_tex: &str, _display: bool, _size_px: f32, _color: Color32, _key: u64) -> MathEntry {
    MathEntry::Failed("此版本未內建 Typst 引擎，無法顯示數學式".into())
}
