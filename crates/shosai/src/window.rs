//! 記住視窗的位置、大小與是否最大化。
//!
//! eframe 本身會在設定檔的 `window` 裡記這些，但它不管視窗是不是縮到工作列：
//! 最小化時存檔（每 30 秒自動存一次，或直接從工作列關掉），會把最小化時的
//! 0×0、位置 (−32000, −32000) 當成一般大小存起來。下次開啟時最大化的視窗
//! 「還原」的大小就變成那個，按一下右上角的還原鈕，視窗就縮成一小塊。
//!
//! 這裡自己追蹤視窗「一般狀態」（沒有最大化、全螢幕、最小化）時的位置與大小，
//! 存檔時蓋掉 eframe 寫的那一份；格式和 eframe 一樣，開啟時仍由 eframe 套用
//! （它會把位置限制在現有的螢幕內）。已經存壞的設定在開啟後第一幀修正。

use eframe::egui::{self, vec2, Pos2, Vec2, ViewportCommand};
use serde::{Deserialize, Serialize};

/// eframe 存放視窗狀態的鍵。
const KEY: &str = "window";
/// 沒有可用的紀錄時的大小。
pub const DEFAULT_SIZE: Vec2 = vec2(1180.0, 760.0);
/// 比這還小的紀錄當成壞掉的（最小化時存到的 0×0，或被 eframe 限制成的 64×64）。
const SANE_SIZE: Vec2 = vec2(240.0, 160.0);

/// 和 eframe（egui-winit 的 `WindowGeometry`）相同的格式。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Geometry {
    /// 視窗外框左上角（實體像素）
    outer_position_pixels: Option<Pos2>,
    /// 存的時候螢幕的縮放比例（不含介面縮放）
    scale_factor: f32,
    /// 內容區大小（點，含介面縮放）
    inner_size_points: Vec2,
}

impl Geometry {
    const DEFAULT: Geometry =
        Geometry { outer_position_pixels: None, scale_factor: 1.0, inner_size_points: DEFAULT_SIZE };

    fn sane(&self) -> bool {
        let size = self.inner_size_points;
        let pos_ok = self.outer_position_pixels.is_none_or(|p| p.x > -10_000.0 && p.y > -10_000.0);
        size.x >= SANE_SIZE.x && size.y >= SANE_SIZE.y && size.x.is_finite() && size.y.is_finite() && pos_ok
    }
}

/// 和 eframe（egui-winit 的 `WindowSettings`）相同的格式。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    normal_geometry: Geometry,
    fullscreen: bool,
    maximized: bool,
}

#[derive(Default)]
pub struct WindowMemory {
    /// 開啟時讀到的紀錄
    loaded: Option<Saved>,
    /// 這次執行中最後看到的一般狀態
    normal: Option<Geometry>,
    maximized: bool,
    fullscreen: bool,
    checked: bool,
}

impl WindowMemory {
    pub fn load(storage: Option<&dyn eframe::Storage>) -> WindowMemory {
        let loaded: Option<Saved> = storage.and_then(|s| eframe::get_value(s, KEY));
        WindowMemory {
            loaded,
            maximized: loaded.is_some_and(|l| l.maximized),
            fullscreen: loaded.is_some_and(|l| l.fullscreen),
            ..Default::default()
        }
    }

    /// 每一幀呼叫：記下視窗現在的狀態。
    pub fn track(&mut self, ctx: &egui::Context) {
        let (minimized, maximized, fullscreen, inner, outer, native, monitor) = ctx.input(|i| {
            let v = i.viewport();
            (
                v.minimized,
                v.maximized,
                v.fullscreen,
                v.inner_rect,
                v.outer_rect,
                v.native_pixels_per_point,
                v.monitor_size,
            )
        });
        if !std::mem::replace(&mut self.checked, true) {
            self.repair(ctx, maximized.unwrap_or(self.maximized), monitor);
        }
        // 縮到工作列時什麼都不記：最大化與否維持縮小之前的樣子
        if minimized == Some(true) {
            return;
        }
        if let Some(m) = maximized {
            self.maximized = m;
        }
        if let Some(f) = fullscreen {
            self.fullscreen = f;
        }
        if self.maximized || self.fullscreen {
            return;
        }
        if let (Some(inner), Some(outer), Some(native)) = (inner, outer, native) {
            let px = outer.min * ctx.pixels_per_point();
            let g = Geometry {
                outer_position_pixels: Some(Pos2::new(px.x.round(), px.y.round())),
                scale_factor: native,
                inner_size_points: inner.size(),
            };
            if g.sane() {
                self.normal = Some(g);
            }
        }
    }

    /// 要存的狀態。
    pub fn to_save(&self) -> Saved {
        let normal = self
            .normal
            .or_else(|| self.loaded.map(|l| l.normal_geometry).filter(Geometry::sane))
            .unwrap_or(Geometry::DEFAULT);
        Saved { normal_geometry: normal, fullscreen: self.fullscreen, maximized: self.maximized }
    }

