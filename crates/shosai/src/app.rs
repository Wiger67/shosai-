//! 視窗介面（egui）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32, FontId, Key, KeyboardShortcut, Modifiers, RichText, TextStyle};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use serde::{Deserialize, Serialize};
use shosai_core::book::{is_book, BookMeta};
use shosai_core::{library, markdown, Book, Entry, EntryKind, Library};

const AUTOSAVE_AFTER: Duration = Duration::from_millis(1200);

pub fn run(open: Option<PathBuf>) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("書齋")
            .with_app_id("shosai")
            .with_inner_size([1120.0, 740.0])
            .with_min_inner_size([480.0, 320.0]),
        ..Default::default()
    };
    eframe::run_native("shosai", options, Box::new(move |cc| Ok(Box::new(ShosaiApp::new(cc, open)))))
}

// ---------------------------------------------------------------------------
// 狀態

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum ViewMode {
    Edit,
    Split,
    Read,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    library_root: Option<PathBuf>,
    last_file: Option<PathBuf>,
    recent_libraries: Vec<PathBuf>,
    view: ViewMode,
    /// None = 跟隨系統
    dark: Option<bool>,
    editor_size: f32,
    reader_size: f32,
    reader_width: f32,
    sidebar: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            library_root: None,
            last_file: None,
            recent_libraries: Vec::new(),
            view: ViewMode::Split,
            dark: None,
            editor_size: 15.0,
            reader_size: 17.0,
            reader_width: 720.0,
            sidebar: true,
        }
    }
}

struct OpenDoc {
    path: PathBuf,
    text: String,
    dirty: bool,
    last_edit: Instant,
    mtime: Option<SystemTime>,
    words: usize,
    words_stale: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Pdf,
    Html,
    Typst,
}

impl Format {
    fn ext(self) -> &'static str {
        match self {
            Format::Pdf => "pdf",
            Format::Html => "html",
            Format::Typst => "typ",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Format::Pdf => "PDF",
            Format::Html => "HTML",
            Format::Typst => "Typst",
        }
    }
}

enum Dialog {
    NewNote {
        dir: PathBuf,
        name: String,
    },
    NewFolder {
        dir: PathBuf,
        name: String,
    },
    NewBook {
        dir: PathBuf,
        title: String,
        author: String,
    },
    NewChapter {
        book: PathBuf,
        title: String,
    },
    Rename {
        path: PathBuf,
        name: String,
    },
    Trash {
        path: PathBuf,
    },
    BookSettings {
        root: PathBuf,
        meta: Box<BookMeta>,
    },
    /// Nextcloud 同步設定
    #[cfg_attr(not(feature = "nextcloud"), allow(dead_code))]
    Nextcloud {
        server: String,
        user: String,
        password: String,
        remote_dir: String,
        insecure: bool,
    },
    /// 同步結果
    Report(String),
    About,
}

enum Action {
    Open(PathBuf),
    OpenLibrary(PathBuf),
    PickLibrary,
    Dialog(Dialog),
    MoveChapter {
        book: PathBuf,
        index: usize,
        delta: isize,
    },
    Export {
        source: PathBuf,
        format: Format,
    },
    Reveal(PathBuf),
    Refresh,
    #[cfg_attr(not(feature = "nextcloud"), allow(dead_code))]
    Sync,
}

enum Status {
    Info(String, Instant),
    Error(String, Instant),
    Exported(PathBuf),
    Working(String),
}

pub struct ShosaiApp {
    settings: Settings,
    library: Option<Library>,
    books: HashMap<PathBuf, Book>,
    doc: Option<OpenDoc>,
    md_cache: CommonMarkCache,
    dialog: Option<Dialog>,
    actions: Vec<Action>,
    status: Option<Status>,
    export_job: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    sync_job: Option<mpsc::Receiver<Result<String, String>>>,
    filter: String,
    first_frame: bool,
    applied_theme: Option<Option<bool>>,
}

impl ShosaiApp {
    fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let settings: Settings = cc.storage.and_then(|s| eframe::get_value(s, eframe::APP_KEY)).unwrap_or_default();

