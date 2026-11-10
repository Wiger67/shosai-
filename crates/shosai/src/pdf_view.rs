//! PDF 檢視器：以 hayro（純 Rust 的 PDF 渲染器，Typst 本身也在用）在背景執行緒渲染頁面。
//!
//! - 只渲染畫面附近的頁面，依縮放重新渲染，離畫面太遠的頁面釋放記憶體
//! - 左側縮圖列：點縮圖跳頁，目前頁會標示並自動捲到可見處
//! - 工具列：上一頁／下一頁、輸入頁碼跳頁、縮放（符合寬度、符合頁面、百分比）、雙頁模式

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui::{
    self, pos2, vec2, Align, Color32, ColorImage, Rect, RichText, Sense, Stroke, TextureHandle, TextureOptions,
};

use crate::icons::{self, Icon};
use crate::theme::Palette;

/// 1pt = 4/3 px（CSS 96 dpi）
const PT_TO_PX: f32 = 96.0 / 72.0;
const PAGE_GAP: f32 = 18.0;
const MARGIN: f32 = 28.0;
/// 單頁貼圖的最大寬度（像素），避免超大頁面吃光記憶體
const MAX_PX: f32 = 4096.0;
const THUMB_W: f32 = 112.0;
const THUMB_PANEL_W: f32 = 150.0;
const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 6.0;

/// 上一個畫面的版面，Ctrl+滾輪縮放時用來以游標為中心。
struct ViewState {
    /// 每頁在內容座標中的位置
    placed: Vec<Rect>,
    offset: egui::Vec2,
    inner: Rect,
}

#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Fit {
    /// 符合寬度
    Width,
    /// 整頁顯示
    Page,
    /// 固定比例（1.0 = 100%）
    Zoom(f32),
}

#[cfg_attr(not(feature = "pdf"), allow(dead_code))]
struct Request {
    page: usize,
    px_width: u32,
    thumb: bool,
}

#[cfg_attr(not(feature = "pdf"), allow(dead_code))]
enum Response {
    Info(Vec<(f32, f32)>),
    Page { page: usize, px_width: u32, thumb: bool, image: ColorImage },
    PageFailed(usize),
    Error(String),
}

pub struct PdfView {
    pub path: PathBuf,
    /// 每頁大小（pt）
    pages: Vec<(f32, f32)>,
    loaded: bool,
    error: Option<String>,
    textures: HashMap<usize, (u32, TextureHandle)>,
    thumbs: HashMap<usize, TextureHandle>,
    pending: HashSet<(usize, bool)>,
    failed: HashSet<usize>,
    tx: Sender<Request>,
    rx: Receiver<Response>,
    pub fit: Fit,
    /// 雙頁並排（第一頁單獨當封面）
    pub spread: bool,
    pub show_thumbs: bool,
    /// 目前頁（從 0 起算）
    pub current: usize,
    jump_to: Option<usize>,
    initial_page: Option<usize>,
    last_zoom: f32,
    /// 頁碼輸入框的內容
    page_input: String,
    page_input_focused: bool,
    focus_page_input: bool,
    thumb_follow: Option<usize>,
    view: Option<ViewState>,
    scroll_keys: bool,
}

