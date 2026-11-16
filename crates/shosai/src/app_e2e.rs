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
        let startup = Startup { open: Some(dir.join("doc.md")), ..Default::default() };
        let mut app = ShosaiApp::new(&cc, startup);
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

/// 手機大小的畫面：Galaxy S25 直放（1080×2340、縮放 3 倍 → 360×780 點）。
const PHONE: egui::Vec2 = vec2(360.0, 780.0);
/// 存圖時一點畫幾個像素。
const PHONE_PPP: f32 = 2.0;
/// 系統導覽列佔掉最下面的這麼多點。
const PHONE_NAV_BAR: f32 = 24.0;

/// 測試用的手機：導覽列佔掉最下面一條，「所有檔案存取權」可以隨時改。
#[derive(Clone)]
struct FakeDevice {
    access: std::rc::Rc<std::cell::Cell<bool>>,
    /// 書齋要求打開設定頁幾次
    requests: std::rc::Rc<std::cell::Cell<u32>>,
}

impl Default for FakeDevice {
    fn default() -> Self {
        FakeDevice { access: std::rc::Rc::new(std::cell::Cell::new(true)), requests: Default::default() }
    }
}

impl Device for FakeDevice {
    fn safe_insets(&self, _screen: egui::Vec2) -> [f32; 4] {
        // 系統回報的是實體像素
        [0.0, 0.0, 0.0, PHONE_NAV_BAR * PHONE_PPP]
    }
    fn has_all_files_access(&self) -> bool {
        self.access.get()
    }
    fn request_all_files_access(&self) {
        self.requests.set(self.requests.get() + 1);
    }
}

/// 以手機版的介面把整個程式跑起來。
struct Phone {
    ctx: egui::Context,
    app: ShosaiApp,
    /// 假的手機：可以改它回報的權限，看書齋怎麼反應
    device: FakeDevice,
    frame: eframe::Frame,
    textures: crate::shot::Textures,
    shapes: Vec<egui::epaint::ClippedShape>,
    time: f64,
    dir: PathBuf,
}