        cc.egui_ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(8.0, 3.0);
            style.text_styles.insert(TextStyle::Body, FontId::proportional(14.5));
            style.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
            style.url_in_tooltip = true;
        });

        let mut app = ShosaiApp {
            settings,
            library: None,
            books: HashMap::new(),
            doc: None,
            md_cache: CommonMarkCache::default(),
            dialog: None,
            actions: Vec::new(),
            status: None,
            export_job: None,
            sync_job: None,
            filter: String::new(),
            first_frame: true,
            applied_theme: None,
        };

        // 決定開啟的書庫與檔案：命令列參數 > 上次狀態
        let open = open.and_then(|p| p.canonicalize().ok());
        match open {
            Some(p) if p.is_dir() => app.open_library(&p),
            Some(p) if p.is_file() => {
                let in_library = app.settings.library_root.as_ref().is_some_and(|r| p.starts_with(r) && r.is_dir());
                if in_library {
                    let root = app.settings.library_root.clone().unwrap();
                    app.open_library(&root);
                } else if let Some(parent) = p.parent() {
                    app.open_library(parent);
                }
                app.open_file(&p);
            }
            _ => {
                if let Some(root) = app.settings.library_root.clone().filter(|r| r.is_dir()) {
                    app.open_library(&root);
                    if let Some(last) = app.settings.last_file.clone().filter(|f| f.is_file()) {
                        app.open_file(&last);
                    }
                }
            }
        }
        app
    }

    // -----------------------------------------------------------------------
    // 檔案操作

    fn open_library(&mut self, root: &Path) {
        match Library::open(root) {
            Ok(lib) => {
                self.settings.library_root = Some(root.to_path_buf());
                self.settings.recent_libraries.retain(|r| r != root);
                self.settings.recent_libraries.insert(0, root.to_path_buf());
                self.settings.recent_libraries.truncate(8);
                self.library = Some(lib);
                self.reload_books();
            }
            Err(e) => self.error(format!("無法開啟資料夾：{e}")),
        }
    }

    fn refresh(&mut self) {
        if let Some(lib) = self.library.as_mut() {
            if let Err(e) = lib.refresh() {
                self.status = Some(Status::Error(e.to_string(), Instant::now()));
            }
        }
        self.reload_books();
    }

    fn reload_books(&mut self) {
        self.books.clear();
        let Some(lib) = &self.library else { return };
        let mut roots = Vec::new();
        if is_book(&lib.root) {
            roots.push(lib.root.clone());
        }
        fn walk(entries: &[Entry], out: &mut Vec<PathBuf>) {
            for e in entries {
                if e.kind == EntryKind::Book {
                    out.push(e.path.clone());
                }
                walk(&e.children, out);
            }
        }
        walk(&lib.entries, &mut roots);
        for root in roots {
            if let Ok(book) = Book::open(&root) {
                self.books.insert(root, book);
            }
        }
    }

    fn open_file(&mut self, path: &Path) {
        self.save_current();
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let words = markdown::word_count(&text);
                self.doc = Some(OpenDoc {
                    path: path.to_path_buf(),
                    text,
                    dirty: false,
                    last_edit: Instant::now(),
                    mtime: mtime(path),
                    words,
                    words_stale: false,
                });
                self.settings.last_file = Some(path.to_path_buf());
                self.md_cache = CommonMarkCache::default();
            }
            Err(e) => self.error(format!("無法開啟 {}：{e}", path.display())),
        }
    }

    fn save_current(&mut self) {
        let Some(doc) = self.doc.as_mut() else { return };
        if !doc.dirty {
            return;
        }
        match std::fs::write(&doc.path, &doc.text) {
            Ok(()) => {
                doc.dirty = false;
                doc.mtime = mtime(&doc.path);
                // 標題可能改了 → 更新書本章節名稱
                let path = doc.path.clone();
                if let Some(book) = self.books.values_mut().find(|b| b.chapters.iter().any(|c| c.path == path)) {
                    let _ = book.refresh();
                }
            }
            Err(e) => {
                let msg = format!("儲存失敗：{e}");
                self.error(msg);
            }
        }
    }

    /// 視窗重新取得焦點時，若檔案被外部（例如 Nextcloud 同步）修改且尚未編輯，則重新載入。
    fn reload_if_changed(&mut self) {
        let Some(doc) = self.doc.as_ref() else { return };
        if doc.dirty {
            return;
        }
        let path = doc.path.clone();
        if mtime(&path) != doc.mtime {
            if path.is_file() {
                self.open_file(&path);
                self.info("檔案已在外部更新，已重新載入");
            }
            self.refresh();
        }
    }

    fn info(&mut self, msg: impl Into<String>) {
        self.status = Some(Status::Info(msg.into(), Instant::now()));
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.status = Some(Status::Error(msg.into(), Instant::now()));
    }

    fn book_of(&self, path: &Path) -> Option<&Book> {
        self.books.values().find(|b| b.chapters.iter().any(|c| c.path == path))
    }

    /// 閱讀模式的「上一章 / 下一章」順序：書本章節，否則為同資料夾的檔案。
    fn reading_sequence(&self, path: &Path) -> Vec<(PathBuf, String)> {
        if let Some(book) = self.book_of(path) {
            return book.chapters.iter().map(|c| (c.path.clone(), c.title.clone())).collect();
        }
        let Some(dir) = path.parent() else { return Vec::new() };
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| shosai_core::is_markdown(p)).collect())
            .unwrap_or_default();
        files.sort_by(|a, b| library::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
        files
            .into_iter()
            .map(|p| {
                let name = shosai_core::book::title_from_filename(&p);
                (p, name)
            })
            .collect()
    }

    fn start_export(&mut self, source: PathBuf, format: Format, ctx: &egui::Context) {
        if self.export_job.is_some() {
            self.info("上一個輸出還在進行中");
            return;
        }
        self.save_current();
        let doc = match crate::cli::load_document(&source) {
            Ok(d) => d,
            Err(e) => return self.error(e),
        };
        let default = crate::cli::default_output(&source, &doc.title, format.ext());
        let mut dialog = rfd::FileDialog::new()
            .set_title(format!("輸出 {}", format.label()))
            .add_filter(format.label(), &[format.ext()])
            .set_file_name(default.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        if let Some(dir) = default.parent() {
            dialog = dialog.set_directory(dir);
        }
        let Some(mut out) = dialog.save_file() else { return };
        if out.extension().is_none() {
            out.set_extension(format.ext());
        }

        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = shosai_core::export::export_to(&doc, &out).map(|_| out).map_err(|e| e.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.export_job = Some(rx);
        self.status = Some(Status::Working(format!("正在輸出 {}…", format.label())));
    }

    #[cfg(feature = "nextcloud")]
    fn start_sync(&mut self, ctx: &egui::Context) {
        use shosai_core::sync::SyncConfig;
        let Some(root) = self.library.as_ref().map(|l| l.root.clone()) else { return };
        if self.sync_job.is_some() {
            return;
        }
        if !SyncConfig::path(&root).is_file() {
            self.dialog = Some(nextcloud_dialog(&root));
            return;
        }
        self.save_current();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = SyncConfig::load(&root)
                .and_then(|cfg| shosai_core::sync::sync(&root, &cfg))
                .map(|r| r.to_string())
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.sync_job = Some(rx);
        self.status = Some(Status::Working("正在與 Nextcloud 同步…".into()));
    }

    #[cfg(not(feature = "nextcloud"))]
    fn start_sync(&mut self, _ctx: &egui::Context) {}

    fn poll_sync(&mut self) {
        let Some(rx) = &self.sync_job else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.sync_job = None;
        match result {
            Ok(report) => {
                self.status = None;
                self.reload_if_changed();
                self.refresh();
                if report.lines().count() > 1 {
                    self.dialog = Some(Dialog::Report(report));
                } else {
                    self.info(report.trim().to_string());
                }
            }
            Err(e) => self.error(e),
        }
    }

    fn process_actions(&mut self, ctx: &egui::Context) {
        for action in std::mem::take(&mut self.actions) {
            match action {
                Action::Open(p) => self.open_file(&p),
                Action::OpenLibrary(p) => {
                    self.save_current();
                    self.doc = None;
                    self.open_library(&p);
                }
                Action::PickLibrary => {
                    if let Some(dir) = rfd::FileDialog::new().set_title("選擇書庫資料夾").pick_folder() {
                        self.save_current();
                        self.doc = None;
                        self.open_library(&dir);
                    }
                }
                Action::Dialog(d) => self.dialog = Some(d),
                Action::MoveChapter { book, index, delta } => {
                    if let Some(b) = self.books.get_mut(&book) {
                        if let Err(e) = b.move_chapter(index, delta) {
                            self.error(e.to_string());
                        }
                    }
                }
                Action::Export { source, format } => self.start_export(source, format, ctx),
                Action::Reveal(p) => open_path(&p),
                Action::Refresh => self.refresh(),
                Action::Sync => self.start_sync(ctx),
            }
        }
    }

    // -----------------------------------------------------------------------
    // 介面

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let cmd = Modifiers::COMMAND;
        let (save, edit, split, read, sidebar, new_note, zoom_in, zoom_out) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::S)),
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Num1)),
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Num2)),
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Num3)),
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::B)),
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::N)),
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Equals))
                    || i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Plus)),
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Minus)),
            )
        });
        if save {
            self.save_current();
            self.info("已儲存");
        }
        if edit {
            self.settings.view = ViewMode::Edit;
        }
        if split {
            self.settings.view = ViewMode::Split;
        }
        if read {
            self.settings.view = ViewMode::Read;
        }
        if sidebar {
            self.settings.sidebar = !self.settings.sidebar;
        }
        if new_note {
            if let Some(dir) = self.current_dir() {
                self.dialog = Some(Dialog::NewNote { dir, name: String::new() });
            }
        }
        if zoom_in || zoom_out {
            let d = if zoom_in { 1.0 } else { -1.0 };
            if self.settings.view == ViewMode::Read {
                self.settings.reader_size = (self.settings.reader_size + d).clamp(10.0, 40.0);
            } else {
                self.settings.editor_size = (self.settings.editor_size + d).clamp(9.0, 40.0);
            }
        }

        // 檔案拖曳進視窗
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect()
        });
        if let Some(p) = dropped.into_iter().next() {
            if p.is_dir() {
                self.actions.push(Action::OpenLibrary(p));
            } else if shosai_core::is_markdown(&p) {
                if self.library.is_none() {
                    if let Some(parent) = p.parent() {
                        self.open_library(parent);
                    }
                }
                self.actions.push(Action::Open(p));
            }
        }

        // 回到視窗時檢查外部修改
        let focused = ctx.input(|i| i.raw.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(true))));
        if focused {
            self.reload_if_changed();
        }
    }

    /// 新增檔案時預設放的資料夾。
    fn current_dir(&self) -> Option<PathBuf> {
        self.doc
            .as_ref()
            .and_then(|d| d.path.parent().map(Path::to_path_buf))
            .or_else(|| self.library.as_ref().map(|l| l.root.clone()))
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        if self.applied_theme == Some(self.settings.dark) {
            return;
        }
        self.applied_theme = Some(self.settings.dark);
        ctx.set_theme(match self.settings.dark {
            None => egui::ThemePreference::System,
            Some(true) => egui::ThemePreference::Dark,
            Some(false) => egui::ThemePreference::Light,
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.selectable_label(self.settings.sidebar, "☰").on_hover_text("側欄 (Ctrl+B)").clicked() {
                self.settings.sidebar = !self.settings.sidebar;
            }
            ui.separator();
            ui.selectable_value(&mut self.settings.view, ViewMode::Edit, "✏ 編輯").on_hover_text("Ctrl+1");
            ui.selectable_value(&mut self.settings.view, ViewMode::Split, "◫ 分割").on_hover_text("Ctrl+2");
            ui.selectable_value(&mut self.settings.view, ViewMode::Read, "📖 閱讀").on_hover_text("Ctrl+3");
            ui.separator();

            let dirty = self.doc.as_ref().is_some_and(|d| d.dirty);
            if ui.add_enabled(dirty, egui::Button::new("💾 儲存")).on_hover_text("Ctrl+S（也會自動儲存）").clicked()
            {
                self.save_current();
            }

            let current = self.doc.as_ref().map(|d| d.path.clone());
            let book_root = current.as_ref().and_then(|p| self.book_of(p)).map(|b| b.root.clone());
            ui.add_enabled_ui(current.is_some() && self.export_job.is_none(), |ui| {
                ui.menu_button("📤 輸出", |ui| {
                    for format in [Format::Pdf, Format::Html, Format::Typst] {
                        if let Some(root) = &book_root {
                            if ui.button(format!("整本書 → {}", format.label())).clicked() {
                                self.actions.push(Action::Export { source: root.clone(), format });
                            }
                        }
                        if let Some(p) = &current {
                            if ui.button(format!("此檔案 → {}", format.label())).clicked() {
                                self.actions.push(Action::Export { source: p.clone(), format });
                            }
                        }
                        ui.separator();
                    }
                });
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("ⓘ").on_hover_text("關於").clicked() {
                    self.dialog = Some(Dialog::About);
                }
                let (icon, next, tip) = match self.settings.dark {
                    None => ("◐", Some(true), "主題：跟隨系統"),
                    Some(true) => ("🌙", Some(false), "主題：深色"),
                    Some(false) => ("☀", None, "主題：淺色"),
                };
                if ui.button(icon).on_hover_text(tip).clicked() {
                    self.settings.dark = next;
                }
                let reading = self.settings.view == ViewMode::Read;
                let size = if reading { &mut self.settings.reader_size } else { &mut self.settings.editor_size };
                if ui.button("A+").on_hover_text("放大字體 (Ctrl+=)").clicked() {
                    *size = (*size + 1.0).min(40.0);
                }
                ui.label(RichText::new(format!("{size:.0}")).weak());
                if ui.button("A−").on_hover_text("縮小字體 (Ctrl+-)").clicked() {
                    *size = (*size - 1.0).max(9.0);
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        if let Some(rx) = &self.export_job {
            if let Ok(result) = rx.try_recv() {
                self.export_job = None;
                self.status = Some(match result {
                    Ok(p) => Status::Exported(p),
                    Err(e) => Status::Error(e, Instant::now()),
                });
            }
        }

        ui.horizontal(|ui| {
            if let Some(doc) = self.doc.as_mut() {
                if doc.words_stale && doc.last_edit.elapsed() > Duration::from_millis(400) {
                    doc.words = markdown::word_count(&doc.text);
                    doc.words_stale = false;
                }
                let name = doc.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                ui.label(RichText::new(name).strong());
                ui.label(RichText::new(if doc.dirty { "● 未儲存" } else { "✔ 已儲存" }).weak());
                ui.label(RichText::new(format!("{} 字", doc.words)).weak());
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut clear = false;
                match &self.status {
                    Some(Status::Info(msg, t)) => {
                        ui.label(msg);
                        clear = t.elapsed() > Duration::from_secs(4);
                    }
                    Some(Status::Error(msg, t)) => {
                        ui.label(RichText::new(msg.lines().next().unwrap_or("")).color(ui.visuals().error_fg_color))
                            .on_hover_text(msg);
                        clear = t.elapsed() > Duration::from_secs(15);
                    }
                    Some(Status::Working(msg)) => {
                        ui.spinner();
                        ui.label(msg);
                    }
                    Some(Status::Exported(p)) => {
                        if ui.small_button("✕").clicked() {
                            clear = true;
                        }
                        if ui.small_button("開啟資料夾").clicked() {
                            self.actions.push(Action::Reveal(p.parent().unwrap_or(p).to_path_buf()));
                        }
                        if ui.small_button("開啟").clicked() {
                            self.actions.push(Action::Reveal(p.clone()));
                        }
                        ui.label(format!("已輸出 {}", p.file_name().unwrap_or_default().to_string_lossy()));
                    }
                    None => {}
                }
                if clear {
                    self.status = None;
                }
            });
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let Some(lib) = &self.library else {
            ui.add_space(8.0);
            if ui.button("📂 開啟資料夾…").clicked() {
                self.actions.push(Action::PickLibrary);
            }
            return;
        };
        let root = lib.root.clone();
        let root_name =
            root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());

        ui.horizontal(|ui| {
            ui.menu_button("📂", |ui| {
                if ui.button("開啟其他資料夾…").clicked() {
                    self.actions.push(Action::PickLibrary);
                }
                let recent: Vec<PathBuf> =
                    self.settings.recent_libraries.iter().filter(|r| **r != root && r.is_dir()).cloned().collect();
                if !recent.is_empty() {
                    ui.separator();
                    ui.label(RichText::new("最近使用").weak());
                    for r in recent {
                        if ui.button(r.display().to_string()).clicked() {
                            self.actions.push(Action::OpenLibrary(r));
                        }
                    }
                }
            })
            .response
            .on_hover_text("書庫");
            ui.label(RichText::new(&root_name).strong()).on_hover_text(root.display().to_string());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("＋", |ui| new_menu(ui, &root, &mut self.actions));
                if ui.button("⟳").on_hover_text("重新整理").clicked() {
                    self.actions.push(Action::Refresh);
                }
                #[cfg(feature = "nextcloud")]
                {
                    let busy = self.sync_job.is_some();
                    let resp =
                        ui.add_enabled(!busy, egui::Button::new("☁")).on_hover_text("與 Nextcloud 同步（右鍵：設定）");
                    if resp.clicked() {
                        self.actions.push(Action::Sync);
                    }
                    resp.context_menu(|ui| {
                        if ui.button("同步設定…").clicked() {
                            self.actions.push(Action::Dialog(nextcloud_dialog(&root)));
                            ui.close();
                        }
                    });
                }
            });
        });
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("🔍 搜尋檔名…").desired_width(f32::INFINITY));
        ui.separator();

        let current = self.doc.as_ref().map(|d| d.path.clone());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let lib = self.library.as_ref().unwrap();
            let mut tree = TreeCtx {
                current: current.as_deref(),
                books: &self.books,
                actions: &mut self.actions,
                lib_root: &root,
            };
            if !self.filter.trim().is_empty() {
                let q = self.filter.trim().to_lowercase();
                for e in lib.all_files() {
                    let rel = e.path.strip_prefix(&root).unwrap_or(&e.path).to_string_lossy().into_owned();
                    if rel.to_lowercase().contains(&q) {
                        file_row(ui, &e.path, &rel, &mut tree);
                    }
                }
            } else if let Some(book) = self.books.get(&root) {
                book_contents(ui, book, &mut tree);
            } else if lib.entries.is_empty() {
                ui.label(RichText::new("這個資料夾還沒有 Markdown 檔。\n用上方的「＋」新增筆記或書本。").weak());
            } else {
                tree_ui(ui, &lib.entries, &mut tree);
            }
        });
    }

    fn editor(&mut self, ui: &mut egui::Ui, centered: bool) {
        let size = self.settings.editor_size;
        let Some(doc) = self.doc.as_mut() else { return };
        egui::ScrollArea::vertical().id_salt(("editor", &doc.path)).auto_shrink([false, false]).show(ui, |ui| {
            let full = ui.available_width();
            let width = if centered { full.min(860.0) } else { full };
            ui.horizontal(|ui| {
                ui.add_space(((full - width) / 2.0).max(0.0));
                let edit = egui::TextEdit::multiline(&mut doc.text)
                    .font(FontId::monospace(size))
                    .desired_width(width - 8.0)
                    .desired_rows(30)
                    .lock_focus(true)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::symmetric(12, 10))
                    .hint_text("# 標題\n\n開始寫作……");
                let response = ui.add(edit);
                if response.changed() {
                    doc.dirty = true;
                    doc.words_stale = true;
                    doc.last_edit = Instant::now();
                }
            });
        });
    }

    fn preview(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = self.doc.as_ref() else { return };
        let base = doc.path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        egui::ScrollArea::vertical().id_salt(("preview", &doc.path)).auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 10)).show(ui, |ui| {
                CommonMarkViewer::new()
                    .default_implicit_uri_scheme(format!("file://{base}/"))
                    .max_image_width(Some(ui.available_width() as usize))
                    .show(ui, &mut self.md_cache, &doc.text);
            });
        });
    }

    fn reader(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = self.doc.as_ref() else { return };
        let path = doc.path.clone();
        let seq = self.reading_sequence(&path);
        let pos = seq.iter().position(|(p, _)| *p == path);
        let prev = pos.and_then(|i| i.checked_sub(1)).and_then(|i| seq.get(i)).cloned();
        let next = pos.and_then(|i| seq.get(i + 1)).cloned();
        let book_title = self.book_of(&path).map(|b| b.meta.title.clone());

        // 翻頁快捷鍵（沒有輸入框取得焦點時）
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        if !typing {
            let (left, right) = ui.input(|i| {
                (
                    i.key_pressed(Key::ArrowLeft) || (i.key_pressed(Key::PageUp) && i.modifiers.command),
                    i.key_pressed(Key::ArrowRight) || (i.key_pressed(Key::PageDown) && i.modifiers.command),
                )
            });
            if left {
                if let Some((p, _)) = &prev {
                    self.actions.push(Action::Open(p.clone()));
                }
            }
            if right {
                if let Some((p, _)) = &next {
                    self.actions.push(Action::Open(p.clone()));
                }
            }
        }

        let dark = ui.visuals().dark_mode;
        let (desk, paper, ink) = if dark {
            (Color32::from_rgb(24, 23, 21), Color32::from_rgb(36, 34, 31), Color32::from_rgb(221, 214, 200))
        } else {
            (Color32::from_rgb(232, 226, 214), Color32::from_rgb(251, 248, 241), Color32::from_rgb(43, 38, 32))
        };
        let size = self.settings.reader_size;
        let max_w = self.settings.reader_width;
        let base = path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();

        egui::Frame::new().fill(desk).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt(("reader", &path)).auto_shrink([false, false]).show(ui, |ui| {
                let full = ui.available_width();
                let page_w = full.min(max_w + 2.0 * 56.0).max(200.0);
                ui.add_space(24.0);
                ui.horizontal(|ui| {
                    ui.add_space(((full - page_w) / 2.0).max(0.0));
                    egui::Frame::new()
                        .fill(paper)
                        .corner_radius(4.0)
                        .shadow(egui::Shadow {
                            offset: [0, 4],
                            blur: 18,
                            spread: 0,
                            color: Color32::from_black_alpha(40),
                        })
                        .inner_margin(egui::Margin::symmetric(if page_w < 520.0 { 20 } else { 56 }, 44))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.set_width(page_w - if page_w < 520.0 { 40.0 } else { 112.0 });
                                let style = ui.style_mut();
                                style.visuals.override_text_color = Some(ink);
                                style.text_styles.insert(TextStyle::Body, FontId::proportional(size));
                                style.text_styles.insert(TextStyle::Monospace, FontId::monospace(size * 0.85));
                                style.text_styles.insert(TextStyle::Heading, FontId::proportional(size * 1.6));
                                style.text_styles.insert(TextStyle::Small, FontId::proportional(size * 0.75));
                                style.text_styles.insert(TextStyle::Button, FontId::proportional(size * 0.85));
                                style.spacing.item_spacing.y = size * 0.55;

                                if let Some(t) = &book_title {
                                    ui.vertical_centered(|ui| {
                                        ui.label(RichText::new(t).size(size * 0.75).weak());
                                    });
                                    ui.add_space(size);
                                }
                                CommonMarkViewer::new()
                                    .default_implicit_uri_scheme(format!("file://{base}/"))
                                    .max_image_width(Some(ui.available_width() as usize))
                                    .show(ui, &mut self.md_cache, &doc.text);

                                ui.add_space(size * 2.0);
                                ui.separator();
                                ui.horizontal(|ui| {
                                    if let Some((p, t)) = &prev {
                                        if ui.button(format!("‹ {t}")).on_hover_text("← 上一章").clicked() {
                                            self.actions.push(Action::Open(p.clone()));
                                        }
                                    }
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if let Some((p, t)) = &next {
                                            if ui.button(format!("{t} ›")).on_hover_text("下一章 →").clicked() {
                                                self.actions.push(Action::Open(p.clone()));
                                            }
                                        }
                                        if let Some(i) = pos {
                                            if seq.len() > 1 {
                                                ui.centered_and_justified(|ui| {
                                                    ui.label(
                                                        RichText::new(format!("{} / {}", i + 1, seq.len())).weak(),
                                                    );
                                                });
                                            }
                                        }
                                    });
                                });
                            })
                        });
                });
                ui.add_space(32.0);
            });
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.22);
            ui.label(RichText::new("書齋").size(44.0).strong());
            ui.label(RichText::new("輕量的 Markdown 寫作與閱讀器").size(16.0).weak());
            ui.add_space(28.0);
            if self.library.is_none() {
                if ui.button(RichText::new("📂 開啟資料夾…").size(16.0)).clicked() {
                    self.actions.push(Action::PickLibrary);
                }
                ui.add_space(6.0);
                ui.label(RichText::new("也可以把資料夾或 .md 檔拖曳進視窗").weak());
                let recent: Vec<PathBuf> =
                    self.settings.recent_libraries.iter().filter(|r| r.is_dir()).cloned().collect();
                if !recent.is_empty() {
                    ui.add_space(18.0);
                    ui.label(RichText::new("最近使用").weak());
                    for r in recent {
                        if ui.link(r.display().to_string()).clicked() {
                            self.actions.push(Action::OpenLibrary(r));
                        }
                    }
                }
            } else {
                ui.label(RichText::new("從左側選一個檔案開始，或用「＋」新增筆記、書本").weak());
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Ctrl+N 新增筆記　Ctrl+S 儲存　Ctrl+1/2/3 切換模式　Ctrl+B 側欄").weak().small(),
                );
            }
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let mut keep = true;
        let mut confirm = false;

        let modal = egui::Modal::new(egui::Id::unique("shosai-dialog")).show(ctx, |ui| {
            ui.set_min_width(340.0);
            let enter = ui.input(|i| i.key_pressed(Key::Enter));
            match &mut dialog {
                Dialog::NewNote { name, .. } => {
                    ui.heading("新增筆記");
                    focused_input(ui, name, "檔名");
                    confirm |= enter;
                }
                Dialog::NewFolder { name, .. } => {
                    ui.heading("新增資料夾");
                    focused_input(ui, name, "資料夾名稱");
                    confirm |= enter;
                }
                Dialog::NewBook { title, author, .. } => {
                    ui.heading("建立新書");
                    ui.label("會建立一個資料夾，裡面每個 Markdown 檔就是一章。");
                    focused_input(ui, title, "書名");
                    ui.add(egui::TextEdit::singleline(author).hint_text("作者（可留空）").desired_width(f32::INFINITY));
                }
                Dialog::NewChapter { title, .. } => {
                    ui.heading("新增章節");
                    focused_input(ui, title, "章節標題");
                    confirm |= enter;
                }
                Dialog::Rename { name, .. } => {
                    ui.heading("重新命名");
                    focused_input(ui, name, "新名稱");
                    confirm |= enter;
                }
                Dialog::Trash { path } => {
                    ui.heading("移到回收桶");
                    ui.label(format!("確定要移除「{}」？", path.file_name().unwrap_or_default().to_string_lossy()));
                    ui.label(RichText::new(format!("檔案會移到書庫的 {} 資料夾，可以手動救回。", library::TRASH_DIR)).weak());
                }
                Dialog::BookSettings { meta, .. } => book_settings_ui(ui, meta),
                Dialog::Nextcloud { server, user, password, remote_dir, insecure } => {
                    ui.heading("Nextcloud 同步設定");
                    egui::Grid::new("nc").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        ui.label("伺服器");
                        ui.add(egui::TextEdit::singleline(server).hint_text("https://cloud.example.com"));
                        ui.end_row();
                        ui.label("帳號");
                        ui.text_edit_singleline(user);
                        ui.end_row();
                        ui.label("應用程式密碼");
                        ui.add(egui::TextEdit::singleline(password).password(true));
                        ui.end_row();
                        ui.label("雲端資料夾");
                        ui.text_edit_singleline(remote_dir);
                        ui.end_row();
                    });
                    ui.checkbox(insecure, "略過 TLS 憑證檢查（自簽憑證的 homelab）");
                    ui.label(
                        RichText::new("密碼會以明文存在書庫的 .shosai/nextcloud.toml（此資料夾不會被同步）。\n請在 Nextcloud「設定 → 安全性」建立專用的應用程式密碼。")
                            .weak()
                            .small(),
                    );
                }
                Dialog::Report(report) => {
                    ui.heading("同步完成");
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        ui.label(RichText::new(report.as_str()).monospace());
                    });
                }
                Dialog::About => {
                    ui.heading("書齋 Shosai");
                    ui.label(format!("版本 {}", env!("CARGO_PKG_VERSION")));
                    ui.label("輕量的 Markdown 寫作與閱讀器，可將資料夾整理成書並輸出 PDF。");
                    ui.add_space(6.0);
                    ui.label(RichText::new("PDF 排版引擎：Typst　介面：egui").weak());
                }
            }
            ui.add_space(8.0);
            ui.separator();
            ui.horizontal(|ui| {
                let label = match &dialog {
                    Dialog::Trash { .. } => "移除",
                    Dialog::About | Dialog::Report(_) => "關閉",
                    Dialog::BookSettings { .. } => "儲存",
                    _ => "確定",
                };
                if ui.button(label).clicked() {
                    confirm = true;
                }
                if !matches!(dialog, Dialog::About | Dialog::Report(_)) && ui.button("取消").clicked() {
                    keep = false;
                }
            });
        });
        if modal.should_close() {
            keep = false;
        }

        if confirm {
            if let Err(e) = self.apply_dialog(dialog) {
                self.error(e);
            }
        } else if keep {
            self.dialog = Some(dialog);
        }
    }

    fn apply_dialog(&mut self, dialog: Dialog) -> Result<(), String> {
        let lib_root = self.library.as_ref().map(|l| l.root.clone());
        match dialog {
            Dialog::NewNote { dir, name } => {
                let name = if name.trim().is_empty() { "未命名".to_string() } else { name };
                let path = library::create_note(&dir, &name).map_err(|e| e.to_string())?;
                self.refresh();
                self.open_file(&path);
                self.settings.view = ViewMode::Edit;
            }
            Dialog::NewFolder { dir, name } => {
                library::create_dir(&dir, if name.trim().is_empty() { "新資料夾" } else { &name })
                    .map_err(|e| e.to_string())?;
                self.refresh();
            }
            Dialog::NewBook { dir, title, author } => {
                let title =
                    if title.trim().is_empty() { "未命名的書".to_string() } else { title.trim().to_string() };
                let target = library::create_dir(&dir, &title).map_err(|e| e.to_string())?;
                let book = Book::create(&target, &title, author.trim()).map_err(|e| e.to_string())?;
                let first = book.chapters.first().map(|c| c.path.clone());
                self.refresh();
                if let Some(first) = first {
                    self.open_file(&first);
                }
            }
            Dialog::NewChapter { book, title } => {
                let title = if title.trim().is_empty() { "新章節".to_string() } else { title.trim().to_string() };
                let b = self.books.get_mut(&book).ok_or("找不到書本")?;
                let path = b.add_chapter(&title).map_err(|e| e.to_string())?;
                self.refresh();
                self.open_file(&path);
                self.settings.view = ViewMode::Edit;
            }
            Dialog::Rename { path, name } => {
                if name.trim().is_empty() {
                    return Ok(());
                }
                let was_open = self.doc.as_ref().map(|d| d.path.starts_with(&path)).unwrap_or(false);
                self.save_current();
                let new = library::rename(&path, &name).map_err(|e| e.to_string())?;
                if was_open {
                    if let Some(doc) = self.doc.as_mut() {
                        let rest = doc.path.strip_prefix(&path).map(Path::to_path_buf).unwrap_or_default();
                        doc.path = if rest.as_os_str().is_empty() { new.clone() } else { new.join(rest) };
                        self.settings.last_file = Some(doc.path.clone());
                    }
                }
                if lib_root.as_deref() == Some(path.as_path()) {
                    self.open_library(&new);
                } else {
                    self.refresh();
                }
            }
            Dialog::Trash { path } => {
                let root = lib_root.ok_or("沒有開啟書庫")?;
                if self.doc.as_ref().is_some_and(|d| d.path.starts_with(&path)) {
                    self.doc = None;
                }
                library::trash(&root, &path).map_err(|e| e.to_string())?;
                self.refresh();
                self.info("已移到回收桶");
            }
            Dialog::BookSettings { root, meta } => {
                let mut book = Book::open(&root).map_err(|e| e.to_string())?;
                book.meta = *meta;
                book.save().map_err(|e| e.to_string())?;
                self.refresh();
                self.info("書本設定已儲存");
            }
            #[cfg(feature = "nextcloud")]
            Dialog::Nextcloud { server, user, password, remote_dir, insecure } => {
                let root = lib_root.ok_or("沒有開啟書庫")?;
                let cfg = shosai_core::sync::SyncConfig {
                    server: server.trim().to_string(),
                    user: user.trim().to_string(),
                    app_password: password,
                    remote_dir: remote_dir.trim().trim_matches('/').to_string(),
                    insecure,
                };
                cfg.save(&root).map_err(|e| e.to_string())?;
                self.info("同步設定已儲存，按 ☁ 開始同步");
            }
            #[cfg(not(feature = "nextcloud"))]
            Dialog::Nextcloud { .. } => {}
            Dialog::Report(_) | Dialog::About => {}
        }
        Ok(())
    }
}

