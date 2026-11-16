//! 本機點陣圖：在背景執行緒解碼並縮小後做成貼圖（預覽、書本模式的插圖與封面共用）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui;

/// 點陣圖最大解碼寬度（像素）。原圖再大也只保留這個寬度，避免一張相片吃掉幾十 MB。
const MAX_IMAGE_PX: u32 = 1600;

pub type ImageCache = Arc<Mutex<HashMap<PathBuf, ImageState>>>;

pub enum ImageState {
    Loading,
    /// 貼圖與原圖尺寸（像素）
    Ready(egui::TextureHandle, [u32; 2]),
    Failed(String),
}

/// 是否為本機的點陣圖（PNG / JPEG / GIF / WebP）。
pub fn is_local_raster(url: &str) -> bool {
    if url.contains("://") || url.starts_with("data:") {
        return false;
    }
    let lower = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp"].iter().any(|e| lower.ends_with(e))
}

/// Markdown 中的相對網址 → 檔案路徑。
pub fn resolve(base_dir: &Path, url: &str) -> PathBuf {
    let rel = shosai_core::export::percent_decode_pub(url.split(['?', '#']).next().unwrap_or(url));
    base_dir.join(rel.trim_start_matches("./"))
}

/// 取得貼圖；還沒載入時會在背景開始載入並回傳 `None`（完成後自動重畫）。
pub fn get(
    cache: &ImageCache,
    path: &Path,
    ctx: &egui::Context,
) -> Option<Result<(egui::TextureId, [u32; 2]), String>> {
    let mut map = cache.lock().unwrap();
    match map.get(path) {
        Some(ImageState::Ready(tex, size)) => Some(Ok((tex.id(), *size))),
        Some(ImageState::Failed(e)) => Some(Err(e.clone())),
        Some(ImageState::Loading) => None,
        None => {
            map.insert(path.to_path_buf(), ImageState::Loading);
            let (images, ctx, p) = (cache.clone(), ctx.clone(), path.to_path_buf());
            std::thread::spawn(move || {
                let result = load(&p, &ctx);
                images.lock().unwrap().insert(p, result);
                ctx.request_repaint();
            });
            None
        }
    }
}

fn load(path: &Path, ctx: &egui::Context) -> ImageState {
    // 只有真的不存在才說「找不到」；沒有權限之類的錯誤交給下面照實回報
    if std::fs::metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return ImageState::Failed(missing_message(path));
    }
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

/// 圖片檔不存在時的說明：列出最後幾層路徑，看得出是哪個資料夾少了檔案。
/// 最常見的原因是同步程式（例如手機上的 Nextcloud App）還沒把那個資料夾下載下來。
fn missing_message(path: &Path) -> String {
    // 先把 `章節/../插圖/x.jpg` 這種寫法化簡，顯示的才是實際要找的位置
    let mut parts: Vec<String> = Vec::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            _ => {}
        }
    }
    let tail = parts[parts.len().saturating_sub(3)..].join("/");
    format!("找不到圖片檔：{tail}\n同步的資料夾可能還沒下載完")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_image_says_which_file() {
        let dir = Path::new("/storage/emulated/0/書/86/01 第一卷");
        let path = resolve(dir, "../插圖/01%20第一卷/001.jpg");
        let ImageState::Failed(msg) = load(&path, &egui::Context::default()) else { panic!("loaded a missing file") };
        assert!(msg.contains("插圖/01 第一卷/001.jpg"), "{msg}");
        assert!(!msg.contains(".."), "{msg}");
    }
}