impl Phone {
    /// 書庫裡有兩本書，各兩章。
    fn new(name: &str) -> Phone {
        let dir = std::env::temp_dir().join(format!("shosai-e2e-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for book in ["雨夜的茶館", "星海航路"] {
            std::fs::create_dir_all(dir.join(book)).unwrap();
            for (file, title) in [("1.md", "第一章　出發"), ("2.md", "第二章　歸途")] {
                let mut text = format!("# {title}\n\n");
                for i in 0..60 {
                    text.push_str(&"那一天的雨下得很久，他在窗邊等了一個下午。".repeat(1 + i % 3));
                    text.push_str("「還要多久？」她問，手裡的茶已經涼了。｜茶館《ちゃかん》裡只剩下他們兩個人。\n\n");
                }
                std::fs::write(dir.join(book).join(file), text).unwrap();
            }
        }
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let device = FakeDevice::default();
        let startup = Startup {
            library: Some(dir.clone()),
            mobile: true,
            device: Some(Box::new(device.clone())),
            ..Default::default()
        };
        let mut app = ShosaiApp::new(&cc, startup);
        // 手機上的預設值（在電腦上跑測試時 `Settings::default()` 給的是桌面版的）
        app.settings.sidebar = false;
        app.settings.view = ViewMode::Read;
        app.settings.dark = Some(false);
        let mut phone = Phone {
            ctx,
            app,
            device,
            frame: eframe::Frame::_new_kittest(),
            textures: Default::default(),
            shapes: Vec::new(),
            time: 0.0,
            dir,
        };
        phone.settle();
        phone
    }

    /// 跑一幀，送入這些事件。
    fn step(&mut self, events: Vec<Event>) {
        self.time += 1.0 / 30.0;
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, PHONE)),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(PHONE_PPP);
        self.app.raw_input_hook(&self.ctx, &mut input);
        let mut out = self.ctx.run_ui(input, |ui| self.app.ui(ui, &mut self.frame));
        self.textures.update(&mut out.textures_delta);
        self.shapes = out.shapes;
    }

    /// 排版、背景的字數統計、翻頁動畫都跑完。
    fn settle(&mut self) {
        for _ in 0..40 {
            self.step(Vec::new());
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// 手指點一下。
    fn tap(&mut self, at: Pos2) {
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.step(vec![Event::PointerMoved(at), button(true)]);
        self.step(vec![button(false), Event::PointerGone]);
        self.settle();
    }

    /// 手機的返回鍵。
    fn back(&mut self) {
        let key = |pressed| Event::Key {
            key: Key::BrowserBack,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        self.step(vec![key(true)]);
        self.step(vec![key(false)]);
        self.settle();
    }

    /// 書本模式現在在哪一頁。
    fn page(&self) -> Option<(PathBuf, usize)> {
        self.app.novel.as_ref().and_then(|n| n.position())
    }

    /// 把現在的畫面存成 `target/shots/` 裡的圖片。
    fn save(&self, name: &str) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots").join(name);
        let size = [(PHONE.x * PHONE_PPP) as usize, (PHONE.y * PHONE_PPP) as usize];
        self.textures.save(&self.ctx, self.shapes.clone(), size, &path);
    }
}

impl Drop for Phone {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 手機版：書櫃 → 點一本書 → 點兩側翻頁、點中間開目錄 → 返回鍵一路退回書櫃。
#[test]
fn phone_reads_a_book_by_touch() {
    let mut p = Phone::new("phone");
    assert!(p.app.novel.is_none() && p.app.doc.is_none(), "starts on the bookshelf");
    // 畫面要避開系統導覽列：最下面那一條不能有任何會接收點擊的東西
    let safe = p.ctx.content_rect();
    assert_eq!(safe.bottom(), PHONE.y - PHONE_NAV_BAR, "the navigation bar is kept clear");

    // 書櫃上的第一本書
    p.tap(pos2(PHONE.x * 0.5, 350.0));
    assert!(p.app.novel.is_some(), "tapping a book opens it in book mode");
    assert_eq!(p.page(), None, "a book opens on its cover");
    // 直排：點左邊是下一頁。封面 → 目錄 → 第一章
    let left = pos2(PHONE.x * 0.1, PHONE.y * 0.5);
    p.tap(left);
    p.tap(left);
    let first = p.page().expect("two taps past the cover and contents reach the first chapter");
    assert_eq!(first.1, 0, "at the start of the chapter");
    p.tap(left);
    let second = p.page().expect("still in the chapter");
    assert!(second.0 == first.0 && second.1 > 0, "tapping the left edge turns the page: {second:?}");
    // 點右邊往回
    p.tap(pos2(PHONE.x * 0.9, PHONE.y * 0.5));
    assert_eq!(p.page(), Some(first), "tapping the right edge goes back");

    // 書裡的目錄頁（封面的下一頁）：點章名就跳到那一章
    p.tap(pos2(PHONE.x * 0.9, PHONE.y * 0.5));
    p.tap(pos2(PHONE.x * 0.9, PHONE.y * 0.5));
    assert_eq!(p.page(), None, "back on the cover");
    p.tap(left);
    assert_eq!(p.page(), None, "the contents page is still front matter");
    // 直排由右往左：第一欄是「目錄」兩個字，接著是第一章、第二章
    p.tap(pos2(194.5, 170.0));
    let jumped = p.page().expect("tapping a chapter on the contents page opens it");
    assert!(jumped.0.ends_with("2.md") && jumped.1 == 0, "the second chapter: {jumped:?}");

    // 點中間開目錄；返回鍵先關目錄，再回書櫃
    p.tap(pos2(PHONE.x * 0.5, PHONE.y * 0.5));
    assert!(p.app.novel.as_ref().is_some_and(|n| n.show_toc), "tapping the middle opens the contents");
    // 狀態列在導覽列上方，而且文字不會長到左右疊在一起
    let (left, right) = p.app.novel.as_ref().unwrap().status();
    assert!(left.chars().count() + right.chars().count() < 30, "a short status line: {left} | {right}");
    p.back();
    assert!(p.app.novel.as_ref().is_some_and(|n| !n.show_toc), "back closes the contents first");
    p.back();
    assert!(p.app.novel.is_none() && p.app.doc.is_none(), "back again returns to the bookshelf");
}

/// 手機版沒有系統的檔案對話框：書櫃上的「選擇書庫資料夾」打開自己的選擇畫面，
/// 一層一層點進去，選好之後那個資料夾就是書庫，下次開啟也記得。
#[test]
fn phone_picks_its_library_folder() {
    let mut p = Phone::new("phone-picker");
    let library = p.dir.clone();
    assert!(!p.app.settings.library_picked, "the library it starts with was found, not picked");
    let pick = pos2(PHONE.x * 0.5, 462.0);
    // 返回鍵關掉選擇畫面，書庫不變
    p.tap(pick);
    assert!(p.app.folder_picker.is_some(), "the button on the bookshelf opens the folder picker");
    p.back();
    assert!(p.app.folder_picker.is_none(), "back closes it");
    assert_eq!(p.app.library.as_ref().map(|l| l.root.clone()), Some(library.clone()));

    // 點進第一個資料夾，按「用這個資料夾」
    p.tap(pick);
    p.tap(pos2(PHONE.x * 0.5, 267.0));
    p.tap(pos2(89.0, 181.0));
    assert!(p.app.folder_picker.is_none(), "choosing closes the picker");
    let chosen = library.join("星海航路");
    assert_eq!(p.app.library.as_ref().map(|l| l.root.clone()), Some(chosen.clone()), "the folder is now the library");
    assert!(p.app.settings.library_picked);

    // 下次啟動（設定存起來再讀回來）：自動找到的書庫不會蓋掉使用者選的
    let mut saved = MemoryStorage::default();
    p.app.save(&mut saved);
    let relaunch = |saved: &MemoryStorage| {
        let mut cc = eframe::CreationContext::_new_kittest(egui::Context::default());
        cc.storage = Some(saved);
        let startup = Startup { library: Some(library.clone()), mobile: true, ..Default::default() };
        ShosaiApp::new(&cc, startup).library.map(|l| l.root)
    };
    assert_eq!(relaunch(&saved), Some(chosen.clone()), "the picked library is kept on the next launch");
    // 沒有存過任何設定（第一次開啟）：用自動找到的
    assert_eq!(relaunch(&MemoryStorage::default()), Some(library.clone()), "a first launch uses the found library");
    // 選的資料夾不見了（例如 Nextcloud 那邊刪掉了）：退回自動找到的
    std::fs::remove_dir_all(&chosen).unwrap();
    assert_eq!(relaunch(&saved), Some(library.clone()), "a picked library that is gone falls back to the found one");
}

/// 沒有「所有檔案存取權」時（Android 只讓書齋看到資料夾名稱、看不到裡面的文章）：
/// 書櫃上方說明原因，按鈕打開設定頁；允許之後一回到書齋就重新讀書庫。
#[test]
fn phone_asks_for_file_access() {
    let mut p = Phone::new("phone-access");
    assert!(p.app.files_access.is_some_and(|(ok, _)| ok), "asked the device once at start");
    p.device.access.set(false);
    // 重新問一次（實際上是隔一秒自動再問）
    p.app.files_access = None;
    p.settle();
    p.save("mobile-9-access.png");
    p.tap(pos2(180.0, 423.0));
    assert_eq!(p.device.requests.get(), 1, "the button opens the settings page");

    // 沒權限時打開一本書：說明是權限的問題，而不是「沒有 Markdown 檔」
    let empty = p.dir.join("看不到內容");
    std::fs::create_dir_all(&empty).unwrap();
    p.app.refresh();
    p.app.open_novel(&empty, None);
    assert!(
        matches!(&p.app.status, Some(Status::Error(msg, _)) if msg.contains("所有檔案存取權")),
        "explains why the book is empty"
    );

    // 使用者在設定頁允許了，回到書齋：重新讀書庫，新的書馬上出現
    std::fs::create_dir_all(p.dir.join("新同步的書")).unwrap();
    std::fs::write(p.dir.join("新同步的書/1.md"), "# 第一章\n\n內文。\n").unwrap();
    p.device.access.set(true);
    p.app.files_access = Some((false, Instant::now() - ACCESS_RECHECK));
    p.settle();
    assert!(p.app.files_access.is_some_and(|(ok, _)| ok));
    let lib = p.app.library.as_ref().unwrap();
    assert!(lib.entries.iter().any(|e| e.name == "新同步的書"), "the library was read again");
}

/// 放在記憶體裡的設定檔（測試「存起來、下次啟動再讀回來」）。
#[derive(Default)]
struct MemoryStorage(HashMap<String, String>);

impl eframe::Storage for MemoryStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.to_string(), value);
    }
    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }
    fn flush(&mut self) {}
}

