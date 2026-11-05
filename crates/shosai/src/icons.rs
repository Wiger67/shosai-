//! 以向量線條繪製的小圖示（不依賴 emoji 字型，各平台顯示一致）。

use std::f32::consts::{PI, TAU};

use eframe::egui::{self, pos2, vec2, Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Sidebar,
    Plus,
    Minus,
    Refresh,
    Search,
    Folder,
    File,
    Pdf,
    Book,
    ChevronRight,
    ChevronDown,
    #[cfg_attr(not(feature = "nextcloud"), allow(dead_code))]
    Cloud,
    Sun,
    Moon,
    Auto,
    More,
    Export,
    FitWidth,
    External,
}

/// 圖示按鈕：平時只有圖示，滑過時出現淡淡的底色。
pub fn button(ui: &mut Ui, icon: Icon, tooltip: &str) -> Response {
    button_sized(ui, icon, tooltip, 28.0)
}

pub fn button_sized(ui: &mut Ui, icon: Icon, tooltip: &str, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let enabled = ui.is_enabled();
        if resp.hovered() && enabled {
            ui.painter().rect_filled(rect, 6.0, v.widgets.hovered.weak_bg_fill);
        }
        let color = if !enabled {
            v.weak_text_color().gamma_multiply(0.5)
        } else if resp.hovered() {
            v.strong_text_color()
        } else {
            v.text_color()
        };
        paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), icon, color);
    }
    if tooltip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tooltip)
    }
}