    /// 在 eframe 存完視窗狀態之後呼叫，換成這裡記的。
    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, KEY, &self.to_save());
    }

    /// 上次存的大小是壞的：視窗現在的「還原」大小也是壞的，改成預設大小、放在螢幕中間。
    fn repair(&self, ctx: &egui::Context, maximized: bool, monitor: Option<Vec2>) {
        if self.loaded.is_none_or(|l| l.normal_geometry.sane()) {
            return;
        }
        let size = monitor.map_or(DEFAULT_SIZE, |m| DEFAULT_SIZE.min(m * 0.9));
        if maximized {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
        }
        if let Some(m) = monitor {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(((m - size) / 2.0).max(Vec2::ZERO).to_pos2()));
        }
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
        if maximized {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::Storage as _;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Mem(HashMap<String, String>);

    impl eframe::Storage for Mem {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.into(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    /// 實際從使用者設定檔裡看到、壞掉的紀錄（eframe 0.37 寫的）。
    const BROKEN: &str = "(normal_geometry:(outer_position_pixels:Some((x:0.0,y:0.0)),scale_factor:1.0,inner_size_points:(x:63.636364,y:63.636364)),fullscreen:false,maximized:true)";
    const MINIMIZED: &str = "(normal_geometry:(outer_position_pixels:Some((x:-32000.0,y:-32000.0)),scale_factor:1.0,inner_size_points:(x:0.0,y:0.0)),fullscreen:false,maximized:false)";

    #[test]
    fn same_format_as_eframe() {
        let mut mem = Mem::default();
        mem.set_string(KEY, BROKEN.into());
        let w = WindowMemory::load(Some(&mem));
        let loaded = w.loaded.expect("reads what eframe wrote");
        assert!(loaded.maximized && !loaded.normal_geometry.sane());
        // 寫回去的格式和 eframe 一字不差，eframe 才讀得懂
        eframe::set_value(&mut mem, "copy", &loaded);
        assert_eq!(mem.get_string("copy").unwrap(), BROKEN);
    }

    #[test]
    fn broken_records_are_replaced() {
        for (text, maximized) in [(BROKEN, true), (MINIMIZED, false)] {
            let mut mem = Mem::default();
            mem.set_string(KEY, text.into());
            let w = WindowMemory::load(Some(&mem));
            let saved = w.to_save();
            assert_eq!(saved.normal_geometry, Geometry::DEFAULT, "{text}");
            assert_eq!(saved.maximized, maximized, "keeps whether it was maximized");
        }
        // 沒有紀錄
        assert_eq!(WindowMemory::load(None).to_save().normal_geometry, Geometry::DEFAULT);
    }

    /// 不開視窗跑幾幀：模擬最大化、縮到工作列，存下來的仍是縮小前的一般大小。
    #[test]
    fn minimizing_does_not_overwrite_the_normal_size() {
        let ctx = egui::Context::default();
        let mut w = WindowMemory::load(None);
        let frame = |w: &mut WindowMemory, info: egui::ViewportInfo| {
            let mut input = egui::RawInput::default();
            input.viewports.insert(egui::ViewportId::ROOT, info);
            let mut out = ctx.run_ui(input, |ui| w.track(ui.ctx()));
            out.textures_delta.clear();
        };
        let normal = egui::ViewportInfo {
            native_pixels_per_point: Some(1.0),
            inner_rect: Some(egui::Rect::from_min_size(Pos2::new(208.0, 240.0), vec2(1000.0, 700.0))),
            outer_rect: Some(egui::Rect::from_min_size(Pos2::new(200.0, 200.0), vec2(1016.0, 748.0))),
            minimized: Some(false),
            maximized: Some(false),
            fullscreen: Some(false),
            ..Default::default()
        };
        frame(&mut w, normal.clone());
        let expected = Geometry {
            outer_position_pixels: Some(Pos2::new(200.0, 200.0)),
            scale_factor: 1.0,
            inner_size_points: vec2(1000.0, 700.0),
        };
        assert_eq!(w.to_save().normal_geometry, expected);

        let maximized = egui::ViewportInfo {
            inner_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(1920.0, 1040.0))),
            outer_rect: Some(egui::Rect::from_min_size(Pos2::new(-8.0, -8.0), vec2(1936.0, 1056.0))),
            maximized: Some(true),
            ..normal.clone()
        };
        frame(&mut w, maximized);
        let minimized = egui::ViewportInfo {
            inner_rect: Some(egui::Rect::from_min_size(Pos2::new(-32000.0, -32000.0), Vec2::ZERO)),
            outer_rect: Some(egui::Rect::from_min_size(Pos2::new(-32000.0, -32000.0), Vec2::ZERO)),
            minimized: Some(true),
            maximized: Some(false),
            ..normal
        };
        frame(&mut w, minimized);
        let saved = w.to_save();
        assert_eq!(saved.normal_geometry, expected, "the size from before maximizing");
        assert!(saved.maximized, "it was maximized before going to the taskbar");
    }
}
