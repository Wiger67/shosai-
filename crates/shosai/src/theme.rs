//! 配色：暖色、低對比的「書房」風格，淺色與深色各一套。
//!
//! 介面上盡量不用分隔線，改用背景色深淺來區分區塊。

use eframe::egui::{self, Color32, CornerRadius, Shadow, Stroke, Visuals};

/// 介面用到的顏色。
#[derive(Clone, Copy)]
pub struct Palette {
    /// 側欄、工具列背景
    pub chrome: Color32,
    /// 主要內容區背景（編輯器、預覽）
    pub surface: Color32,
    /// 閱讀模式的桌面與紙張
    pub desk: Color32,
    pub paper: Color32,
    /// 文字
    pub ink: Color32,
    pub weak: Color32,
    /// 強調色（選取、目前檔案）
    pub accent: Color32,
    /// 列表 hover / 選取背景
    pub hover: Color32,
    pub selected: Color32,
    /// 檔案樹：各類型的圖示顏色與縮排導引線
    pub folder: Color32,
    pub book: Color32,
    pub markdown: Color32,
    pub pdf: Color32,
    pub guide: Color32,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            chrome: rgb(0x1e1d1b),
            surface: rgb(0x252422),
            desk: rgb(0x191816),
            paper: rgb(0x2a2926),
            ink: rgb(0xe3ddd0),
            weak: rgb(0x8f877a),
            accent: rgb(0xd6a273),
            hover: rgb(0x2c2a27),
            selected: rgb(0x3a3129),
            folder: rgb(0xd9a94e),
            book: rgb(0xd6895a),
            markdown: rgb(0x8fa9c9),
            pdf: rgb(0xe0705e),
            guide: rgb(0x3a3834),
        }
    } else {
        Palette {
            chrome: rgb(0xf2eee6),
            surface: rgb(0xfcfbf8),
            desk: rgb(0xe9e4da),
            paper: rgb(0xfdfbf6),
            ink: rgb(0x2f2a24),
            weak: rgb(0x8c8376),
            accent: rgb(0x9a5f32),
            hover: rgb(0xe8e2d7),
            selected: rgb(0xe3d5c3),
            folder: rgb(0xc08a2a),
            book: rgb(0xb0582c),
            markdown: rgb(0x4f6f96),
            pdf: rgb(0xc4463a),
            guide: rgb(0xddd5c7),
        }
    }
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// 在啟動時呼叫一次：設定淺色與深色兩套樣式，切換主題時 egui 會自動套用。
pub fn install(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Light, visuals(false));
    ctx.set_visuals_of(egui::Theme::Dark, visuals(true));
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.menu_margin = egui::Margin::same(6);
        style.spacing.interact_size.y = 24.0;
        style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(14.5));
        style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
        style.url_in_tooltip = true;
    });
}

fn visuals(dark: bool) -> Visuals {
    let p = palette(dark);
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    let radius = CornerRadius::same(6);

    v.panel_fill = p.chrome;
    v.window_fill = p.surface;
    v.extreme_bg_color = if dark { rgb(0x1a1917) } else { rgb(0xffffff) };
    v.faint_bg_color = if dark { rgb(0x2a2926) } else { rgb(0xf4f0e8) };
    v.code_bg_color = if dark { rgb(0x2e2c29) } else { rgb(0xf1ece3) };
    v.hyperlink_color = if dark { rgb(0x8db7f0) } else { rgb(0x2f66ad) };
    v.selection.bg_fill = p.selected;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.window_stroke = Stroke::new(1.0, if dark { rgb(0x34322e) } else { rgb(0xe2dccf) });
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow =
        Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(if dark { 90 } else { 38 }) };
    v.popup_shadow =
        Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if dark { 80 } else { 30 }) };
    v.striped = false;
    v.indent_has_left_vline = false;
    v.collapsing_header_frame = false;

    let w = &mut v.widgets;
    // 靜態元素（文字、分隔線）
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.ink);
    w.noninteractive.bg_stroke = Stroke::new(1.0, if dark { rgb(0x34322e) } else { rgb(0xe4ded2) });
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.corner_radius = radius;
    // 按鈕：平時無框、無底色，滑過才出現
    w.inactive.weak_bg_fill = Color32::TRANSPARENT;
    w.inactive.bg_fill = if dark { rgb(0x34322e) } else { rgb(0xe6e0d4) };
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, p.ink);
    w.inactive.corner_radius = radius;
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.bg_fill = p.hover;
    w.hovered.bg_stroke = Stroke::NONE;
    w.hovered.fg_stroke = Stroke::new(1.5, p.ink);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 0.0;
    w.active.weak_bg_fill = p.selected;
    w.active.bg_fill = p.selected;
    w.active.bg_stroke = Stroke::NONE;
    w.active.fg_stroke = Stroke::new(1.5, p.ink);
    w.active.corner_radius = radius;
    w.active.expansion = 0.0;
    w.open.weak_bg_fill = p.hover;
    w.open.bg_fill = p.hover;
    w.open.bg_stroke = Stroke::NONE;
    w.open.corner_radius = radius;
    v
}
