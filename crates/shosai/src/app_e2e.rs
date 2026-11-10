//! 端對端測試：不開視窗，把整個程式跑起來，送入模擬的滾輪與鍵盤事件，
//! 檢查尋找、大綱、分割模式捲動同步的實際行為。

use eframe::egui::{self, pos2, vec2, Event, Key, Modifiers, Pos2, Rect};
use eframe::App as _;

use super::*;

const SCREEN: egui::Vec2 = vec2(1300.0, 720.0);

/// 一份夠長、內容多樣的文件：12 節，每節有長短不一的段落，夾雜清單、程式碼、引言、提示框。
fn sample() -> String {
    let mut s = String::from("---\ntitle: 範例\n---\n\n# 捲動同步測試\n\n開頭的說明文字。\n\n");
    for i in 1..=12 {
        s.push_str(&format!("## 第 {i} 節\n\n"));
        for p in 0..(1 + i % 3) {
            s.push_str(&"這是一段用來測試的內文，長度會自動換行。".repeat(2 + (i + p) % 4));
            s.push_str("\n\n");
        }
        match i % 4 {
            0 => s.push_str("- 清單一\n- 清單二\n- 清單三\n\n"),
            1 => s.push_str("```rust\nfn main() {\n    println!(\"關鍵字\");\n}\n```\n\n"),
            2 => s.push_str("> 引言裡的一段話，關鍵字也在這裡。\n\n"),
            _ => s.push_str(":::info\n提示框裡的內容。\n:::\n\n"),
        }
        s.push_str(&format!("### 第 {i} 節的小標\n\n小標底下的關鍵字段落。\n\n"));
    }
    s
}

struct Harness {
    ctx: egui::Context,
    app: ShosaiApp,
    frame: eframe::Frame,
    time: f64,
    dir: PathBuf,
    text: String,
    /// 上一幀結束時，程式要求多久之後重畫（零 = 馬上再畫一次）
    repaint_after: std::time::Duration,
    modifiers: Modifiers,
}

impl Harness {
    fn new(name: &str, view: ViewMode) -> Harness {
        let dir = std::env::temp_dir().join(format!("shosai-e2e-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let text = sample();
        std::fs::write(dir.join("doc.md"), &text).unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = ShosaiApp::new(&cc, Some(dir.join("doc.md")));
        app.settings.view = view;
        assert!(app.doc.is_some(), "the document opened");
        let mut h = Harness {
            ctx,
            app,
            frame: eframe::Frame::_new_kittest(),
            time: 0.0,
            dir,
            text,
            repaint_after: Default::default(),
            modifiers: Modifiers::NONE,
        };
        h.run(5);
        h
    }

    /// 跑一幀，送入這些事件。
    fn step(&mut self, mut events: Vec<Event>, modifiers: Modifiers) {
        self.time += 1.0 / 60.0;
        // only when they change: an event every frame would itself keep the app repainting
        if modifiers != self.modifiers {
            self.modifiers = modifiers;
            events.insert(0, Event::ModifiersChanged(modifiers));
        }
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let mut out = self.ctx.run_ui(input, |ui| self.app.ui(ui, &mut self.frame));
        // egui 不允許貼圖更新沒人處理就丟掉
        out.textures_delta.clear();
        self.repaint_after = out
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.repaint_delay)
            .unwrap_or(std::time::Duration::MAX);
    }

    /// 沒有輸入地跑幾幀（讓捲動動畫、跟隨捲動跑完）。
    fn run(&mut self, frames: usize) {
        for _ in 0..frames {
            self.step(Vec::new(), Modifiers::NONE);
        }
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        let down = Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        let up = Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers };
        self.step(vec![down], modifiers);
        self.step(vec![up], modifiers);
    }

    fn ctrl(&mut self, key: Key) {
        self.key(key, Modifiers { ctrl: true, command: true, ..Default::default() });
    }

    fn type_text(&mut self, text: &str) {
        self.step(vec![Event::Text(text.to_string())], Modifiers::NONE);
        self.run(2);
    }