/// 把圖示畫在 `rect`（建議 16×16）。
pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = Stroke::new(1.4, color);
    let r = rect;
    let at = |x: f32, y: f32| pos2(r.left() + x * r.width(), r.top() + y * r.height());
    match icon {
        Icon::Sidebar => {
            p.rect_stroke(Rect::from_min_max(at(0.08, 0.15), at(0.92, 0.85)), 2.0, s, egui::StrokeKind::Middle);
            p.line_segment([at(0.38, 0.15), at(0.38, 0.85)], s);
        }
        Icon::Plus => {
            p.line_segment([at(0.5, 0.18), at(0.5, 0.82)], s);
            p.line_segment([at(0.18, 0.5), at(0.82, 0.5)], s);
        }
        Icon::Minus => {
            p.line_segment([at(0.18, 0.5), at(0.82, 0.5)], s);
        }
        Icon::Refresh => {
            let c = r.center();
            let rad = r.width() * 0.32;
            p.add(Shape::line(arc(c, rad, -0.35 * PI, 1.35 * PI), s));
            // 箭頭
            let end = c + vec2((-0.35 * PI).cos(), (-0.35 * PI).sin()) * rad;
            p.add(Shape::line(vec![end + vec2(-4.0, -1.0), end, end + vec2(0.5, 4.0)], s));
        }
        Icon::Search => {
            p.circle_stroke(at(0.43, 0.43), r.width() * 0.27, s);
            p.line_segment([at(0.63, 0.63), at(0.86, 0.86)], Stroke::new(1.8, color));
        }
        Icon::Folder => {
            let pts = vec![at(0.06, 0.2), at(0.4, 0.2), at(0.5, 0.32), at(0.94, 0.32), at(0.94, 0.82), at(0.06, 0.82)];
            p.add(Shape::closed_line(pts, s));
        }
        Icon::File | Icon::Pdf => {
            let pts = vec![at(0.2, 0.08), at(0.62, 0.08), at(0.82, 0.28), at(0.82, 0.92), at(0.2, 0.92)];
            p.add(Shape::closed_line(pts, s));
            p.add(Shape::line(vec![at(0.62, 0.08), at(0.62, 0.28), at(0.82, 0.28)], s));
            if icon == Icon::Pdf {
                let band = Rect::from_min_max(at(0.08, 0.52), at(0.7, 0.8));
                p.rect_filled(band, 1.5, Color32::from_rgb(196, 70, 54));
            } else {
                let thin = Stroke::new(1.0, color);
                p.line_segment([at(0.33, 0.52), at(0.69, 0.52)], thin);
                p.line_segment([at(0.33, 0.7), at(0.69, 0.7)], thin);
            }
        }
        Icon::Book => {
            // 打開的書
            p.add(Shape::closed_line(vec![at(0.5, 0.26), at(0.08, 0.16), at(0.08, 0.8), at(0.5, 0.9)], s));
            p.add(Shape::closed_line(vec![at(0.5, 0.26), at(0.92, 0.16), at(0.92, 0.8), at(0.5, 0.9)], s));
        }
        Icon::ChevronRight => {
            p.add(Shape::line(vec![at(0.38, 0.22), at(0.66, 0.5), at(0.38, 0.78)], s));
        }
        Icon::ChevronDown => {
            p.add(Shape::line(vec![at(0.22, 0.38), at(0.5, 0.66), at(0.78, 0.38)], s));
        }
        Icon::Cloud => {
            let mut pts = Vec::new();
            pts.extend(arc(at(0.3, 0.6), r.width() * 0.2, 0.5 * PI, 1.5 * PI));
            pts.extend(arc(at(0.52, 0.45), r.width() * 0.24, 1.08 * PI, 1.95 * PI));
            pts.extend(arc(at(0.73, 0.62), r.width() * 0.18, 1.5 * PI, 2.5 * PI));
            p.add(Shape::closed_line(pts, s));
        }
        Icon::Sun => {
            let c = r.center();
            p.circle_stroke(c, r.width() * 0.2, s);
            for i in 0..8 {
                let a = i as f32 * TAU / 8.0;
                let d = vec2(a.cos(), a.sin());
                p.line_segment([c + d * r.width() * 0.32, c + d * r.width() * 0.44], s);
            }
        }
        Icon::Moon => {
            let c = r.center();
            let rad = r.width() * 0.36;
            let mut pts = arc(c, rad, 0.35 * PI, 1.65 * PI);
            let inner = c + vec2(rad * 0.55, -rad * 0.2);
            let mut back = arc(inner, rad * 0.78, 1.3 * PI, 0.62 * PI);
            pts.append(&mut back);
            p.add(Shape::closed_line(pts, s));
        }
        Icon::Auto => {
            let c = r.center();
            let rad = r.width() * 0.36;
            p.circle_stroke(c, rad, s);
            let mut half = arc(c, rad, 0.5 * PI, 1.5 * PI);
            half.push(c + vec2(0.0, -rad));
            p.add(Shape::convex_polygon(half, color, Stroke::NONE));
        }
        Icon::More => {
            for x in [0.22, 0.5, 0.78] {
                p.circle_filled(at(x, 0.5), 1.6, color);
            }
        }
        Icon::Export => {
            p.add(Shape::line(vec![at(0.15, 0.55), at(0.15, 0.88), at(0.85, 0.88), at(0.85, 0.55)], s));
            p.line_segment([at(0.5, 0.12), at(0.5, 0.66)], s);
            p.add(Shape::line(vec![at(0.3, 0.32), at(0.5, 0.12), at(0.7, 0.32)], s));
        }
        Icon::FitWidth => {
            p.line_segment([at(0.1, 0.5), at(0.9, 0.5)], s);
            p.add(Shape::line(vec![at(0.28, 0.32), at(0.1, 0.5), at(0.28, 0.68)], s));
            p.add(Shape::line(vec![at(0.72, 0.32), at(0.9, 0.5), at(0.72, 0.68)], s));
            p.line_segment([at(0.04, 0.15), at(0.04, 0.85)], s);
            p.line_segment([at(0.96, 0.15), at(0.96, 0.85)], s);
        }
        Icon::External => {
            p.add(Shape::line(vec![at(0.45, 0.18), at(0.15, 0.18), at(0.15, 0.85), at(0.82, 0.85), at(0.82, 0.55)], s));
            p.line_segment([at(0.45, 0.55), at(0.85, 0.15)], s);
            p.add(Shape::line(vec![at(0.58, 0.15), at(0.85, 0.15), at(0.85, 0.42)], s));
        }
    }
}

/// 圓弧上的點（角度以弧度表示，順時針為正，因為螢幕 y 軸向下）。
fn arc(c: Pos2, r: f32, from: f32, to: f32) -> Vec<Pos2> {
    let n = (((to - from).abs() / TAU) * 32.0).ceil().max(4.0) as usize;
    (0..=n)
        .map(|i| {
            let a = from + (to - from) * i as f32 / n as f32;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect()
}
