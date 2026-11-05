//! Markdown 小工具：解析選項、標題擷取、大綱與字數統計。

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// 全專案統一使用的 Markdown 解析選項（HackMD 相容）。
pub fn options() -> Options {
    crate::hmd::options()
}

/// 一個標題項目（用於大綱 / 目錄）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    /// 在原始文字中的位元組位置。
    pub offset: usize,
}

/// 取得文件中所有標題。
pub fn outline(md: &str) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut current: Option<(u8, String, usize)> = None;
    for (event, range) in Parser::new_ext(md, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current = Some((level_num(level), String::new(), range.start));
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, text, offset)) = current.take() {
                    out.push(Heading { level, text: text.trim().to_string(), offset });
                }
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some((_, text, _)) = current.as_mut() {
                    text.push_str(&t);
                }
            }
            _ => {}
        }
    }
    out
}

/// 文件標題：第一個一級標題，否則 front matter 的 `title`。
pub fn title(md: &str) -> Option<String> {
    crate::hmd::parse(md).title().filter(|t| !t.is_empty())
}

pub(crate) fn level_num(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// 是否為中日韓文字（用於字數統計與斷行處理）。
pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF   // 平假名、片假名
        | 0x3400..=0x4DBF // CJK 擴充 A
        | 0x4E00..=0x9FFF // CJK 統一表意文字
        | 0xF900..=0xFAFF // 相容表意文字
        | 0xAC00..=0xD7AF // 韓文
        | 0x20000..=0x2FFFF
        | 0x3000..=0x303F // CJK 標點
        | 0xFF00..=0xFFEF // 全形字元
    )
}

/// 字數統計：中日韓文字每字算一，其餘以空白分隔的單字算一。
pub fn word_count(md: &str) -> usize {
    let mut count = 0;
    let mut in_word = false;
    for event in Parser::new_ext(md, options()) {
        let text = match event {
            Event::Text(t) | Event::Code(t) => t,
            _ => {
                in_word = false;
                continue;
            }
        };
        for c in text.chars() {
            if is_cjk(c) {
                if !c.is_ascii_punctuation() && !matches!(c as u32, 0x3000..=0x303F | 0xFF00..=0xFF0F) {
                    count += 1;
                }
                in_word = false;
            } else if c.is_alphanumeric() {
                if !in_word {
                    count += 1;
                    in_word = true;
                }
            } else {
                in_word = false;
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outline_and_title() {
        let md = "# 第一章 `開始`\n\n內文\n\n## 小節\n";
        let o = outline(md);
        assert_eq!(o.len(), 2);
        assert_eq!(o[0].text, "第一章 開始");
        assert_eq!(o[1].level, 2);
        assert_eq!(title(md).as_deref(), Some("第一章 開始"));
    }

    #[test]
    fn counts_cjk_and_words() {
        assert_eq!(word_count("你好，世界 hello world"), 6);
    }
}
