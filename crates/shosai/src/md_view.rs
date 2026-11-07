//! GUI 的 Markdown 渲染器：把 HackMD 語法樹（`shosai_core::hmd`）畫成 egui 介面。
//!
//! 與 PDF / HTML 輸出共用同一份解析結果，所以三者支援的語法一致。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Color32, RichText, Stroke, TextStyle};
use shosai_core::hmd::{self, Align as CellAlign, Block, ContainerKind, Inline, MdDoc};

/// 使用者在預覽中的操作，交給 App 處理。
pub enum ViewEvent {
    /// 切換待辦清單；值為 `[ ]` 在原始文字中的位置
    ToggleTask(usize),
    /// 點了連結
    OpenLink(String),
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
}

/// 點陣圖最大解碼寬度（像素）。原圖再大也只保留這個寬度，避免一張相片吃掉幾十 MB。
const MAX_IMAGE_PX: u32 = 1600;

type ImageCache = Arc<std::sync::Mutex<HashMap<std::path::PathBuf, ImageState>>>;

enum ImageState {
    Loading,
    /// 貼圖與原圖尺寸（像素）
    Ready(egui::TextureHandle, [u32; 2]),
    Failed(String),
}

enum MathEntry {
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    Svg {
        uri: String,
        bytes: Arc<[u8]>,
    },
    Failed(String),
}

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
    /// 顯示 Markdown。`base_dir` 用來解析相對路徑的圖片。
    pub fn show(&mut self, ui: &mut egui::Ui, source: &str, base_dir: &Path) -> Vec<ViewEvent> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        let hash = hasher.finish();
        if hash != self.source_hash || self.source_hash == 0 {
            self.doc = Arc::new(hmd::parse(source));
            self.source_hash = hash;
        }
        let doc = self.doc.clone();
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
        };
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = cx.body * 0.6;
            blocks(ui, &doc.blocks, &mut cx);
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
        Block::Code { lang, code, line_start } => code_block(ui, lang, code, *line_start),
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

fn code_block(ui: &mut egui::Ui, lang: &str, code: &str, line_start: Option<usize>) {
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
                let job = egui_extras::syntax_highlighting::highlight(ui.ctx(), ui.style(), &theme, code, lang);
                ui.add(egui::Label::new(job).wrap());
            });
        });
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
    let rich = styled(t, style, cx, ui);
    match &style.link {
        Some(url) => {
            let resp = ui.add(egui::Link::new(rich)).on_hover_text(url);
            if resp.clicked() {
                cx.events.push(ViewEvent::OpenLink(url.clone()));
            }
        }
        None => {
            ui.add(egui::Label::new(rich));
        }
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
            ui.add(egui::Label::new(RichText::new(t).code().size(size)));
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
        Inline::Image { url, alt, width, height, .. } if is_local_raster(url) => {
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

fn is_local_raster(url: &str) -> bool {
    if url.contains("://") || url.starts_with("data:") {
        return false;
    }
    let lower = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp"].iter().any(|e| lower.ends_with(e))
}

/// 本機點陣圖：在背景執行緒解碼並縮到最多 [`MAX_IMAGE_PX`] 寬，避免大相片佔用大量記憶體。
fn raster_image(ui: &mut egui::Ui, url: &str, alt: &str, width: Option<u32>, height: Option<u32>, cx: &mut Ctx) {
    let rel = shosai_core::export::percent_decode_pub(url.split(['?', '#']).next().unwrap_or(url));
    let path = cx.base_dir.join(rel.trim_start_matches("./"));
    let state = {
        let mut map = cx.images.lock().unwrap();
        match map.get(&path) {
            Some(ImageState::Ready(tex, size)) => Some(Ok((tex.id(), *size))),
            Some(ImageState::Failed(e)) => Some(Err(e.clone())),
            Some(ImageState::Loading) => None,
            None => {
                map.insert(path.clone(), ImageState::Loading);
                let (images, ctx, p) = (cx.images.clone(), ui.ctx().clone(), path.clone());
                std::thread::spawn(move || {
                    let result = load_raster(&p, &ctx);
                    images.lock().unwrap().insert(p, result);
                    ctx.request_repaint();
                });
                None
            }
        }
    };
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

fn load_raster(path: &Path, ctx: &egui::Context) -> ImageState {
    let img = match image::open(path) {
        Ok(i) => i,
        Err(e) => return ImageState::Failed(format!("無法讀取圖片：{e}")),
    };
    let (w, h) = (img.width(), img.height());
    let img =
        if w > MAX_IMAGE_PX { img.resize(MAX_IMAGE_PX, u32::MAX, image::imageops::FilterType::Triangle) } else { img };
    let rgba = img.to_rgba8();
    let color =
        egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
    let tex = ctx.load_texture(path.display().to_string(), color, egui::TextureOptions::LINEAR);
    ImageState::Ready(tex, [w, h])
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
