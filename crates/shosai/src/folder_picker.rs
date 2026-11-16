//! 手機版「選擇書庫資料夾」的畫面。
//!
//! 手機上沒有系統的檔案對話框可用，所以自己把資料夾列出來，讓使用者一層一層點進去，
//! 走到要當書庫的那個資料夾再按「用這個資料夾」。整個畫面只用點的，不需要鍵盤。

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, RichText};
use shosai_core::library;

use crate::theme::Palette;

/// 手機的內部儲存空間（檔案管理 App 裡看到的最上層）。
pub const INTERNAL_STORAGE: &str = "/storage/emulated/0";
/// Nextcloud App 把同步下來的檔案放在這裡（底下一個帳號一個資料夾）。
const NEXTCLOUD_FILES: &str = "/storage/emulated/0/Android/media/com.nextcloud.client/nextcloud";

pub struct FolderPicker {
    dir: PathBuf,
    /// 這個資料夾裡的子資料夾（路徑、名稱），依名稱排序
    dirs: Vec<(PathBuf, String)>,
    /// 這個資料夾裡直接放了幾個 Markdown 檔
    notes: usize,
    error: Option<String>,
    /// 上方可以直接跳過去的地方（名稱、路徑）：存在、而且不是現在這個資料夾的才列出來
    shortcuts: Vec<(&'static str, &'static Path)>,
}

pub enum Picked {
    Chosen(PathBuf),
    Cancelled,
}

impl FolderPicker {
    pub fn open(start: &Path) -> FolderPicker {
        let mut picker =
            FolderPicker { dir: PathBuf::new(), dirs: Vec::new(), notes: 0, error: None, shortcuts: Vec::new() };
        picker.go(start.to_path_buf());
        picker
    }

    /// 一開始要從哪裡看起：手機從內部儲存空間，其他情況從目前的書庫。
    pub fn start(library: Option<&Path>) -> PathBuf {
        let storage = Path::new(INTERNAL_STORAGE);
        if storage.is_dir() {
            return storage.to_path_buf();
        }
        library.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"))
    }

    fn go(&mut self, dir: PathBuf) {
        self.dirs.clear();
        self.notes = 0;
        self.error = None;
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = entry.file_name().to_string_lossy().into_owned();
                    // 隱藏資料夾（.shosai、.thumbnails…）與書櫃也不列的資料夾不會是書庫
                    if name.starts_with('.') || library::IGNORED_DIRS.contains(&name.as_str()) {
                        continue;
                    }
                    if path.is_dir() {
                        self.dirs.push((path, name));
                    } else if shosai_core::is_markdown(&path) {
                        self.notes += 1;
                    }
                }
                // 和書櫃一樣的順序（「第2卷」在「第10卷」前面）
                self.dirs.sort_by(|(_, a), (_, b)| library::natural_cmp(a, b));
            }
            Err(e) => self.error = Some(format!("讀不到這個資料夾：{e}")),
        }
        // 換資料夾時才看一次捷徑在不在（手機上每次都要問儲存空間，別每一幀都問）
        self.shortcuts = [("內部儲存空間", INTERNAL_STORAGE), ("Nextcloud", NEXTCLOUD_FILES)]
            .into_iter()
            .map(|(label, path)| (label, Path::new(path)))
            .filter(|(_, path)| *path != dir && path.is_dir())
            .collect();
        self.dir = dir;
    }

    /// 畫出畫面；使用者做了決定時回傳結果。
    pub fn show(&mut self, ui: &mut egui::Ui, pal: &Palette) -> Option<Picked> {
        let mut result = None;
        let mut go = None;
        egui::Frame::new().fill(pal.surface).inner_margin(egui::Margin::symmetric(14, 12)).show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            ui.label(RichText::new("選擇書庫資料夾").size(18.0).strong().color(pal.ink));
            ui.label(RichText::new(self.dir.display().to_string()).small().color(pal.weak));

            // 換地方：上一層，以及幾個常用的起點
            ui.horizontal_wrapped(|ui| {
                let chip = |ui: &mut egui::Ui, text: &str| {
                    ui.add(egui::Button::new(text).fill(pal.hover).corner_radius(8).min_size(egui::vec2(72.0, 36.0)))
                };
                if let Some(parent) = self.dir.parent() {
                    if chip(ui, "上一層").clicked() {
                        go = Some(parent.to_path_buf());
                    }
                }
                for (label, path) in &self.shortcuts {
                    if chip(ui, label).clicked() {
                        go = Some(path.to_path_buf());
                    }
                }
            });

            ui.horizontal(|ui| {
                let choose = egui::Button::new(RichText::new("用這個資料夾").size(15.0).color(Color32::WHITE))
                    .fill(pal.accent)
                    .corner_radius(8)
                    .min_size(egui::vec2(150.0, 40.0));
                if ui.add_enabled(self.error.is_none(), choose).clicked() {
                    result = Some(Picked::Chosen(self.dir.clone()));
                }
                if ui.add(egui::Button::new("取消").corner_radius(8).min_size(egui::vec2(72.0, 40.0))).clicked() {
                    result = Some(Picked::Cancelled);
                }
            });

            let summary = match &self.error {
                Some(e) => e.clone(),
                None if self.dirs.is_empty() && self.notes == 0 => {
                    "這裡是空的。如果裡面應該有東西，請到系統設定允許書齋「所有檔案存取權」。".to_string()
                }
                None => {
                    format!("這裡有 {} 個資料夾、{} 篇文章。書庫裡的每個資料夾是一本書。", self.dirs.len(), self.notes)
                }
            };
            ui.label(RichText::new(summary).small().color(pal.weak));

            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.spacing_mut().button_padding = egui::vec2(12.0, 4.0);
                    for (path, name) in &self.dirs {
                        let row = egui::Button::new(RichText::new(name).size(15.0))
                            .fill(pal.hover)
                            .corner_radius(8)
                            .min_size(egui::vec2(0.0, 44.0));
                        if ui.add(row).clicked() {
                            go = Some(path.clone());
                        }
                    }
                });
            });
        });
        if let Some(dir) = go {
            self.go(dir);
        }
        result
    }
}
