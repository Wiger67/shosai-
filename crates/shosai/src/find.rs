//! 文件內尋找（Ctrl+F）：比對文字，以及尋找列的狀態。

use std::ops::Range;

/// 一個符合的位置。編輯器用字元位置（游標以字元計），畫面標示用位元組位置（切字串）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub bytes: Range<usize>,
    pub chars: Range<usize>,
}

/// 在 `hay` 裡找出所有 `needle`，由前往後、不重疊。
///
/// 不分大小寫時逐字元比較（每個字元各自轉小寫），所以回傳的位置一定對得上原文，
/// 不會因為某些字轉小寫後長度不同而錯位。
pub fn find_all(hay: &str, needle: &str, case_sensitive: bool) -> Vec<Match> {
    let fold = |c: char| if case_sensitive { c } else { c.to_lowercase().next().unwrap_or(c) };
    let needle: Vec<char> = needle.chars().map(fold).collect();
    let Some(&first) = needle.first() else { return Vec::new() };
    let hay_chars: Vec<(usize, char)> = hay.char_indices().map(|(i, c)| (i, fold(c))).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay_chars.len() {
        if hay_chars[i].1 == first && hay_chars[i..i + needle.len()].iter().map(|(_, c)| c).eq(needle.iter()) {
            let end = i + needle.len();
            let byte_end = hay_chars.get(end).map_or(hay.len(), |(b, _)| *b);
            out.push(Match { bytes: hay_chars[i].0..byte_end, chars: i..end });
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// 尋找列的狀態。
#[derive(Default)]
pub struct FindState {
    pub open: bool,
    pub query: String,
    pub replacement: String,
    /// 顯示「取代」那一列
    pub with_replace: bool,
    pub case_sensitive: bool,
    /// 目前是第幾個符合的位置（從 0 起算）
    pub current: usize,
    /// 下一幀讓尋找輸入框取得焦點，並全選裡面的字
    pub focus: bool,
    /// 下一幀把目前這個符合的位置捲到看得到的地方
    pub jump: bool,
    /// 剛輸入了新的字：從畫面目前的位置往後找第一個
    pub restart: bool,
    /// 在原始文字中符合的位置（編輯／分割模式）
    pub matches: Vec<Match>,
    /// `matches` 是依哪一版文字、哪個條件算的
    key: Option<(u64, String, bool)>,
    /// 預覽畫面上符合的數量（閱讀模式）
    pub rendered: usize,
    /// `rendered` 是依哪個條件算的
    rendered_key: Option<(String, bool)>,
}

impl FindState {
    pub fn active(&self) -> bool {
        self.open && !self.query.is_empty()
    }

    /// 文字或條件變了才重新比對。`rev` 是文字的版本號。
    pub fn update(&mut self, text: &str, rev: u64) {
        let fresh =
            self.key.as_ref().is_some_and(|(r, q, c)| *r == rev && *q == self.query && *c == self.case_sensitive);
        if fresh {
            return;
        }
        let query_changed = self.key.as_ref().is_none_or(|(_, q, c)| *q != self.query || *c != self.case_sensitive);
        self.matches = find_all(text, &self.query, self.case_sensitive);
        self.key = Some((rev, self.query.clone(), self.case_sensitive));
        if query_changed {
            self.restart = true;
        }
        self.clamp(self.matches.len());
    }

    /// 閱讀模式：畫面畫完後回報符合的數量。條件變了就從第一個開始。
    pub fn update_rendered(&mut self, count: usize) {
        let changed = self.rendered_key.as_ref().is_none_or(|(q, c)| *q != self.query || *c != self.case_sensitive);
        if changed {
            self.rendered_key = Some((self.query.clone(), self.case_sensitive));
            self.current = 0;
            self.jump = true;
        }
        self.rendered = count;
        self.clamp(count);
    }

    pub fn clamp(&mut self, count: usize) {
        if self.current >= count {
            self.current = count.saturating_sub(1);
        }
    }

    /// 往後（`forward`）或往前移到下一個，到底了就繞回另一頭。
    pub fn step(&mut self, forward: bool, count: usize) {
        if count == 0 {
            return;
        }
        self.current = if forward { (self.current + 1) % count } else { (self.current + count - 1) % count };
        self.jump = true;
    }

    /// 從第 `chars` 個字開始往後的第一個符合位置（剛輸入新的字時用，不要跳回文件開頭）。
    pub fn start_from(&mut self, chars: usize) {
        let i = self.matches.partition_point(|m| m.chars.start < chars);
        self.current = if i < self.matches.len() { i } else { 0 };
        self.jump = true;
        self.restart = false;
    }

    /// 取代目前這一個。回傳是否改了文字。
    pub fn replace_current(&mut self, text: &mut String) -> bool {
        let Some(m) = self.matches.get(self.current) else { return false };
        text.replace_range(m.bytes.clone(), &self.replacement);
        self.jump = true;
        true
    }

    /// 全部取代。回傳取代了幾個。
    pub fn replace_all(&mut self, text: &mut String) -> usize {
        let n = self.matches.len();
        // 由後往前取代，前面的位置才不會跑掉
        for m in self.matches.iter().rev() {
            text.replace_range(m.bytes.clone(), &self.replacement);
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(hay: &str, needle: &str, case: bool) -> Vec<Range<usize>> {
        find_all(hay, needle, case).into_iter().map(|m| m.chars).collect()
    }

    #[test]
    fn finds_cjk_and_ignores_case() {
        let hay = "書齋 Shosai：在書齋裡讀書，shosai。";
        assert_eq!(chars(hay, "書齋", true), [0..2, 11..13]);
        assert_eq!(chars(hay, "shosai", false), [3..9, 17..23]);
        assert_eq!(chars(hay, "shosai", true), vec![17..23]);
        assert!(chars(hay, "", false).is_empty());
        assert!(chars(hay, "找不到", false).is_empty());
        // 位元組位置可以直接切原文
        for m in find_all(hay, "SHOSAI", false) {
            assert!(hay[m.bytes].eq_ignore_ascii_case("shosai"));
        }
    }

    #[test]
    fn matches_do_not_overlap() {
        assert_eq!(chars("aaaa", "aa", true), [0..2, 2..4]);
        assert_eq!(chars("哈哈哈", "哈哈", true), vec![0..2]);
    }

    #[test]
    fn positions_survive_case_folding_that_changes_length() {
        // 「İ」轉小寫會變成兩個字元，「K」（克耳文符號）轉小寫後位元組變短
        let hay = "İstanbul K 書";
        let m = find_all(hay, "書", false);
        assert_eq!(m.len(), 1);
        assert_eq!(&hay[m[0].bytes.clone()], "書");
        assert_eq!(hay.chars().nth(m[0].chars.start), Some('書'));
    }

    #[test]
    fn stepping_wraps_and_restart_picks_the_next_one() {
        let text = "一二三 一二三 一二三";
        let mut f = FindState { open: true, query: "一二三".into(), ..Default::default() };
        f.update(text, 1);
        assert_eq!(f.matches.len(), 3);
        assert!(f.restart);
        f.start_from(5);
        assert_eq!(f.current, 2, "first match at or after the view position");
        f.step(true, 3);
        assert_eq!(f.current, 0, "wraps to the first");
        f.step(false, 3);
        assert_eq!(f.current, 2, "and back to the last");
        f.start_from(100);
        assert_eq!(f.current, 0, "nothing after the view position: start over");
    }

    #[test]
    fn replace() {
        let mut text = String::from("貓和貓，還有貓");
        let mut f = FindState { open: true, query: "貓".into(), replacement: "小狗".into(), ..Default::default() };
        f.update(&text, 1);
        f.current = 1;
        assert!(f.replace_current(&mut text));
        assert_eq!(text, "貓和小狗，還有貓");
        f.update(&text, 2);
        assert_eq!(f.matches.len(), 2);
        assert_eq!(f.replace_all(&mut text), 2);
        assert_eq!(text, "小狗和小狗，還有小狗");
        // 文字變短後，目前的位置不會超出範圍
        f.update(&text, 3);
        assert_eq!((f.matches.len(), f.current), (0, 0));
    }
}
