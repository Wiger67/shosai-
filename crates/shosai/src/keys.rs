//! 鍵盤捲動：egui 的 ScrollArea 本身不理會鍵盤，這裡補上方向鍵、空白鍵、
//! PageUp／PageDown、Home／End。

use eframe::egui::{self, vec2, Align, Key, Rect, Vec2};

/// 按住 ↑ ↓ 時的捲動速度（點／秒）。
const HOLD_SPEED: f32 = 1100.0;

/// 在 ScrollArea 的內容裡、內容排完之後呼叫。
///
/// - ↑ ↓：按住時每一幀依經過的時間連續捲動（不是靠鍵盤的重複輸入一格一格跳）
/// - 空白鍵／Shift+空白鍵：往下／往上一個畫面
/// - `paging`：另外處理 PageUp／PageDown（一個畫面）與 Home／End（最前、最後）；
///   PDF 這幾個鍵是整頁跳，所以不開
///
/// `content` 是整份內容的範圍（Home／End 用）。有輸入框取得焦點時不作用。
pub fn scroll(ui: &egui::Ui, content: Rect, paging: bool) {
    if ui.ctx().memory(|m| m.focused().is_some()) {
        return;
    }
    let view_h = ui.clip_rect().height();
    let (hold, pages, home, end, dt) = ui.input(|i| {
        if i.modifiers.command || i.modifiers.alt {
            return (0.0, 0.0, false, false, 0.0);
        }
        let hold = f32::from(i8::from(i.key_down(Key::ArrowDown)) - i8::from(i.key_down(Key::ArrowUp)));
        let mut pages = 0.0;
        if i.key_pressed(Key::Space) {
            pages += if i.modifiers.shift { -1.0 } else { 1.0 };
        }
        let (mut home, mut end) = (false, false);
        if paging {
            pages += f32::from(i8::from(i.key_pressed(Key::PageDown)) - i8::from(i.key_pressed(Key::PageUp)));
            home = i.key_pressed(Key::Home);
            end = i.key_pressed(Key::End);
        }
        (hold, pages, home, end, i.stable_dt.min(0.05))
    });
    // scroll_with_delta 的正值是「內容往下移」，也就是往上捲
    if hold != 0.0 {
        ui.scroll_with_delta_animation(vec2(0.0, -hold * HOLD_SPEED * dt), egui::style::ScrollAnimation::none());
        ui.ctx().request_repaint();
    }
    if pages != 0.0 {
        // 留一點重疊，翻過去時找得到剛剛讀到哪
        ui.scroll_with_delta(vec2(0.0, -pages * (view_h - 48.0).max(view_h * 0.5)));
    }
    if home {
        ui.scroll_to_rect(Rect::from_min_size(content.left_top(), Vec2::ZERO), Some(Align::TOP));
    }
    if end {
        ui.scroll_to_rect(Rect::from_min_size(content.left_bottom(), Vec2::ZERO), Some(Align::BOTTOM));
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, pos2, vec2, Key, Rect};

    fn frame(ctx: &egui::Context, time: f64, events: Vec<egui::Event>, paging: bool) -> f32 {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0))),
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut offset = 0.0;
        let mut out = ctx.run_ui(input, |ui| {
            let area = egui::ScrollArea::vertical().show(ui, |ui| {
                ui.allocate_space(vec2(200.0, 5000.0));
                super::scroll(ui, ui.min_rect(), paging);
            });
            offset = area.state.clamped_offset().y;
        });
        // egui panics if texture updates are dropped unhandled
        out.textures_delta.clear();
        offset
    }

    fn key(key: Key, pressed: bool) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    /// Run frames at 60 fps until `secs` have passed; returns the final scroll offset.
    fn settle(ctx: &egui::Context, t: &mut f64, secs: f64, paging: bool) -> f32 {
        let end = *t + secs;
        let mut offset = 0.0;
        while *t < end {
            *t += 1.0 / 60.0;
            offset = frame(ctx, *t, Vec::new(), paging);
        }
        offset
    }

    #[test]
    fn space_scrolls_one_screen() {
        let ctx = egui::Context::default();
        let mut t = 0.0;
        settle(&ctx, &mut t, 0.1, true);
        frame(&ctx, t, vec![key(Key::Space, true)], true);
        let down = settle(&ctx, &mut t, 1.0, true);
        assert!((200.0..=300.0).contains(&down), "one screen minus the overlap, got {down}");
        frame(&ctx, t, vec![key(Key::End, true)], true);
        let end = settle(&ctx, &mut t, 1.0, true);
        assert!(end > 4000.0, "End goes to the bottom, got {end}");
        frame(&ctx, t, vec![key(Key::Home, true)], true);
        assert_eq!(settle(&ctx, &mut t, 1.0, true), 0.0);
    }

    #[test]
    fn holding_arrow_scrolls_continuously() {
        let ctx = egui::Context::default();
        let mut t = 0.0;
        settle(&ctx, &mut t, 0.1, false);
        frame(&ctx, t, vec![key(Key::ArrowDown, true)], false);
        let a = settle(&ctx, &mut t, 0.25, false);
        let b = settle(&ctx, &mut t, 0.25, false);
        assert!(a > 100.0, "moves while held, got {a}");
        assert!(b > a + 100.0, "keeps moving while held, got {a} then {b}");
        frame(&ctx, t, vec![key(Key::ArrowDown, false)], false);
        let c = settle(&ctx, &mut t, 0.1, false);
        let d = settle(&ctx, &mut t, 0.25, false);
        assert_eq!(c, d, "stops when released");
        // PDF: Home / End / PageDown are page jumps handled elsewhere, not scrolling
        frame(&ctx, t, vec![key(Key::End, true)], false);
        assert_eq!(settle(&ctx, &mut t, 0.5, false), d);
    }
}