    /// 把滑鼠移到 `at`，往下滾 `notches` 格，然後等捲動停下來。
    fn wheel(&mut self, at: Pos2, notches: f32) {
        self.step(vec![Event::PointerMoved(at)], Modifiers::NONE);
        let wheel = Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: vec2(0.0, -notches),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: Modifiers::NONE,
        };
        self.step(vec![wheel], Modifiers::NONE);
        self.run(90);
    }

    fn click(&mut self, at: Pos2) {
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.step(vec![Event::PointerMoved(at)], Modifiers::NONE);
        self.step(vec![button(true)], Modifiers::NONE);
        self.step(vec![button(false)], Modifiers::NONE);
        self.run(60);
    }

    fn preview_pos(&self) -> Pos2 {
        self.app.preview_rect.expect("split mode shows a preview").center()
    }

    fn editor_pos(&self) -> Pos2 {
        let preview = self.app.preview_rect.expect("split mode shows a preview");
        pos2(preview.left() - 150.0, preview.center().y)
    }

    /// 原始文字第 `chars` 個字所在的那一行。
    fn line_at(&self, chars: f32) -> String {
        let before: String = self.text.chars().take(chars.max(0.0) as usize).collect();
        let line = before.matches('\n').count();
        self.text.lines().nth(line).unwrap_or("").to_string()
    }

    /// 第 `chars` 個字之前最後一個標題的文字。
    fn heading_before(&self, chars: f32) -> String {
        let items = self.app.md_view.outline();
        items.iter().rev().find(|h| h.chars as f32 <= chars + 1.0).map(|h| h.text.clone()).unwrap_or_default()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 兩邊對齊的容許誤差（字元）：不到一列。
const ALIGNED: f32 = 3.0;

fn assert_aligned(h: &Harness, what: &str) {
    let (editor, preview) = (h.app.editor_view.top, h.app.md_view.view_top);
    assert!(
        (editor - preview).abs() < ALIGNED,
        "{what}: editor shows char {editor:.1} ({:?}) but preview shows {preview:.1} ({:?})",
        h.line_at(editor),
        h.line_at(preview)
    );
}

#[test]
fn split_scrolling_the_editor_moves_the_preview() {
    let mut h = Harness::new("sync-editor", ViewMode::Split);
    assert_aligned(&h, "at the top");
    let mut last = h.app.editor_view.top;
    for notches in [3.0, 5.0, 10.0, 20.0] {
        h.wheel(h.editor_pos(), notches);
        assert!(h.app.editor_view.top > last + 10.0, "the editor scrolled down");
        last = h.app.editor_view.top;
        assert_aligned(&h, "after scrolling the editor down");
    }
    h.wheel(h.editor_pos(), -12.0);
    assert!(h.app.editor_view.top < last - 10.0, "the editor scrolled back up");
    assert_aligned(&h, "after scrolling the editor up");
}

#[test]
fn split_scrolling_the_preview_moves_the_editor() {
    let mut h = Harness::new("sync-preview", ViewMode::Split);
    let mut last = h.app.md_view.view_top;
    for notches in [4.0, 9.0, 15.0] {
        h.wheel(h.preview_pos(), notches);
        assert!(h.app.md_view.view_top > last + 10.0, "the preview scrolled down");
        last = h.app.md_view.view_top;
        assert_aligned(&h, "after scrolling the preview down");
    }
    h.wheel(h.preview_pos(), -10.0);
    assert!(h.app.md_view.view_top < last - 10.0, "the preview scrolled back up");
    assert_aligned(&h, "after scrolling the preview up");
    // 回到編輯區捲：換編輯區主導，預覽跟上
    h.wheel(h.editor_pos(), 6.0);
    assert_aligned(&h, "after handing control back to the editor");
}

#[test]
fn split_sync_can_be_turned_off() {
    let mut h = Harness::new("sync-off", ViewMode::Split);
    h.app.settings.sync_scroll = false;
    let preview = h.app.md_view.view_top;
    h.wheel(h.editor_pos(), 10.0);
    assert!(h.app.editor_view.top > 100.0);
    assert_eq!(h.app.md_view.view_top, preview, "the preview stays where it was");
}

#[test]
fn switching_modes_keeps_the_reading_position() {
    let mut h = Harness::new("modes", ViewMode::Read);
    h.wheel(pos2(SCREEN.x * 0.6, SCREEN.y * 0.5), 14.0);
    let read = h.app.md_view.view_top;
    assert!(read > 200.0, "the reader scrolled, got {read}");
    h.app.settings.view = ViewMode::Edit;
    h.run(30);
    let edit = h.app.editor_view.top;
    assert!((edit - read).abs() < ALIGNED, "reader was at {read:.1}, editor opened at {edit:.1}");

    h.wheel(pos2(SCREEN.x * 0.6, SCREEN.y * 0.5), 9.0);
    let edit = h.app.editor_view.top;
    assert!(edit > read + 50.0);
    h.app.settings.view = ViewMode::Read;
    h.run(30);
    let read = h.app.md_view.view_top;
    assert!((edit - read).abs() < ALIGNED, "editor was at {edit:.1}, reader opened at {read:.1}");
}

#[test]
fn outline_lists_headings_and_follows_the_position() {
    let mut h = Harness::new("outline", ViewMode::Edit);
    h.app.settings.outline = true;
    h.run(3);
    let items = h.app.md_view.outline().to_vec();
    assert_eq!(items.len(), 1 + 12 * 2, "one title, twelve sections each with a sub-heading");
    assert_eq!(items[0].text, "捲動同步測試");
    assert_eq!((items[1].level, items[1].text.as_str()), (2, "第 1 節"));
    assert_eq!((items[2].level, items[2].text.as_str()), (3, "第 1 節的小標"));
    for it in &items {
        assert!(h.text.chars().skip(it.chars).collect::<String>().starts_with('#'), "{it:?} points at its own line");
    }
    assert_eq!(h.app.outline_current, Some(0), "at the top the title is current");

    // 編輯模式：跳到某個標題，大綱標示的就是它
    let seven = items.iter().position(|i| i.text == "第 7 節").unwrap();
    h.app.editor_goto = Some(EditorGoto::Heading(items[seven].chars));
    h.run(60);
    assert_eq!(h.app.outline_current, Some(seven));
    assert_eq!(h.heading_before(h.app.editor_view.probe), "第 7 節");

    // 捲動時跟著換
    h.wheel(pos2(500.0, 400.0), 8.0);
    let now = h.app.outline_current.unwrap();
    assert!(now > seven, "scrolling down moves the highlight forward, got {now}");
    assert_eq!(items[now].text, h.heading_before(h.app.editor_view.probe));

    // 閱讀模式：看的是預覽的位置
    h.app.settings.view = ViewMode::Read;
    h.run(10);
    let three = items.iter().position(|i| i.text == "第 3 節").unwrap();
    h.app.md_view.scroll_to(&items[three].id);
    h.run(60);
    assert_eq!(h.app.outline_current, Some(three));
}

#[test]
fn clicking_the_outline_jumps_to_that_heading() {
    for view in [ViewMode::Edit, ViewMode::Split, ViewMode::Read] {
        let mut h = Harness::new("outline-click", view);
        h.app.settings.outline = true;
        h.run(5);
        let x = SCREEN.x - 110.0;
        // 大綱每一列高 26、間隔 1。點兩次相隔四列的位置，應該跳到相隔四個的標題
        h.click(pos2(x, 150.0));
        let first = h.app.outline_current.expect("something is current");
        assert!(first > 0, "{view:?}: the click jumped away from the title");
        h.click(pos2(x, 150.0 + 27.0 * 4.0));
        assert_eq!(h.app.outline_current, Some(first + 4), "{view:?}");
        if view == ViewMode::Split {
            assert_aligned(&h, "split view after an outline jump");
        }
    }
}

fn editor_selection_range(h: &Harness) -> Option<(usize, usize)> {
    let id = editor_id(&h.app.doc.as_ref()?.path);
    let range = egui::TextEdit::load_state(&h.ctx, id)?.cursor.char_range()?;
    let [a, b] = range.sorted_cursors().map(|c| c.index.0);
    Some((a, b))
}

#[test]
fn find_in_the_editor() {
    let mut h = Harness::new("find-edit", ViewMode::Edit);
    let expected = h.text.matches("關鍵字").count();
    assert_eq!(expected, 18);
    h.ctrl(Key::F);
    assert!(h.app.find.open);
    h.type_text("關鍵字");
    assert_eq!(h.app.find.query, "關鍵字", "typing goes into the find box");
    assert_eq!(h.app.find.matches.len(), expected);
    assert_eq!(h.app.find.current, 0);

    // Enter 往下一個；找到的地方捲進畫面並選取起來
    for i in 1..=6 {
        h.key(Key::Enter, Modifiers::NONE);
        h.run(40);
        assert_eq!(h.app.find.current, i);
    }
    let m = h.app.find.matches[6].chars.clone();
    let top = h.app.editor_view.top;
    assert!(top <= m.start as f32 && (m.start as f32) < top + 1500.0, "match at {} is on screen (top {top})", m.start);

    // Shift+Enter 往回、F3 往下，到底會繞回頭
    h.key(Key::Enter, Modifiers::SHIFT);
    assert_eq!(h.app.find.current, 5);
    h.key(Key::F3, Modifiers::NONE);
    assert_eq!(h.app.find.current, 6);
    for _ in 0..(expected - 6) {
        h.key(Key::F3, Modifiers::NONE);
    }
    assert_eq!(h.app.find.current, 0, "wraps around to the first match");

    // Esc 關閉：焦點回到編輯區，目前找到的那一個被選取起來，接著打字就會換掉它
    let m = h.app.find.matches[0].chars.clone();
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.app.find.open);
    h.run(3);
    let id = editor_id(&h.app.doc.as_ref().unwrap().path);
    assert!(h.ctx.memory(|m| m.has_focus(id)), "focus is back in the editor");
    assert_eq!(editor_selection_range(&h), Some((m.start, m.end)));
    h.type_text("重點");
    let doc = h.app.doc.as_ref().unwrap();
    assert_eq!(doc.text.matches("關鍵字").count(), expected - 1);
    assert_eq!(doc.text.chars().skip(m.start).take(2).collect::<String>(), "重點");
}