/// 把手機版的幾個畫面存成圖片（`target/shots/mobile-*.png`），用眼睛檢查版面：
/// `cargo test -p shosai mobile_shots -- --ignored`
#[test]
#[ignore = "writes PNGs for looking at; run by hand"]
fn mobile_shots() {
    let mut p = Phone::new("mobile-shots");
    // 測試環境沒有中文字型（程式執行時由系統字型補上）：找得到就裝一個，圖片裡才看得到字
    if let Ok(bytes) = std::fs::read("C:/Windows/Fonts/msjh.ttc") {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert("cjk".into(), egui::FontData::from_owned(bytes).into());
        fonts.families.entry(egui::FontFamily::Proportional).or_default().push("cjk".into());
        p.ctx.set_fonts(fonts);
    }
    p.settle();
    p.save("mobile-1-shelf.png");
    p.tap(pos2(PHONE.x * 0.5, 350.0));
    p.save("mobile-2-cover.png");
    for _ in 0..3 {
        p.tap(pos2(PHONE.x * 0.1, PHONE.y * 0.5));
    }
    p.save("mobile-3-vertical.png");
    p.app.settings.novel.vertical = false;
    p.settle();
    p.save("mobile-4-horizontal.png");
    p.tap(pos2(PHONE.x * 0.5, PHONE.y * 0.5));
    p.save("mobile-5-toc.png");
    p.app.settings.dark = Some(true);
    p.back();
    p.save("mobile-6-dark.png");
    p.app.settings.dark = Some(false);
    p.back();
    p.save("mobile-7-shelf.png");
    p.app.actions.push(Action::PickLibrary);
    p.settle();
    p.save("mobile-8-picker.png");
}
