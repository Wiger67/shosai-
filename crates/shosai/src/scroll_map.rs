//! 原始文字位置 ↔ 畫面位置的對照。
//!
//! 分割模式的捲動同步、大綱的「目前在哪個標題」、跳到某個標題或尋找結果，都是同一件事：
//! 知道原始文字的第幾個字畫在畫面的哪個高度。兩邊都用「第幾個字元」（可帶小數）當共同的座標。
//!
//! - 預覽：每個最上層區塊有一個對位點（區塊在原始文字的位置、畫在多高），區塊之間用內插
//! - 編輯器：直接看排好版的每一列，精確到列內的比例

use eframe::egui::Galley;

/// 一組對位點：（原始文字的字元位置、畫面上的 y），兩者都由小到大。
pub type Anchors = [(f32, f32)];

/// y 這個高度對應到原始文字的第幾個字。
pub fn source_at(anchors: &Anchors, y: f32) -> f32 {
    interpolate(anchors, y, |a| a.1, |a| a.0)
}

/// 原始文字第 `source` 個字畫在多高。
pub fn y_at(anchors: &Anchors, source: f32) -> f32 {
    interpolate(anchors, source, |a| a.0, |a| a.1)
}

fn interpolate(anchors: &Anchors, x: f32, from: fn(&(f32, f32)) -> f32, to: fn(&(f32, f32)) -> f32) -> f32 {
    let (Some(first), Some(last)) = (anchors.first(), anchors.last()) else { return 0.0 };
    if x <= from(first) {
        // 第一個對位點之前（內容上方的留白）：一比一往上延伸，
        // 兩邊都捲到最頂端時，上方的留白才會一起露出來
        return to(first) + (x - from(first));
    }
    if x >= from(last) {
        return to(last);
    }
    // 第一個「起點超過 x」的對位點；x 落在它和前一個之間
    let i = anchors.partition_point(|a| from(a) <= x);
    let (a, b) = (&anchors[i - 1], &anchors[i]);
    let span = from(b) - from(a);
    if span <= 0.0 {
        return to(a);
    }
    to(a) + (to(b) - to(a)) * (x - from(a)) / span
}

/// 把一串由小到大的位元組位置換成字元位置（只掃一次文字）。
pub fn char_offsets(text: &str, bytes: &[usize]) -> Vec<usize> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut it = text.char_indices().map(|(b, _)| b).enumerate().peekable();
    let total = text.chars().count();
    for &target in bytes {
        while it.peek().is_some_and(|(_, b)| *b < target) {
            it.next();
        }
        out.push(it.peek().map_or(total, |(c, _)| *c));
    }
    out
}

/// 編輯器：排版結果中 y（相對於文字最上緣）這個高度是第幾個字，含列內的比例。
pub fn galley_char_at(galley: &Galley, y: f32) -> f32 {
    if y < 0.0 {
        // 文字上方的留白：和預覽一樣一比一延伸（見 `interpolate`）
        return y;
    }
    let mut chars = 0usize;
    for row in &galley.rows {
        let n = row.char_count_including_newline().0;
        if y < row.max_y() {
            let h = (row.max_y() - row.min_y()).max(1.0);
            return chars as f32 + ((y - row.min_y()) / h).clamp(0.0, 1.0) * n as f32;
        }
        chars += n;
    }
    chars as f32
}

/// 編輯器：第 `source` 個字（可帶小數）在排版結果中的 y（相對於文字最上緣）。
pub fn galley_y_of(galley: &Galley, source: f32) -> f32 {
    if source < 0.0 {
        return source;
    }
    let mut chars = 0.0;
    let mut bottom = 0.0;
    for row in &galley.rows {
        let n = row.char_count_including_newline().0 as f32;
        if source < chars + n {
            return row.min_y() + (source - chars).max(0.0) / n.max(1.0) * (row.max_y() - row.min_y());
        }
        chars += n;
        bottom = row.max_y();
    }
    bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_both_ways() {
        // 三個區塊：第 0、100、400 個字，畫在 0、50、80；文件共 500 字、高 300
        let a = [(0.0, 0.0), (100.0, 50.0), (400.0, 80.0), (500.0, 300.0)];
        assert_eq!(source_at(&a, 25.0), 50.0);
        assert_eq!(source_at(&a, 65.0), 250.0);
        assert_eq!(y_at(&a, 250.0), 65.0);
        assert_eq!(y_at(&a, 450.0), 190.0);
        // 最上面之前一比一延伸（上方的留白），超過結尾就停在結尾
        assert_eq!(source_at(&a, -10.0), -10.0);
        assert_eq!(y_at(&a, -10.0), -10.0);
        assert_eq!(source_at(&a, 999.0), 500.0);
        assert_eq!(y_at(&a, 9999.0), 300.0);
        // 來回一趟回到原處
        for y in [0.0, 10.0, 49.0, 50.0, 79.0, 150.0, 300.0] {
            assert!((y_at(&a, source_at(&a, y)) - y).abs() < 1e-3, "{y}");
        }
        assert_eq!(source_at(&[], 5.0), 0.0);
    }

    #[test]
    fn flat_segments_do_not_divide_by_zero() {
        // 兩個區塊在同一個位置（例如空的區塊）
        let a = [(0.0, 0.0), (10.0, 20.0), (10.0, 40.0), (30.0, 40.0), (50.0, 90.0)];
        assert!(source_at(&a, 30.0).is_finite());
        assert!(y_at(&a, 20.0).is_finite());
        assert!(y_at(&a, 10.0).is_finite());
    }

    #[test]
    fn byte_to_char_offsets() {
        let text = "一二three四";
        assert_eq!(char_offsets(text, &[0, 3, 6, 11, 14]), [0, 1, 2, 7, 8]);
        // 落在字的中間（不該發生）時取下一個字的開頭，超出結尾就是總字數
        assert_eq!(char_offsets(text, &[1, 999]), [1, 8]);
    }
}