#[test]
fn replace_in_the_editor() {
    let mut h = Harness::new("replace", ViewMode::Edit);
    h.ctrl(Key::H);
    assert!(h.app.find.open && h.app.find.with_replace);
    h.type_text("關鍵字");
    h.app.find.replacement = "重點".into();
    // 在「取代為」那一格按 Enter：取代目前這一個
    h.ctx.memory_mut(|m| m.request_focus(egui::Id::unique("shosai-replace")));
    h.run(2);
    h.key(Key::Enter, Modifiers::NONE);
    h.run(3);
    let doc = h.app.doc.as_ref().unwrap();
    assert_eq!(doc.text.matches("關鍵字").count(), 17);
    assert_eq!(doc.text.matches("重點").count(), 1);
    assert!(doc.dirty, "the document is marked as changed");
    assert_eq!(h.app.find.matches.len(), 17, "the remaining matches were found again");
}

#[test]
fn find_in_the_reader() {
    let mut h = Harness::new("find-read", ViewMode::Read);
    h.ctrl(Key::F);
    h.type_text("關鍵字");
    h.run(5);
    // 內文、引言、程式碼區塊裡的都算
    assert_eq!(h.app.find.rendered, 18);
    assert_eq!(h.app.find.current, 0);
    let start = h.app.md_view.view_top;
    for _ in 0..8 {
        h.key(Key::Enter, Modifiers::NONE);
        h.run(40);
    }
    assert_eq!(h.app.find.current, 8);
    assert!(h.app.md_view.view_top > start + 300.0, "the reader scrolled to the match, at {}", h.app.md_view.view_top);

    // 不分大小寫；勾選後只找完全相同的
    h.ctrl(Key::F);
    h.type_text("PRINTLN");
    h.run(5);
    assert_eq!(h.app.find.rendered, 3);
    h.app.find.case_sensitive = true;
    h.run(5);
    assert_eq!(h.app.find.rendered, 0);
}