impl PdfView {
    pub fn open(path: &Path, ctx: &egui::Context, fit: Fit, show_thumbs: bool, page: usize) -> PdfView {
        let (req_tx, req_rx) = channel();
        let (resp_tx, resp_rx) = channel();
        let ctx = ctx.clone();
        let p = path.to_path_buf();
        std::thread::Builder::new()
            .name("pdf-render".into())
            .spawn(move || worker(&p, req_rx, resp_tx, ctx))
            .expect("無法建立 PDF 執行緒");
        PdfView {
            path: path.to_path_buf(),
            pages: Vec::new(),
            loaded: false,
            error: None,
            textures: HashMap::new(),
            thumbs: HashMap::new(),
            pending: HashSet::new(),
            failed: HashSet::new(),
            tx: req_tx,
            rx: resp_rx,
            fit,
            spread: false,
            show_thumbs,
            current: 0,
            jump_to: None,
            initial_page: (page > 0).then_some(page),
            last_zoom: 1.0,
            page_input: String::new(),
            page_input_focused: false,
            focus_page_input: false,
            thumb_follow: None,
            view: None,
            scroll_keys: true,
        }
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// 目前實際的縮放比例。
    pub fn effective_zoom(&self) -> f32 {
        match self.fit {
            Fit::Zoom(z) => z,
            _ => self.last_zoom,
        }
    }

    pub fn zoom_by(&mut self, factor: f32) {
        self.fit = Fit::Zoom((self.effective_zoom() * factor).clamp(MIN_ZOOM, MAX_ZOOM));
    }

    pub fn go_to(&mut self, page: usize) {
        if !self.pages.is_empty() {
            let page = page.min(self.pages.len() - 1);
            self.jump_to = Some(page);
            self.thumb_follow = Some(page);
        }
    }

    pub fn next(&mut self) {
        let step = if self.spread && self.current > 0 { 2 } else { 1 };
        self.go_to(self.current + step);
    }

    pub fn prev(&mut self) {
        let step = if self.spread && self.current > 2 { 2 } else { 1 };
        self.go_to(self.current.saturating_sub(step));
    }

    /// 讓頁碼輸入框取得焦點（Ctrl+G）。
    pub fn focus_page_input(&mut self) {
        self.focus_page_input = true;
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(resp) = self.rx.try_recv() {
            match resp {
                Response::Info(pages) => {
                    self.pages = pages;
                    self.loaded = true;
                    if let Some(p) = self.initial_page.take() {
                        self.go_to(p);
                    }
                }
                Response::Page { page, px_width, thumb, image } => {
                    self.pending.remove(&(page, thumb));
                    let name = format!("pdf-{}-{page}-{thumb}", self.path.display());
                    let tex = ctx.load_texture(name, image, TextureOptions::LINEAR);
                    if thumb {
                        self.thumbs.insert(page, tex);
                    } else {
                        self.textures.insert(page, (px_width, tex));
                    }
                }
                Response::PageFailed(page) => {
                    self.pending.remove(&(page, false));
                    self.pending.remove(&(page, true));
                    self.failed.insert(page);
                }
                Response::Error(e) => self.error = Some(e),
            }
        }
    }

    fn request(&mut self, page: usize, px_width: u32, thumb: bool) {
        if self.pending.insert((page, thumb)) {
            let _ = self.tx.send(Request { page, px_width, thumb });
        }
    }

    /// 以列（row）排版：單頁模式每列一頁；雙頁模式第一頁單獨，其後兩頁一列。
    fn rows(&self) -> Vec<Vec<usize>> {
        let n = self.pages.len();
        if !self.spread || n <= 1 {
            return (0..n).map(|i| vec![i]).collect();
        }
        let mut rows = vec![vec![0]];
        let mut i = 1;
        while i < n {
            rows.push(if i + 1 < n { vec![i, i + 1] } else { vec![i] });
            i += 2;
        }
        rows
    }

    // -----------------------------------------------------------------------
    // 工具列

    /// 左半部：縮圖開關、上一頁、頁碼輸入、下一頁。
    pub fn toolbar_left(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let thumbs_tip = if self.show_thumbs { "隱藏頁面縮圖" } else { "顯示頁面縮圖" };
        if icons::toggle(ui, Icon::Thumbnails, thumbs_tip, self.show_thumbs).clicked() {
            self.show_thumbs = !self.show_thumbs;
        }
        ui.add_space(6.0);
        let n = self.page_count();
        if n == 0 {
            return;
        }
        if icons::button(ui, Icon::ChevronLeft, "上一頁（←）").clicked() {
            self.prev();
        }
        if !self.page_input_focused {
            self.page_input = (self.current + 1).to_string();
        }
        let edit = egui::TextEdit::singleline(&mut self.page_input)
            .desired_width(38.0)
            .align(egui::Align2::CENTER_CENTER)
            .char_limit(6);
        let resp = ui.add(edit).on_hover_text("輸入頁碼後按 Enter（Ctrl+G）");
        if self.focus_page_input {
            self.focus_page_input = false;
            resp.request_focus();
            self.page_input.clear();
        }
        self.page_input_focused = resp.has_focus();
        if resp.lost_focus() {
            let entered = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if entered {
                match self.page_input.trim().parse::<usize>() {
                    Ok(p) if (1..=n).contains(&p) => self.go_to(p - 1),
                    _ => {}
                }
            }
            self.page_input = (self.current + 1).to_string();
        }
        ui.label(RichText::new(format!("/ {n}")).color(pal.weak));
        if icons::button(ui, Icon::ChevronRight, "下一頁（→）").clicked() {
            self.next();
        }
    }

    /// 右半部（由右往左排）：縮放與顯示模式。
    pub fn toolbar_right(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let spread_tip = if self.spread { "改為單頁" } else { "雙頁並排" };
        if icons::toggle(ui, Icon::Spread, spread_tip, self.spread).clicked() {
            self.spread = !self.spread;
            let c = self.current;
            self.go_to(c);
        }
        if icons::button(ui, Icon::FitPage, "整頁顯示").clicked() {
            self.fit = Fit::Page;
            let c = self.current;
            self.go_to(c);
        }
        if icons::button(ui, Icon::FitWidth, "符合寬度").clicked() {
            self.fit = Fit::Width;
            let c = self.current;
            self.go_to(c);
        }
        ui.add_space(4.0);
        if icons::button(ui, Icon::Plus, "放大（Ctrl+=）").clicked() {
            self.zoom_by(1.2);
        }
        let label = match self.fit {
            Fit::Width => format!("寬 {:.0}%", self.last_zoom * 100.0),
            Fit::Page => format!("頁 {:.0}%", self.last_zoom * 100.0),
            Fit::Zoom(z) => format!("{:.0}%", z * 100.0),
        };
        let resp = ui.add(egui::Button::new(RichText::new(label).color(pal.weak)).min_size(vec2(64.0, 24.0)));
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(120.0);
            if ui.button("符合寬度").clicked() {
                self.fit = Fit::Width;
                ui.close();
            }
            if ui.button("整頁顯示").clicked() {
                self.fit = Fit::Page;
                ui.close();
            }
            ui.separator();
            for z in [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0] {
                if ui.button(format!("{:.0}%", z * 100.0)).clicked() {
                    self.fit = Fit::Zoom(z);
                    ui.close();
                }
            }
        });
        if icons::button(ui, Icon::Minus, "縮小（Ctrl+-）").clicked() {
            self.zoom_by(1.0 / 1.2);
        }
    }

    // -----------------------------------------------------------------------
    // 內容

    /// `keys`：是否處理鍵盤捲動（側欄檔案樹正在用方向鍵時關掉）。
    pub fn show(&mut self, ui: &mut egui::Ui, pal: &Palette, keys: bool) {
        self.scroll_keys = keys;
        self.poll(ui.ctx());
        if let Some(e) = &self.error {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new(format!("無法開啟這個 PDF：{e}")).color(ui.visuals().error_fg_color));
            });
            return;
        }
        if !self.loaded {
            ui.centered_and_justified(|ui| {
                ui.spinner();
            });
            return;
        }
        self.keyboard(ui);

        if self.show_thumbs {
            egui::Panel::left(("pdf-thumbs", &self.path))
                .show_separator_line(false)
                .resizable(false)
                .exact_size(THUMB_PANEL_W)
                .frame(egui::Frame::new().fill(pal.chrome))
                .show(ui, |ui| self.thumbnails(ui, pal));
        }
        egui::CentralPanel::no_frame().frame(egui::Frame::new().fill(pal.desk)).show(ui, |ui| self.pages_view(ui));
    }

    fn keyboard(&mut self, ui: &egui::Ui) {
        if ui.ctx().memory(|m| m.focused().is_some()) {
            return;
        }
        let (prev, next, first, last) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::PageUp),
                i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::PageDown),
                i.key_pressed(egui::Key::Home),
                i.key_pressed(egui::Key::End),
            )
        });
        if prev {
            self.prev();
        }
        if next {
            self.next();
        }
        if first {
            self.go_to(0);
        }
        if last {
            self.go_to(usize::MAX);
        }
    }

    /// 依縮放計算版面：每頁在內容座標中的位置與整體內容大小。
    fn layout(&self, avail: egui::Vec2, zoom: f32) -> (Vec<Rect>, egui::Vec2) {
        let pages = &self.pages;
        let mut placed = vec![Rect::NOTHING; pages.len()];
        let content_w = avail.x.max(self.widest_row() * PT_TO_PX * zoom + 2.0 * MARGIN);
        let mut y = MARGIN;
        for row in self.rows() {
            let rw =
                row.iter().map(|&i| pages[i].0).sum::<f32>() * PT_TO_PX * zoom + PAGE_GAP * (row.len() as f32 - 1.0);
            let rh = row.iter().map(|&i| pages[i].1).fold(0.0, f32::max) * PT_TO_PX * zoom;
            let mut x = ((content_w - rw) / 2.0).max(MARGIN);
            for &i in &row {
                let size = vec2(pages[i].0, pages[i].1) * PT_TO_PX * zoom;
                // 同一列中較矮的頁面垂直置中
                let top = y + (rh - size.y) / 2.0;
                placed[i] = Rect::from_min_size(pos2(x, top), size);
                x += size.x + PAGE_GAP;
            }
            y += rh + PAGE_GAP;
        }
        (placed, vec2(content_w, y + MARGIN - PAGE_GAP))
    }

    /// 最寬一列的寬度（pt，含頁間距）。
    fn widest_row(&self) -> f32 {
        self.rows()
            .iter()
            .map(|row| {
                row.iter().map(|&i| self.pages[i].0).sum::<f32>() + PAGE_GAP / PT_TO_PX * (row.len() as f32 - 1.0)
            })
            .fold(1.0, f32::max)
    }

    fn pages_view(&mut self, ui: &mut egui::Ui) {
        let ppp = ui.ctx().pixels_per_point();
        let avail = ui.available_size();
        let widest = self.widest_row();
        let tallest = self.pages.iter().map(|p| p.1).fold(1.0, f32::max);
        // 符合寬度時不超過舒適的閱讀寬度（雙頁時加倍）
        let max_w = if self.spread { 1700.0 } else { 860.0 };
        let fit_w = (avail.x - 2.0 * MARGIN).clamp(120.0, max_w) / (widest * PT_TO_PX);
        let fit_h = (avail.y - 2.0 * MARGIN).max(120.0) / (tallest * PT_TO_PX);
        let old_zoom = match self.fit {
            Fit::Width => fit_w,
            Fit::Page => fit_w.min(fit_h),
            Fit::Zoom(z) => z,
        };

        // Ctrl+滾輪（或觸控板雙指縮放）：以游標所在位置為中心縮放
        let mut anchor = None;
        if let Some(view) = &self.view {
            let (delta, pointer) = ui.input(|i| (i.zoom_delta(), i.pointer.hover_pos()));
            if let Some(p) = pointer.filter(|p| delta != 1.0 && view.inner.contains(*p)) {
                let zoom = (old_zoom * delta).clamp(MIN_ZOOM, MAX_ZOOM);
                if zoom != old_zoom {
                    self.fit = Fit::Zoom(zoom);
                    let local = p - view.inner.min;
                    let c = (local + view.offset).to_pos2();
                    // 游標所在（或最接近）的頁面，記下游標在頁面中的相對位置
                    let dist = |r: &Rect| (r.top() - c.y).max(c.y - r.bottom()).max(0.0);
                    let nearest = view.placed.iter().enumerate().min_by(|a, b| dist(a.1).total_cmp(&dist(b.1)));
                    if let Some((i, r)) = nearest {
                        anchor = Some((i, (c - r.min) / r.size(), local));
                    }
                }
            }
        }
        let zoom = match self.fit {
            Fit::Zoom(z) => z,
            _ => old_zoom,
        };
        self.last_zoom = zoom;
        let (placed, content) = self.layout(avail, zoom);

        // 滑鼠也可以直接拖曳頁面移動（放大後左右移動用）
        let mut area = egui::ScrollArea::both()
            .id_salt(("pdf", &self.path))
            .auto_shrink([false, false])
            .scroll_source(egui::scroll_area::ScrollSource::ALL);
        if let Some((i, uv, local)) = anchor {
            let r = placed[i];
            let c = r.min + uv * r.size();
            area = area.scroll_offset((c - local).to_vec2().max(egui::Vec2::ZERO));
        }
        let scroll_keys = self.scroll_keys;
        let out = area.show_viewport(ui, |ui, viewport| {
            let origin = ui.min_rect().min;
            ui.set_min_size(content);
            if scroll_keys {
                // ↑ ↓ 與空白鍵捲動；PageUp／PageDown、Home／End 是整頁跳（見 keyboard）
                crate::keys::scroll(ui, Rect::from_min_size(origin, content), false);
            }

            // 預先渲染上下各半個畫面；超出 1.5 個畫面的頁面釋放貼圖（每頁約 4MB）
            let near = viewport.expand2(vec2(0.0, viewport.height() * 0.5));
            let far = viewport.expand2(vec2(0.0, viewport.height() * 1.5));
            // 目前頁：畫面上方三分之一處所在的頁
            let probe = viewport.top() + viewport.height() * 0.33;
            if let Some(i) = placed.iter().position(|r| r.top() - PAGE_GAP <= probe && probe <= r.bottom()) {
                if self.current != i && self.jump_to.is_none() {
                    self.thumb_follow = Some(i);
                }
                self.current = i;
            }
            let shadow = Color32::from_black_alpha(if ui.visuals().dark_mode { 90 } else { 30 });
            // 對齊實體像素：貼圖一個像素對一個螢幕像素，才不會被線性取樣糊掉
            let snap = |p: egui::Pos2| pos2((p.x * ppp).round() / ppp, (p.y * ppp).round() / ppp);

            for (i, &local) in placed.iter().enumerate() {
                if self.jump_to == Some(i) {
                    let target = local.translate(origin.to_vec2()).translate(vec2(0.0, -MARGIN / 2.0));
                    ui.scroll_to_rect(target, Some(Align::TOP));
                    self.jump_to = None;
                    self.current = i;
                }
                if !local.intersects(far) {
                    self.textures.remove(&i);
                    continue;
                }
                if !local.intersects(near) {
                    continue;
                }
                let rect = local.translate(origin.to_vec2());
                let painter = ui.painter();
                painter.rect_filled(rect.translate(vec2(0.0, 3.0)).expand(2.0), 3.0, shadow);
                painter.rect_filled(rect, 0.0, Color32::WHITE);

                let exact = (rect.width() * ppp).round().max(1.0);
                let wanted = exact.min(MAX_PX) as u32;
                match self.textures.get(&i) {
                    Some((px, tex)) => {
                        // 解析度剛好符合畫面時，以貼圖原尺寸畫在像素格上（超過上限時只能拉伸）
                        let draw = if *px as f32 == exact {
                            let [w, h] = tex.size();
                            Rect::from_min_size(snap(rect.min), vec2(w as f32, h as f32) / ppp)
                        } else {
                            rect
                        };
                        painter.image(
                            tex.id(),
                            draw,
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                        // 解析度與畫面不符就重新渲染（舊的先繼續顯示）
                        if *px != wanted && !self.failed.contains(&i) {
                            self.request(i, wanted, false);
                        }
                    }
                    None if self.failed.contains(&i) => {
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "這一頁無法顯示",
                            egui::FontId::proportional(14.0),
                            Color32::GRAY,
                        );
                    }
                    None => {
                        self.request(i, wanted, false);
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            format!("{}", i + 1),
                            egui::FontId::proportional(18.0),
                            Color32::from_gray(190),
                        );
                    }
                }
            }
        });
        let dragging = ui.input(|i| {
            i.pointer.is_decidedly_dragging() && i.pointer.press_origin().is_some_and(|p| out.inner_rect.contains(p))
        });
        if dragging {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        self.view = Some(ViewState { placed, offset: out.state.clamped_offset(), inner: out.inner_rect });
    }

    fn thumbnails(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let ppp = ui.ctx().pixels_per_point();
        let follow = self.thumb_follow.take();
        egui::ScrollArea::vertical().id_salt(("pdf-thumbs", &self.path)).auto_shrink([false, false]).show_viewport(
            ui,
            |ui, viewport| {
                let label_h = 18.0;
                let gap = 12.0;
                let mut y = 12.0;
                let mut items = Vec::with_capacity(self.pages.len());
                for &(w, h) in &self.pages {
                    let th = THUMB_W * h / w.max(1.0);
                    items.push(y);
                    y += th + label_h + gap;
                }
                let origin = ui.min_rect().min;
                ui.set_min_size(vec2(THUMB_PANEL_W, y));
                let near = viewport.expand2(vec2(0.0, viewport.height() * 0.5));
                let far = viewport.expand2(vec2(0.0, viewport.height() * 3.0));

                for (i, top) in items.into_iter().enumerate() {
                    let (w, h) = self.pages[i];
                    let th = THUMB_W * h / w.max(1.0);
                    let local = Rect::from_min_size(pos2((THUMB_PANEL_W - THUMB_W) / 2.0, top), vec2(THUMB_W, th));
                    let block =
                        Rect::from_min_size(local.min - vec2(6.0, 6.0), vec2(THUMB_W + 12.0, th + label_h + 10.0));
                    if follow == Some(i) {
                        ui.scroll_to_rect(block.translate(origin.to_vec2()), Some(Align::Center));
                    }
                    if !local.intersects(far) {
                        self.thumbs.remove(&i);
                        continue;
                    }
                    if !block.intersects(near) {
                        continue;
                    }
                    let rect = local.translate(origin.to_vec2());
                    let block_abs = block.translate(origin.to_vec2());
                    let resp = ui.interact(block_abs, ui.scope_id().with(("thumb", i)), Sense::click());
                    let selected = i == self.current;
                    let painter = ui.painter();
                    if selected {
                        painter.rect_filled(block_abs, 6.0, pal.selected);
                    } else if resp.hovered() {
                        painter.rect_filled(block_abs, 6.0, pal.hover);
                    }
                    painter.rect_filled(rect, 0.0, Color32::WHITE);
                    match self.thumbs.get(&i) {
                        Some(tex) => {
                            painter.image(
                                tex.id(),
                                rect,
                                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        }
                        None if !self.failed.contains(&i) => self.request(i, (THUMB_W * ppp).round() as u32, true),
                        None => {}
                    }
                    let border = if selected { Stroke::new(2.0, pal.accent) } else { Stroke::new(1.0, pal.guide) };
                    painter.rect_stroke(rect, 0.0, border, egui::StrokeKind::Outside);
                    painter.text(
                        pos2(rect.center().x, rect.bottom() + 4.0 + label_h / 2.0),
                        egui::Align2::CENTER_CENTER,
                        format!("{}", i + 1),
                        egui::FontId::proportional(12.0),
                        if selected { pal.accent } else { pal.weak },
                    );
                    if resp.clicked() {
                        self.go_to(i);
                    }
                }
            },
        );
    }
}

