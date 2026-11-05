//! PDF 檢視器：以 hayro（純 Rust 的 PDF 渲染器，Typst 本身也在用）在背景執行緒渲染頁面。
//!
//! 只渲染畫面附近的頁面，並依縮放比例重新渲染，離畫面太遠的頁面會釋放記憶體。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui::{self, pos2, vec2, Align, Color32, ColorImage, Rect, TextureHandle, TextureOptions};

/// 1pt = 4/3 px（CSS 96 dpi）
const PT_TO_PX: f32 = 96.0 / 72.0;
const PAGE_GAP: f32 = 18.0;
const MARGIN: f32 = 28.0;
/// 單頁貼圖的最大寬度（像素），避免超大頁面吃光記憶體
const MAX_PX: f32 = 4096.0;

#[cfg_attr(not(feature = "pdf"), allow(dead_code))]
struct Request {
    page: usize,
    px_width: u32,
}

#[cfg_attr(not(feature = "pdf"), allow(dead_code))]
enum Response {
    Info(Vec<(f32, f32)>),
    Page { page: usize, px_width: u32, image: ColorImage },
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
    pending: HashSet<usize>,
    failed: HashSet<usize>,
    tx: Sender<Request>,
    rx: Receiver<Response>,
    /// `None` = 符合寬度；`Some(1.0)` = 100%
    pub zoom: Option<f32>,
    /// 目前頁（從 0 起算）
    pub current: usize,
    jump_to: Option<usize>,
    last_fit_scale: f32,
}

impl PdfView {
    pub fn open(path: &Path, ctx: &egui::Context) -> PdfView {
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
            pending: HashSet::new(),
            failed: HashSet::new(),
            tx: req_tx,
            rx: resp_rx,
            zoom: None,
            current: 0,
            jump_to: None,
            last_fit_scale: 1.0,
        }
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// 目前實際的縮放比例（符合寬度時為計算出來的值）。
    pub fn effective_zoom(&self) -> f32 {
        self.zoom.unwrap_or(self.last_fit_scale)
    }

    pub fn zoom_by(&mut self, factor: f32) {
        let z = (self.effective_zoom() * factor).clamp(0.25, 5.0);
        self.zoom = Some(z);
    }

    pub fn fit_width(&mut self) {
        self.zoom = None;
    }

    pub fn go_to(&mut self, page: usize) {
        if !self.pages.is_empty() {
            self.jump_to = Some(page.min(self.pages.len() - 1));
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(resp) = self.rx.try_recv() {
            match resp {
                Response::Info(pages) => {
                    self.pages = pages;
                    self.loaded = true;
                }
                Response::Page { page, px_width, image } => {
                    self.pending.remove(&page);
                    let name = format!("pdf-{}-{page}", self.path.display());
                    let tex = ctx.load_texture(name, image, TextureOptions::LINEAR);
                    self.textures.insert(page, (px_width, tex));
                }
                Response::PageFailed(page) => {
                    self.pending.remove(&page);
                    self.failed.insert(page);
                }
                Response::Error(e) => self.error = Some(e),
            }
        }
    }

    fn request(&mut self, page: usize, px_width: u32) {
        if self.pending.insert(page) {
            let _ = self.tx.send(Request { page, px_width });
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, desk: Color32) {
        self.poll(ui.ctx());
        if let Some(e) = &self.error {
            ui.centered_and_justified(|ui| {
                ui.label(egui::RichText::new(format!("無法開啟這個 PDF：{e}")).color(ui.visuals().error_fg_color));
            });
            return;
        }
        if !self.loaded {
            ui.centered_and_justified(|ui| {
                ui.spinner();
            });
            return;
        }

        // ← → 翻頁
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        if !typing {
            let (prev, next) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::PageUp),
                    i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::PageDown),
                )
            });
            if prev {
                self.go_to(self.current.saturating_sub(1));
            }
            if next {
                self.go_to(self.current + 1);
            }
        }

        let ppp = ui.ctx().pixels_per_point();
        egui::Frame::new().fill(desk).show(ui, |ui| {
            egui::ScrollArea::both().id_salt(("pdf", &self.path)).auto_shrink([false, false]).show_viewport(
                ui,
                |ui, viewport| {
                    let avail = ui.available_width();
                    let widest = self.pages.iter().map(|p| p.0).fold(1.0, f32::max);
                    // 符合寬度，但不超過舒適的閱讀寬度
                    let fit = ((avail - 2.0 * MARGIN).clamp(120.0, 860.0)) / (widest * PT_TO_PX);
                    self.last_fit_scale = fit;
                    let zoom = self.zoom.unwrap_or(fit);

                    // 版面：每頁的位置（相對於內容左上角）
                    let mut y = MARGIN;
                    let mut rects = Vec::with_capacity(self.pages.len());
                    let mut content_w: f32 = avail;
                    for &(w, h) in &self.pages {
                        let size = vec2(w, h) * PT_TO_PX * zoom;
                        content_w = content_w.max(size.x + 2.0 * MARGIN);
                        rects.push((y, size));
                        y += size.y + PAGE_GAP;
                    }
                    let content_h = y + MARGIN - PAGE_GAP;
                    let origin = ui.min_rect().min;
                    ui.set_min_size(vec2(content_w, content_h));

                    let near = viewport.expand2(vec2(0.0, viewport.height()));
                    let far = viewport.expand2(vec2(0.0, viewport.height() * 4.0));
                    let center_y = viewport.center().y;
                    let shadow = Color32::from_black_alpha(if ui.visuals().dark_mode { 90 } else { 30 });

                    for (i, &(top, size)) in rects.iter().enumerate() {
                        let x = ((content_w - size.x) / 2.0).max(MARGIN);
                        let local = Rect::from_min_size(pos2(x, top), size);
                        if top <= center_y && center_y <= top + size.y + PAGE_GAP {
                            self.current = i;
                        }
                        if self.jump_to == Some(i) {
                            ui.scroll_to_rect(local.translate(origin.to_vec2()), Some(Align::TOP));
                            self.jump_to = None;
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

                        let wanted = (size.x * ppp).min(MAX_PX).round() as u32;
                        match self.textures.get(&i) {
                            Some((px, tex)) => {
                                painter.image(
                                    tex.id(),
                                    rect,
                                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                    Color32::WHITE,
                                );
                                // 縮放差太多時用新的解析度重新渲染（舊的先繼續顯示）
                                let ratio = wanted as f32 / *px as f32;
                                if !(0.85..=1.15).contains(&ratio) && !self.failed.contains(&i) {
                                    self.request(i, wanted);
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
                                self.request(i, wanted);
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
                },
            );
        });
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
        // 把排隊中的請求一次取出，同一頁只保留最新的
        let mut queue = vec![first];
        while let Ok(r) = rx.try_recv() {
            queue.push(r);
        }
        let mut seen = HashSet::new();
        let queue: Vec<Request> = queue.into_iter().rev().filter(|r| seen.insert(r.page)).collect();

        for req in queue.into_iter().rev() {
            let Some(page) = pages.get(req.page) else { continue };
            let (w, _) = page.render_dimensions();
            let scale = req.px_width as f32 / w.max(1.0);
            let rs = RenderSettings {
                x_scale: scale,
                y_scale: scale,
                bg_color: hayro::vello_cpu::color::palette::css::WHITE,
                ..Default::default()
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
                Ok(image) => send(Response::Page { page: req.page, px_width: req.px_width, image }),
                Err(_) => send(Response::PageFailed(req.page)),
            }
        }
    }
}