/// 閒置時不能一直重畫（這個程式的賣點之一是閒置 0% CPU）。
/// 同步、大綱、尋找都開著，捲動停下來之後，不應該還在要求「馬上再畫一幀」。
#[test]
fn nothing_keeps_repainting_when_idle() {
    let idle = std::time::Duration::from_millis(100);
    let mut h = Harness::new("idle", ViewMode::Split);
    h.run(120);
    assert!(h.repaint_after > idle, "plain split view: {:?} {:?}", h.repaint_after, h.ctx.repaint_causes());
    h.app.settings.outline = true;
    h.run(120);
    assert!(h.repaint_after > idle, "with the outline: {:?} {:?}", h.repaint_after, h.ctx.repaint_causes());
    h.wheel(h.editor_pos(), 12.0);
    h.run(120);
    assert!(h.repaint_after > idle, "after editor scroll: {:?} {:?}", h.repaint_after, h.ctx.repaint_causes());
    h.wheel(h.preview_pos(), 5.0);
    h.run(120);
    assert!(h.repaint_after > idle, "after preview scroll: {:?} {:?}", h.repaint_after, h.ctx.repaint_causes());
    h.ctrl(Key::F);
    h.type_text("關鍵字");
    h.key(Key::Enter, Modifiers::NONE);
    h.run(120);
    for _ in 0..10 {
        h.run(1);
        assert!(h.repaint_after > idle, "with find open: {:?} {:?}", h.repaint_after, h.ctx.repaint_causes());
    }
    // 閱讀模式也一樣
    h.app.settings.view = ViewMode::Read;
    h.key(Key::Enter, Modifiers::NONE);
    h.run(120);
    assert!(h.repaint_after > idle, "reader: {:?} {:?}", h.repaint_after, h.ctx.repaint_causes());
}