#[cfg(not(feature = "pdf"))]
fn worker(_path: &Path, _rx: Receiver<Request>, tx: Sender<Response>, ctx: egui::Context) {
    let _ = tx.send(Response::Error("此版本未內建 PDF 檢視器".into()));
    ctx.request_repaint();
}

#[cfg(feature = "pdf")]
fn worker(path: &Path, rx: Receiver<Request>, tx: Sender<Response>, ctx: egui::Context) {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::hayro_syntax::Pdf;
    use hayro::{render, RenderCache, RenderSettings};

    let send = |r: Response| {
        let _ = tx.send(r);
        ctx.request_repaint();
    };
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => return send(Response::Error(e.to_string())),
    };
    let pdf = match Pdf::new(data) {
        Ok(p) => p,
        Err(e) => {
            let msg = match e {
                hayro::hayro_syntax::LoadPdfError::Decryption(_) => "檔案有加密或密碼保護".to_string(),
                hayro::hayro_syntax::LoadPdfError::Invalid => "不是有效的 PDF 檔".to_string(),
            };
            return send(Response::Error(msg));
        }
    };
    let pages = pdf.pages();
    send(Response::Info(pages.iter().map(|p| p.render_dimensions()).collect()));

    let cache = RenderCache::new();
    let settings = InterpreterSettings::default();
    while let Ok(first) = rx.recv() {
        // 把排隊中的請求一次取出：同一頁只保留最新的，正文頁面優先於縮圖
        let mut queue = vec![first];
        while let Ok(r) = rx.try_recv() {
            queue.push(r);
        }
        let mut seen = HashSet::new();
        let mut queue: Vec<Request> = queue.into_iter().rev().filter(|r| seen.insert((r.page, r.thumb))).collect();
        queue.reverse();
        queue.sort_by_key(|r| r.thumb);

        for req in queue {
            let Some(page) = pages.get(req.page) else { continue };
            let (w, h) = page.render_dimensions();
            let scale = req.px_width as f32 / w.max(1.0);
            // 明確指定像素大小：只給比例的話 hayro 會無條件捨去，常少一個像素而被拉伸變糊
            let px_w = req.px_width.min(u16::MAX as u32) as u16;
            let px_h = (h * scale).round().clamp(1.0, u16::MAX as f32) as u16;
            let rs = RenderSettings {
                x_scale: scale,
                y_scale: scale,
                width: Some(px_w),
                height: Some(px_h),
                bg_color: hayro::vello_cpu::color::palette::css::WHITE,
            };
            // hayro 仍在開發中，個別頁面若出錯不要拖垮整個程式
            let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let pix = render(page, &cache, &settings, &rs);
                ColorImage::from_rgba_premultiplied(
                    [pix.width() as usize, pix.height() as usize],
                    pix.data_as_u8_slice(),
                )
            }));
            match rendered {
                Ok(image) => send(Response::Page { page: req.page, px_width: req.px_width, thumb: req.thumb, image }),
                Err(_) => send(Response::PageFailed(req.page)),
            }
        }
    }
}