impl eframe::App for ShosaiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        self.shortcuts(&ctx);

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(3.0);
            self.toolbar(ui);
            ui.add_space(1.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));

        let mut sidebar_open = self.settings.sidebar;
        egui::Panel::left("sidebar").resizable(true).default_size(250.0).size_range(160.0..=480.0).show_collapsible(
            ui,
            &mut sidebar_open,
            |ui| {
                ui.add_space(4.0);
                self.sidebar(ui);
            },
        );
        self.settings.sidebar = sidebar_open;

        let mode = self.settings.view;
        let paper = egui::Frame::new().fill(ui.visuals().panel_fill);
        if self.doc.is_none() {
            egui::CentralPanel::default_margins().show(ui, |ui| self.welcome(ui));
        } else {
            match mode {
                ViewMode::Edit => {
                    egui::CentralPanel::no_frame().frame(paper).show(ui, |ui| self.editor(ui, true));
                }
                ViewMode::Split => {
                    let half = ui.available_width() / 2.0;
                    egui::Panel::right("preview")
                        .frame(paper)
                        .resizable(true)
                        .default_size(half)
                        .size_range(200.0..=f32::INFINITY)
                        .show(ui, |ui| self.preview(ui));
                    egui::CentralPanel::no_frame().frame(paper).show(ui, |ui| self.editor(ui, false));
                }
                ViewMode::Read => {
                    egui::CentralPanel::no_frame().show(ui, |ui| self.reader(ui));
                }
            }
        }

        self.dialogs(&ctx);
        self.process_actions(&ctx);
        self.poll_sync();

        // 自動儲存
        if let Some(doc) = &self.doc {
            if doc.dirty {
                if doc.last_edit.elapsed() >= AUTOSAVE_AFTER {
                    self.save_current();
                } else {
                    ctx.request_repaint_after(AUTOSAVE_AFTER);
                }
            }
        }
        if matches!(self.status, Some(Status::Info(..) | Status::Error(..))) {
            ctx.request_repaint_after(Duration::from_secs(1));
        }

        // 量測啟動速度：SHOSAI_STARTUP_TRACE=1 shosai
        if self.first_frame {
            self.first_frame = false;
            if std::env::var_os("SHOSAI_STARTUP_TRACE").is_some() {
                if let Some(t) = crate::STARTED.get() {
                    eprintln!("first frame: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        // 視窗標題
        let title = match &self.doc {
            Some(d) => format!(
                "{}{} — 書齋",
                if d.dirty { "● " } else { "" },
                d.path.file_name().unwrap_or_default().to_string_lossy()
            ),
            None => "書齋".into(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_current();
    }
}

// ---------------------------------------------------------------------------
// 側欄樹狀清單

struct TreeCtx<'a> {
    current: Option<&'a Path>,
    books: &'a HashMap<PathBuf, Book>,
    actions: &'a mut Vec<Action>,
    lib_root: &'a Path,
}

fn tree_ui(ui: &mut egui::Ui, entries: &[Entry], ctx: &mut TreeCtx) {
    for e in entries {
        match e.kind {
            EntryKind::Markdown => {
                let label = e.name.trim_end_matches(".md").to_string();
                file_row(ui, &e.path, &label, ctx);
            }
            EntryKind::Dir => {
                let open = ctx.current.is_some_and(|c| c.starts_with(&e.path));
                let resp = egui::CollapsingHeader::new(format!("📁 {}", e.name))
                    .id_salt(&e.path)
                    .default_open(open)
                    .show(ui, |ui| {
                        if e.children.is_empty() {
                            ui.label(RichText::new("（空）").weak());
                        }
                        tree_ui(ui, &e.children, ctx);
                    });
                resp.header_response.context_menu(|ui| {
                    new_menu(ui, &e.path, ctx.actions);
                    ui.separator();
                    common_menu(ui, &e.path, ctx.actions);
                });
            }
            EntryKind::Book => {
                let Some(book) = ctx.books.get(&e.path) else { continue };
                let open = ctx.current.is_some_and(|c| c.starts_with(&e.path));
                let resp = egui::CollapsingHeader::new(RichText::new(format!("📖 {}", book.meta.title)).strong())
                    .id_salt(&e.path)
                    .default_open(open)
                    .show(ui, |ui| book_contents(ui, book, ctx));
                resp.header_response.context_menu(|ui| {
                    book_menu(ui, book, ctx.actions);
                    ui.separator();
                    common_menu(ui, &e.path, ctx.actions);
                });
            }
        }
    }
}

fn book_contents(ui: &mut egui::Ui, book: &Book, ctx: &mut TreeCtx) {
    let n = book.chapters.len();
    for (i, ch) in book.chapters.iter().enumerate() {
        let selected = ctx.current == Some(ch.path.as_path());
        let resp = ui.selectable_label(selected, format!("{}. {}", i + 1, ch.title)).on_hover_text(&ch.rel);
        if resp.clicked() && !selected {
            ctx.actions.push(Action::Open(ch.path.clone()));
        }
        resp.context_menu(|ui| {
            if ui.add_enabled(i > 0, egui::Button::new("⬆ 上移")).clicked() {
                ctx.actions.push(Action::MoveChapter { book: book.root.clone(), index: i, delta: -1 });
                ui.close();
            }
            if ui.add_enabled(i + 1 < n, egui::Button::new("⬇ 下移")).clicked() {
                ctx.actions.push(Action::MoveChapter { book: book.root.clone(), index: i, delta: 1 });
                ui.close();
            }
            ui.separator();
            common_menu(ui, &ch.path, ctx.actions);
        });
    }
    if book.root == ctx.lib_root {
        ui.separator();
        book_menu(ui, book, ctx.actions);
    } else if ui.small_button("＋ 新增章節").clicked() {
        ctx.actions.push(Action::Dialog(Dialog::NewChapter { book: book.root.clone(), title: String::new() }));
    }
}

fn file_row(ui: &mut egui::Ui, path: &Path, label: &str, ctx: &mut TreeCtx) {
    let selected = ctx.current == Some(path);
    let resp = ui.selectable_label(selected, format!("📄 {label}"));
    if resp.clicked() && !selected {
        ctx.actions.push(Action::Open(path.to_path_buf()));
    }
    resp.context_menu(|ui| {
        if ui.button("輸出 PDF…").clicked() {
            ctx.actions.push(Action::Export { source: path.to_path_buf(), format: Format::Pdf });
            ui.close();
        }
        ui.separator();
        common_menu(ui, path, ctx.actions);
    });
}

fn new_menu(ui: &mut egui::Ui, dir: &Path, actions: &mut Vec<Action>) {
    if ui.button("📄 新增筆記").clicked() {
        actions.push(Action::Dialog(Dialog::NewNote { dir: dir.to_path_buf(), name: String::new() }));
        ui.close();
    }
    if ui.button("📁 新增資料夾").clicked() {
        actions.push(Action::Dialog(Dialog::NewFolder { dir: dir.to_path_buf(), name: String::new() }));
        ui.close();
    }
    if ui.button("📖 建立新書").clicked() {
        actions.push(Action::Dialog(Dialog::NewBook {
            dir: dir.to_path_buf(),
            title: String::new(),
            author: String::new(),
        }));
        ui.close();
    }
}

fn book_menu(ui: &mut egui::Ui, book: &Book, actions: &mut Vec<Action>) {
    if ui.button("＋ 新增章節").clicked() {
        actions.push(Action::Dialog(Dialog::NewChapter { book: book.root.clone(), title: String::new() }));
        ui.close();
    }
    if ui.button("⚙ 書本設定").clicked() {
        actions
            .push(Action::Dialog(Dialog::BookSettings { root: book.root.clone(), meta: Box::new(book.meta.clone()) }));
        ui.close();
    }
    if ui.button("📤 輸出 PDF…").clicked() {
        actions.push(Action::Export { source: book.root.clone(), format: Format::Pdf });
        ui.close();
    }
    if ui.button("📤 輸出 HTML…").clicked() {
        actions.push(Action::Export { source: book.root.clone(), format: Format::Html });
        ui.close();
    }
}

fn common_menu(ui: &mut egui::Ui, path: &Path, actions: &mut Vec<Action>) {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if ui.button("✏ 重新命名").clicked() {
        actions.push(Action::Dialog(Dialog::Rename { path: path.to_path_buf(), name }));
        ui.close();
    }
    if ui.button("📂 在檔案管理員中顯示").clicked() {
        let target = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        actions.push(Action::Reveal(target.to_path_buf()));
        ui.close();
    }
    if ui.button("🗑 移到回收桶").clicked() {
        actions.push(Action::Dialog(Dialog::Trash { path: path.to_path_buf() }));
        ui.close();
    }
}

#[cfg(feature = "nextcloud")]
fn nextcloud_dialog(root: &Path) -> Dialog {
    match shosai_core::sync::SyncConfig::load(root) {
        Ok(c) => Dialog::Nextcloud {
            server: c.server,
            user: c.user,
            password: c.app_password,
            remote_dir: c.remote_dir,
            insecure: c.insecure,
        },
        Err(_) => Dialog::Nextcloud {
            server: String::new(),
            user: String::new(),
            password: String::new(),
            remote_dir: "Shosai".into(),
            insecure: false,
        },
    }
}

fn focused_input(ui: &mut egui::Ui, text: &mut String, hint: &str) {
    let resp = ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(f32::INFINITY));
    if !resp.has_focus() && ui.memory(|m| m.focused().is_none()) {
        resp.request_focus();
    }
}

fn book_settings_ui(ui: &mut egui::Ui, meta: &mut BookMeta) {
    ui.heading("書本設定");
    egui::Grid::new("book-settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("書名");
        ui.text_edit_singleline(&mut meta.title);
        ui.end_row();
        ui.label("副標題");
        ui.text_edit_singleline(&mut meta.subtitle);
        ui.end_row();
        ui.label("作者");
        ui.text_edit_singleline(&mut meta.author);
        ui.end_row();
        ui.label("語言");
        ui.text_edit_singleline(&mut meta.language);
        ui.end_row();

        let l = &mut meta.layout;
        ui.label("紙張");
        egui::ComboBox::from_id_salt("paper").selected_text(l.paper.to_uppercase()).show_ui(ui, |ui| {
            for p in ["a4", "a5", "b5", "a6", "letter"] {
                ui.selectable_value(&mut l.paper, p.to_string(), p.to_uppercase());
            }
        });
        ui.end_row();
        ui.label("字級");
        ui.add(egui::DragValue::new(&mut l.font_size).range(7.0..=20.0).speed(0.1).suffix(" pt"));
        ui.end_row();
        ui.label("行距");
        ui.add(egui::DragValue::new(&mut l.line_height).range(1.0..=3.0).speed(0.05).suffix(" 倍"));
        ui.end_row();
        ui.label("目錄深度");
        ui.add(egui::DragValue::new(&mut l.toc_depth).range(1..=4));
        ui.end_row();
    });
    let l = &mut meta.layout;
    ui.checkbox(&mut l.indent, "段落首行縮排兩字");
    ui.checkbox(&mut l.numbered, "章節編號（第一章、1.1…）");
    ui.checkbox(&mut l.cover, "產生封面");
    ui.checkbox(&mut l.toc, "產生目錄");
}

// ---------------------------------------------------------------------------

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// 用系統預設程式開啟檔案或資料夾。
fn open_path(path: &Path) {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("explorer");
        c.arg(path);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(path);
        c
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path);
        c
    };
    let _ = cmd.spawn();
}
