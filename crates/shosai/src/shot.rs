//! 測試用：把 egui 一幀的輸出用軟體畫成圖片，不需要視窗或顯示卡。
//!
//! 動畫、版面這類「要用看的」才知道對不對的東西，可以在測試裡存成 PNG 來檢查。
//! 只求看得出畫面長什麼樣子：貼圖取最近的像素，三角形的邊不做特別處理。

use std::collections::HashMap;
use std::path::Path;

use eframe::egui::{self, epaint, Color32, ColorImage, TextureId};

/// 目前有哪些貼圖（字型圖集、圖片）。每一幀都要用 [`Textures::update`] 跟上變動。
#[derive(Default)]
pub struct Textures(HashMap<TextureId, ColorImage>);

impl Textures {
    /// 套用這一幀的貼圖變動（之後 `delta` 就是空的，可以直接丟掉）。
    pub fn update(&mut self, delta: &mut egui::TexturesDelta) {
        for (id, changes) in std::mem::take(&mut delta.set) {
            for change in changes {
                let egui::ImageData::Color(image) = &change.image;
                match change.pos {
                    None => {
                        self.0.insert(id, ColorImage::clone(image));
                    }
                    Some([x, y]) => {
                        let Some(whole) = self.0.get_mut(&id) else { continue };
                        for row in 0..image.size[1] {
                            let from = row * image.size[0];
                            let to = (y + row) * whole.size[0] + x;
                            whole.pixels[to..to + image.size[0]]
                                .copy_from_slice(&image.pixels[from..from + image.size[0]]);
                        }
                    }
                }
            }
        }
        for id in std::mem::take(&mut delta.free) {
            self.0.remove(&id);
        }
        delta.clear();
    }

    /// 把一幀的形狀畫成 `size`（像素）大小的圖片，存成 PNG。
    pub fn save(&self, ctx: &egui::Context, shapes: Vec<epaint::ClippedShape>, size: [usize; 2], path: &Path) {
        let [w, h] = size;
        let ppp = ctx.pixels_per_point();
        // 預乘過透明度的 RGBA
        let mut canvas = vec![[0.0f32, 0.0, 0.0, 255.0]; w * h];
        for clipped in ctx.tessellate(shapes, ppp) {
            let epaint::Primitive::Mesh(mesh) = &clipped.primitive else { continue };
            let texture = self.0.get(&mesh.texture_id);
            let clip = clipped.clip_rect;
            let limit = |v: f32, max: usize| (v * ppp).round().clamp(0.0, max as f32) as usize;
            let (cx0, cx1) = (limit(clip.min.x, w), limit(clip.max.x, w));
            let (cy0, cy1) = (limit(clip.min.y, h), limit(clip.max.y, h));
            for tri in mesh.indices.as_chunks::<3>().0 {
                let v = [0, 1, 2].map(|i| &mesh.vertices[tri[i] as usize]);
                let p = v.map(|v| [v.pos.x * ppp, v.pos.y * ppp]);
                let area = edge(p[0], p[1], p[2]);
                if area == 0.0 {
                    continue;
                }
                let lo = |f: fn(f32, f32) -> f32, i: usize| p.iter().map(|p| p[i]).fold(p[0][i], f);
                let x0 = (lo(f32::min, 0).floor().max(0.0) as usize).max(cx0);
                let x1 = (lo(f32::max, 0).ceil().max(0.0) as usize).min(cx1);
                let y0 = (lo(f32::min, 1).floor().max(0.0) as usize).max(cy0);
                let y1 = (lo(f32::max, 1).ceil().max(0.0) as usize).min(cy1);
                for py in y0..y1 {
                    for px in x0..x1 {
                        let at = [px as f32 + 0.5, py as f32 + 0.5];
                        let weights =
                            [edge(p[1], p[2], at) / area, edge(p[2], p[0], at) / area, edge(p[0], p[1], at) / area];
                        if weights.iter().any(|w| *w < 0.0) {
                            continue;
                        }
                        let mix =
                            |f: &dyn Fn(&epaint::Vertex) -> f32| (0..3).map(|i| weights[i] * f(v[i])).sum::<f32>();
                        let texel = texture.map_or(Color32::WHITE, |t| {
                            let tx = ((mix(&|v| v.uv.x) * t.size[0] as f32) as usize).min(t.size[0] - 1);
                            let ty = ((mix(&|v| v.uv.y) * t.size[1] as f32) as usize).min(t.size[1] - 1);
                            t.pixels[ty * t.size[0] + tx]
                        });
                        let src: [f32; 4] =
                            std::array::from_fn(|c| mix(&|v| f32::from(v.color[c])) * f32::from(texel[c]) / 255.0);
                        let dst = &mut canvas[py * w + px];
                        for c in 0..4 {
                            dst[c] = src[c] + dst[c] * (1.0 - src[3] / 255.0);
                        }
                    }
                }
            }
        }
        let bytes: Vec<u8> = canvas.iter().flat_map(|p| [p[0] as u8, p[1] as u8, p[2] as u8, 255]).collect();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        image::RgbaImage::from_raw(w as u32, h as u32, bytes).unwrap().save(path).unwrap();
    }
}

/// 點 `c` 在有向線段 a→b 的哪一側（兩倍的三角形面積）。
fn edge(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}
