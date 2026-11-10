//! 視窗介面（egui）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32, FontId, Key, KeyboardShortcut, Modifiers, RichText, TextStyle};
use serde::{Deserialize, Serialize};
use shosai_core::book::{is_book, BookMeta};
use shosai_core::library::{CustomOrder, SortKey, SortOptions};
use shosai_core::novel::NovelBook;
use shosai_core::{library, markdown, Book, Entry, EntryKind, Library};

use crate::icons::{self, Icon};
use crate::md_view::{MdView, ViewEvent};
use crate::novel_view::{NovelEvent, NovelPrefs, NovelView};
use crate::pdf_view::{Fit, PdfView};
use crate::theme::{self, Palette};
use crate::tree::{self, DragItem, Place, TreeCtx, TreeNav};

pub fn run(open: Option<PathBuf>) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("書齋")
            .with_app_id("shosai")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([520.0, 360.0])
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native("shosai", options, Box::new(move |cc| Ok(Box::new(ShosaiApp::new(cc, open)))))
}

/// 視窗 / 工作列圖示。
fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/shosai.png")).unwrap_or_default()
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
    /// 側欄中展開的資料夾 / 書本
    expanded: HashSet<PathBuf>,
    /// 介面縮放（1.0 = 100%）
    ui_scale: f32,
    /// 編輯器使用一般字型（否則等寬）
    editor_proportional: bool,
    /// 停止輸入幾秒後自動儲存
    autosave_secs: f32,
    show_word_count: bool,
    /// 側欄
    show_pdfs: bool,
    show_counts: bool,
    /// PDF
    pdf_fit: Fit,
    pdf_thumbs: bool,
    /// 每個 PDF 上次讀到的頁（從 0 起算）
    pdf_pages: HashMap<PathBuf, usize>,
    /// 檔案樹排序
    sort: SortOptions,
    /// 書本模式
    novel: NovelPrefs,
    /// 每本書讀到哪一章的第幾個字
    novel_pos: HashMap<PathBuf, (PathBuf, usize)>,
    /// 上次關閉時正在用書本模式讀的資料夾
    novel_open: Option<PathBuf>,
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
            expanded: HashSet::new(),
            ui_scale: 1.0,
            editor_proportional: false,
            autosave_secs: 1.2,
            show_word_count: true,
            show_pdfs: true,
            show_counts: true,
            pdf_fit: Fit::Width,
            pdf_thumbs: true,
            pdf_pages: HashMap::new(),
            sort: SortOptions::default(),
            novel: NovelPrefs::default(),
            novel_pos: HashMap::new(),
            novel_open: None,
        }
    }
}

struct OpenDoc {
    path: PathBuf,
    text: String,
    dirty: bool,
    last_edit: Instant,
    mtime: Option<SystemTime>,
    /// 磁碟上那一版內容的雜湊（開啟或上次儲存時），用來分辨外部是否真的改了內容
    saved_hash: u64,
    words: usize,
    words_stale: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Format {
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

pub(crate) enum Dialog {
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
    Shortcuts,
}

pub(crate) enum Action {
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
    /// 拖放檔案樹中的項目
    Drop {
        src: PathBuf,
        target: PathBuf,
        place: Place,
    },
    /// 以書本模式閱讀資料夾（可指定從哪一章開始）
    OpenNovel {
        root: PathBuf,
        chapter: Option<PathBuf>,
    },
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
    ctx: egui::Context,
    settings: Settings,
    library: Option<Library>,
    books: HashMap<PathBuf, Book>,
    doc: Option<OpenDoc>,
    pdf: Option<PdfView>,
    md_view: MdView,
    view_events: Vec<ViewEvent>,
    dialog: Option<Dialog>,
    actions: Vec<Action>,
    status: Option<Status>,
    export_job: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    sync_job: Option<mpsc::Receiver<Result<String, String>>>,
    filter: String,
    first_frame: bool,
    applied_theme: Option<Option<bool>>,
    applied_scale: f32,
    /// Ctrl+滾輪累積的縮放量（對數），滿一格才調整一級字級
    wheel_zoom: f32,
    show_settings: bool,
    settings_tab: SettingsTab,
    /// 拖動介面縮放拉條時的暫定值（放開才套用，避免縮放改變拉條位置造成失控）
    scale_draft: Option<f32>,
    /// 目前視窗標題（避免每幀重送）
    window_title: String,
    /// SHOSAI_FRAME_STATS：每秒印出重畫次數（檢查閒置時是否真的不重畫）
    frame_stats: Option<(Instant, u32)>,
    /// 自訂排序（`.shosai/order.toml`）
    order: CustomOrder,
    /// 監看書庫資料夾，有檔案新增 / 刪除 / 修改時自動更新檔案樹
    watcher: Option<FsWatcher>,
    /// 書本模式
    novel: Option<NovelView>,
    /// 檔案樹的鍵盤操作
    tree_nav: TreeNav,
    novel_titles: shosai_core::novel::TitleCache,
    /// 下一幀讓側欄的搜尋框取得焦點（Ctrl+P）
    focus_filter: bool,
    /// 開啟另一個檔案後要捲到的標題（`其他檔.md#標題` 這種連結）
    pending_anchor: Option<String>,
}

/// 檔案監看：背景執行緒收到變動後記下時間，畫面在停止變動一小段時間後才重新整理
/// （同步軟體一次寫入很多檔案時只會更新一次）。
struct FsWatcher {
    _watcher: notify::RecommendedWatcher,
    changed: Changed,
}

/// 最後一次變動的時間，以及這段期間變動過的路徑。
type Changed = Arc<Mutex<Option<(Instant, HashSet<PathBuf>)>>>;

const FS_DEBOUNCE: Duration = Duration::from_millis(300);

impl FsWatcher {
    fn start(root: &Path, ctx: &egui::Context) -> Option<FsWatcher> {
        use notify::Watcher;
        let changed: Changed = Arc::default();
        let (c, ctx, root_buf) = (changed.clone(), ctx.clone(), root.to_path_buf());
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                return;
            }
            let paths: Vec<PathBuf> = ev.paths.into_iter().filter(|p| fs_relevant(&root_buf, p)).collect();
            if paths.is_empty() {
                return;
            }
            let mut guard = c.lock().unwrap();
            let entry = guard.get_or_insert_with(|| (Instant::now(), HashSet::new()));
            entry.0 = Instant::now();
            entry.1.extend(paths);
            ctx.request_repaint_after(FS_DEBOUNCE + Duration::from_millis(20));
        })
        .ok()?;
        watcher.watch(root, notify::RecursiveMode::Recursive).ok()?;
        Some(FsWatcher { _watcher: watcher, changed })
    }

    /// 停止變動夠久了 → 取出變動的路徑；還在變動 → 回傳 None 並安排稍後再檢查。
    fn take(&self, ctx: &egui::Context) -> Option<HashSet<PathBuf>> {
        let mut guard = self.changed.lock().unwrap();
        let (since, _) = guard.as_ref()?;
        let elapsed = since.elapsed();
        if elapsed < FS_DEBOUNCE {
            ctx.request_repaint_after(FS_DEBOUNCE - elapsed);
            return None;
        }
        guard.take().map(|(_, p)| p)
    }
}

/// 只有會影響檔案樹或書本模式的變動才需要處理（略過隱藏資料夾、回收桶、暫存檔）。
fn fs_relevant(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else { return false };
    let rel_s = rel.to_string_lossy().replace('\\', "/");
    if rel_s == library::ORDER_FILE {
        return true;
    }
    if rel.components().any(|c| {
        let c = c.as_os_str().to_string_lossy();
        c.starts_with('.') || library::IGNORED_DIRS.contains(&c.as_ref())
    }) {
        return false;
    }
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()) {
        // 沒有副檔名：多半是資料夾
        None => true,
        Some(ext) => matches!(
            ext.as_str(),
            "md" | "markdown" | "mdown" | "mkd" | "pdf" | "toml" | "png" | "jpg" | "jpeg" | "webp" | "gif"
        ),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    Appearance,
    Editor,
    Reader,
    Pdf,
    Sidebar,
    Library,
    About,
}

impl SettingsTab {
    const ALL: [SettingsTab; 7] = [
        SettingsTab::Appearance,
        SettingsTab::Editor,
        SettingsTab::Reader,
        SettingsTab::Pdf,
        SettingsTab::Sidebar,
        SettingsTab::Library,
        SettingsTab::About,
    ];
    fn label(self) -> &'static str {
        match self {
            SettingsTab::Appearance => "外觀",
            SettingsTab::Editor => "編輯器",
            SettingsTab::Reader => "閱讀",
            SettingsTab::Pdf => "PDF",
            SettingsTab::Sidebar => "側欄",
            SettingsTab::Library => "書庫與同步",
            SettingsTab::About => "關於",
        }
    }
}

impl ShosaiApp {
    fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        theme::install(&cc.egui_ctx);
        let settings: Settings = cc.storage.and_then(|s| eframe::get_value(s, eframe::APP_KEY)).unwrap_or_default();

        let mut app = ShosaiApp {
            ctx: cc.egui_ctx.clone(),
            settings,
            library: None,
            books: HashMap::new(),
            doc: None,
            pdf: None,
            md_view: MdView::default(),
            view_events: Vec::new(),
            dialog: None,
            actions: Vec::new(),
            status: None,
            export_job: None,
            sync_job: None,
            filter: String::new(),
            first_frame: true,
            applied_theme: None,
            applied_scale: 0.0,
            wheel_zoom: 0.0,
            tree_nav: TreeNav::default(),
            novel_titles: Default::default(),
            focus_filter: false,
            pending_anchor: None,
            show_settings: false,
            settings_tab: SettingsTab::Appearance,
            scale_draft: None,
            window_title: String::new(),
            frame_stats: std::env::var_os("SHOSAI_FRAME_STATS").map(|_| (Instant::now(), 0)),
            order: CustomOrder::default(),
            watcher: None,
            novel: None,
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
                    if let Some(novel) = app.settings.novel_open.clone().filter(|d| d.is_dir() && d.starts_with(&root))
                    {
                        app.open_novel(&novel, None);
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
                self.order = CustomOrder::load(root);
                self.library = Some(lib);
                self.novel = None;
                self.watcher = FsWatcher::start(root, &self.ctx);
                self.resort();
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
        self.resort();
        self.reload_books();
        self.refresh_novel();
    }

    /// 依目前的排序設定重排檔案樹。
    fn resort(&mut self) {
        if let Some(lib) = self.library.as_mut() {
            lib.sort(&self.settings.sort, &self.order);
        }
    }

    /// 檔案監看回報變動。
    fn on_fs_change(&mut self, paths: HashSet<PathBuf>) {
        // 只是自己剛存檔的那個檔案 → 不用整個重新整理
        if let Some(doc) = &self.doc {
            if paths.iter().all(|p| *p == doc.path) && mtime(&doc.path) == doc.mtime {
                return;
            }
        }
        if paths.iter().any(|p| p.ends_with(library::ORDER_FILE)) {
            if let Some(root) = self.library.as_ref().map(|l| l.root.clone()) {
                self.order = CustomOrder::load(&root);
            }
        }
        self.reload_if_changed();
        self.refresh();
    }

    // -----------------------------------------------------------------------
    // 書本模式

    /// 以書本模式開啟資料夾。`chapter` 指定從哪一章開始（否則回到上次讀的位置）。
    fn open_novel(&mut self, root: &Path, chapter: Option<&Path>) {
        let Some(book) = self.novel_book(root) else {
            self.error("找不到這個資料夾");
            return;
        };
        if book.chapters.is_empty() {
            self.info("這個資料夾裡沒有 Markdown 檔");
            return;
        }
        self.save_current();
        let saved = self.settings.novel_pos.get(root).cloned();
        let at = match chapter {
            Some(c) => match saved {
                // 指定的章就是上次讀的那章 → 回到上次的位置
                Some((p, off)) if p == c => Some((p, off)),
                _ => Some((c.to_path_buf(), 0)),
            },
            None => saved,
        };
        self.novel = Some(NovelView::open(book, at));
        self.settings.novel_open = Some(root.to_path_buf());
    }

    fn novel_book(&mut self, root: &Path) -> Option<NovelBook> {
        let lib = self.library.as_ref()?;
        let children = lib.children_of(root)?;
        Some(NovelBook::with_cache(root, children, self.books.get(root), &mut self.novel_titles))
    }

    fn refresh_novel(&mut self) {
        let Some(root) = self.novel.as_ref().map(|n| n.book.root.clone()) else { return };
        match self.novel_book(&root) {
            Some(book) => {
                if let Some(n) = self.novel.as_mut() {
                    if n.book != book {
                        n.update_book(book);
                    } else {
                        n.files_changed();
                    }
                }
            }
            None => self.close_novel(false),
        }
    }

    /// 結束書本模式；`open_chapter` 時把正在讀的那一章開進一般的閱讀畫面。
    fn close_novel(&mut self, open_chapter: bool) {
        let Some(n) = self.novel.take() else { return };
        if let Some(pos) = n.position() {
            self.settings.novel_pos.insert(n.book.root.clone(), pos);
        }
        self.settings.novel_open = None;
        if open_chapter {
            if let Some(ch) = n.current_chapter() {
                if self.doc.as_ref().is_none_or(|d| d.path != ch) {
                    let ch = ch.to_path_buf();
                    self.open_file(&ch);
                }
            }
            if self.settings.view == ViewMode::Edit {
                self.settings.view = ViewMode::Read;
            }
        }
    }

    // -----------------------------------------------------------------------
    // 拖放與排序

    /// 檔案或資料夾被移動 / 改名後，更新所有記著舊路徑的地方。
    fn relocate(&mut self, old: &Path, new: &Path) {
        let moved = |p: &Path| -> Option<PathBuf> {
            let rest = p.strip_prefix(old).ok()?;
            Some(if rest.as_os_str().is_empty() { new.to_path_buf() } else { new.join(rest) })
        };
        if let Some(doc) = self.doc.as_mut() {
            if let Some(p) = moved(&doc.path) {
                doc.path = p.clone();
                self.settings.last_file = Some(p);
            }
        }
        if self.pdf.as_ref().is_some_and(|p| p.path.starts_with(old)) {
            self.pdf = None;
        }
        if self.novel.as_ref().is_some_and(|n| n.book.root.starts_with(old)) {
            self.close_novel(false);
        }
        let expanded: Vec<PathBuf> = self.settings.expanded.iter().filter(|p| p.starts_with(old)).cloned().collect();
        for p in expanded {
            self.settings.expanded.remove(&p);
            if let Some(n) = moved(&p) {
                self.settings.expanded.insert(n);
            }
        }
        if let Some(root) = self.library.as_ref().map(|l| l.root.clone()) {
            if self.order.rename(&root, old, new) {
                let _ = self.order.save(&root);
            }
        }
    }

    /// 處理檔案樹的拖放：移到別的資料夾，或在同一個資料夾裡調整順序。
    fn drop_item(&mut self, src: PathBuf, target: PathBuf, place: Place) -> Result<(), String> {
        let root = self.library.as_ref().map(|l| l.root.clone()).ok_or("沒有開啟書庫")?;
        let dest_dir = match place {
            Place::Into => target.clone(),
            _ => target.parent().map(Path::to_path_buf).ok_or("無效的位置")?,
        };
        if !dest_dir.starts_with(&root) {
            return Err("只能移到書庫裡".into());
        }
        if dest_dir.starts_with(&src) {
            return Err("不能把資料夾移到自己裡面".into());
        }
        self.save_current();
        let mut item = src.clone();
        if src.parent() != Some(dest_dir.as_path()) {
            item = library::move_into(&src, &dest_dir).map_err(|e| e.to_string())?;
            self.relocate(&src, &item);
            self.refresh();
            if place == Place::Into {
                let name = dest_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.info(format!("已移到「{name}」"));
            }
        }
        if place == Place::Into {
            return Ok(());
        }

        // 書本裡的章節：調整 book.toml 的章節順序
        let book_root =
            self.books.values().find(|b| b.chapters.iter().any(|c| c.path == target)).map(|b| b.root.clone());
        if let Some(book_root) = book_root {
            let mut book = Book::open(&book_root).map_err(|e| e.to_string())?;
            let rel_of = |p: &Path| book.chapters.iter().find(|c| c.path == p).map(|c| c.rel.clone());
            if let (Some(from), Some(to)) = (rel_of(&item), rel_of(&target)) {
                let list = &mut book.meta.chapters;
                list.retain(|c| *c != from);
                let at = list.iter().position(|c| *c == to).unwrap_or(list.len());
                list.insert(if place == Place::After { at + 1 } else { at }, from);
                book.save().map_err(|e| e.to_string())?;
                self.refresh();
                return Ok(());
            }
        }

        // 一般資料夾：寫入自訂排序
        let names: Vec<String> = self
            .library
            .as_ref()
            .and_then(|l| l.children_of(&dest_dir))
            .map(|c| c.iter().map(|e| e.name.clone()).collect())
            .unwrap_or_default();
        let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let (item_name, target_name) = (name(&item), name(&target));
        let mut names: Vec<String> = names.into_iter().filter(|n| *n != item_name).collect();
        let at = names.iter().position(|n| *n == target_name).unwrap_or(names.len());
        names.insert(if place == Place::After { at + 1 } else { at }, item_name);
        self.order.set(&root, &dest_dir, names);
        self.order.save(&root).map_err(|e| e.to_string())?;
        if self.settings.sort.key != SortKey::Custom {
            self.settings.sort.key = SortKey::Custom;
            self.settings.sort.reverse = false;
            self.info("已改用自訂排序（可在側欄的排序按鈕切換回來）");
        }
        self.resort();
        self.refresh_novel();
        Ok(())
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

    /// 開啟檔案：Markdown 進編輯器，PDF 進檢視器。
    fn open_file(&mut self, path: &Path) {
        self.save_current();
        self.reveal_in_tree(path);
        let anchor = self.pending_anchor.take();
        if shosai_core::is_pdf(path) {
            if cfg!(feature = "pdf") {
                self.doc = None;
                let page = self.settings.pdf_pages.get(path).copied().unwrap_or(0);
                self.pdf = Some(PdfView::open(path, &self.ctx, self.settings.pdf_fit, self.settings.pdf_thumbs, page));
                self.settings.last_file = Some(path.to_path_buf());
            } else {
                open_path(path);
            }
            return;
        }
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let words = markdown::word_count(&text);
                let saved_hash = content_hash(text.as_bytes());
                self.pdf = None;
                self.doc = Some(OpenDoc {
                    path: path.to_path_buf(),
                    text,
                    dirty: false,
                    last_edit: Instant::now(),
                    mtime: mtime(path),
                    saved_hash,
                    words,
                    words_stale: false,
                });
                self.settings.last_file = Some(path.to_path_buf());
                self.md_view = MdView::default();
                if let Some(a) = anchor {
                    self.md_view.scroll_to(&a);
                }
            }
            Err(e) => self.error(format!("無法開啟 {}：{e}", path.display())),
        }
    }

    /// 展開側欄中這個檔案所在的資料夾。
    fn reveal_in_tree(&mut self, path: &Path) {
        let Some(root) = self.library.as_ref().map(|l| l.root.clone()) else { return };
        let mut p = path.parent();
        while let Some(dir) = p {
            if !dir.starts_with(&root) || dir == root {
                break;
            }
            self.settings.expanded.insert(dir.to_path_buf());
            p = dir.parent();
        }
    }

    /// 儲存目前的檔案。回傳是否單純地存好了（失敗或有需要注意的事時回傳 false，訊息已顯示在狀態列）。
    fn save_current(&mut self) -> bool {
        let Some(doc) = self.doc.as_mut() else { return true };
        if !doc.dirty {
            return true;
        }
        // 開啟之後檔案被別的程式改過（例如同步軟體送進來的新版本）→ 先把那一版另存一份，
        // 不要直接蓋掉。和 Nextcloud 同步的衝突規則一樣：兩份都保留。
        let mut kept = None;
        let on_disk = mtime(&doc.path);
        if doc.mtime.is_some() && on_disk.is_some() && on_disk != doc.mtime {
            if let Ok(theirs) = std::fs::read(&doc.path) {
                if content_hash(&theirs) != doc.saved_hash && theirs != doc.text.as_bytes() {
                    let copy = conflict_path(&doc.path, &shosai_core::timestamp());
                    if let Err(e) = std::fs::write(&copy, &theirs) {
                        let msg = format!("這個檔案在外部被修改過，但無法另存那個版本（{e}），所以先不儲存");
                        self.error(msg);
                        return false;
                    }
                    kept = Some(copy);
                }
            }
        }
        match std::fs::write(&doc.path, &doc.text) {
            Ok(()) => {
                doc.dirty = false;
                doc.mtime = mtime(&doc.path);
                doc.saved_hash = content_hash(doc.text.as_bytes());
                // 標題可能改了 → 更新書本章節名稱
                let path = doc.path.clone();
                if let Some(book) = self.books.values_mut().find(|b| b.chapters.iter().any(|c| c.path == path)) {
                    let _ = book.refresh();
                }
                match kept {
                    Some(copy) => {
                        let name = copy.file_name().unwrap_or_default().to_string_lossy().into_owned();
                        self.error(format!("這個檔案在外部被修改過：你的內容已儲存，外部的版本另存為「{name}」"));
                        false
                    }
                    None => true,
                }
            }
            Err(e) => {
                let msg = format!("儲存失敗：{e}");
                self.error(msg);
                false
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

    fn current_path(&self) -> Option<&Path> {
        if let Some(ch) = self.novel.as_ref().and_then(|n| n.current_chapter()) {
            return Some(ch);
        }
        self.doc.as_ref().map(|d| d.path.as_path()).or(self.pdf.as_ref().map(|p| p.path.as_path()))
    }

    /// 閱讀模式的「上一章 / 下一章」順序：書本章節，否則為同資料夾的檔案。
    fn reading_sequence(&self, path: &Path) -> Vec<(PathBuf, String)> {
        if let Some(book) = self.book_of(path) {
            return book.chapters.iter().map(|c| (c.path.clone(), c.title.clone())).collect();
        }
        let Some(dir) = path.parent() else { return Vec::new() };
        // 依檔案樹目前的排序
        let from_tree: Option<Vec<PathBuf>> = self
            .library
            .as_ref()
            .and_then(|l| l.children_of(dir))
            .map(|c| c.iter().filter(|e| e.kind == EntryKind::Markdown).map(|e| e.path.clone()).collect());
        let files = from_tree.unwrap_or_else(|| {
            let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
                .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| shosai_core::is_markdown(p)).collect())
                .unwrap_or_default();
            files.sort_by(|a, b| library::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
            files
        });
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

    fn poll_export(&mut self) {
        let Some(rx) = &self.export_job else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.export_job = None;
        match result {
            Ok(p) => {
                // 輸出的 PDF 要出現在側欄
                self.refresh();
                self.status = Some(Status::Exported(p));
            }
            Err(e) => self.error(e),
        }
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

    /// 處理預覽 / 閱讀畫面中的點擊（待辦勾選、連結）。
    fn handle_view_events(&mut self, ctx: &egui::Context) {
        for ev in std::mem::take(&mut self.view_events) {
            match ev {
                ViewEvent::ToggleTask(offset) => {
                    let Some(doc) = self.doc.as_mut() else { continue };
                    let replacement = match doc.text.get(offset..offset + 3) {
                        Some("[ ]") => "[x]",
                        Some("[x]") | Some("[X]") => "[ ]",
                        _ => continue,
                    };
                    doc.text.replace_range(offset..offset + 3, replacement);
                    doc.dirty = true;
                    doc.words_stale = true;
                    doc.last_edit = Instant::now();
                }
                ViewEvent::OpenLink(url) => {
                    if url.starts_with('#') {
                        self.md_view.scroll_to(&url);
                    } else if url.contains("://") || url.starts_with("mailto:") {
                        ctx.open_url(egui::OpenUrl::new_tab(url));
                    } else if let Some(dir) = self.doc.as_ref().and_then(|d| d.path.parent()) {
                        // 相對連結：Markdown / PDF 在書齋中開啟，其他檔案交給系統
                        let (file, anchor) = url.split_once('#').unwrap_or((url.as_str(), ""));
                        let target = dir.join(shosai_core::export::percent_decode_pub(file));
                        let ours = shosai_core::is_markdown(&target) || shosai_core::is_pdf(&target);
                        if ours && target.is_file() {
                            self.actions.push(Action::Open(target));
                            // 開檔時預覽會整個重建，所以先記下來，開好之後再捲過去
                            self.pending_anchor = (!anchor.is_empty()).then(|| anchor.to_string());
                        } else if target.exists() {
                            self.actions.push(Action::Reveal(target));
                        } else {
                            self.error(format!("找不到連結的檔案：{file}"));
                        }
                    }
                }
            }
        }
    }

    fn process_actions(&mut self, ctx: &egui::Context) {
        for action in std::mem::take(&mut self.actions) {
            match action {
                Action::Open(p) => {
                    // 書本模式中點了同一本書的章節 → 直接翻到那一章
                    if let Some(n) = self.novel.as_mut() {
                        if n.goto_chapter(&p) {
                            continue;
                        }
                        self.close_novel(false);
                    }
                    self.open_file(&p)
                }
                Action::Drop { src, target, place } => {
                    if let Err(e) = self.drop_item(src, target, place) {
                        self.error(e);
                    }
                }
                Action::OpenNovel { root, chapter } => self.open_novel(&root, chapter.as_deref()),
                Action::OpenLibrary(p) => {
                    self.save_current();
                    self.doc = None;
                    self.pdf = None;
                    self.open_library(&p);
                }
                Action::PickLibrary => {
                    if let Some(dir) = rfd::FileDialog::new().set_title("選擇書庫資料夾").pick_folder() {
                        self.save_current();
                        self.doc = None;
                        self.pdf = None;
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
        // 在任何地方按下滑鼠 → 方向鍵交還給文件（點檔案樹的資料夾時，放開後會再交給檔案樹）
        if ctx.input(|i| i.pointer.any_pressed()) || !self.settings.sidebar {
            self.tree_nav.active = false;
        }
        let (goto, settings, find_file) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::G)),
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Comma)),
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::P)),
            )
        });
        if find_file {
            self.settings.sidebar = true;
            self.focus_filter = true;
        }
        // 正在編輯器裡打字時，Ctrl+B 是粗體（在 editor 裡處理），其他時候才是開關側欄
        let editing = self.doc.as_ref().is_some_and(|d| ctx.memory(|m| m.has_focus(editor_id(&d.path))));
        if goto {
            if let Some(pdf) = self.pdf.as_mut() {
                pdf.focus_page_input();
            }
        }
        if settings {
            self.show_settings = !self.show_settings;
        }
        let (save, edit, split, read, book_mode, sidebar, new_note, mut zoom_in, mut zoom_out) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::S)),
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Num1)),
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Num2)),
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Num3)),
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Num4)),
                (!editing && i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::B)))
                    || i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Backslash)),
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::N)),
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Equals))
                    || i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Plus)),
                i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::Minus)),
            )
        });
        if save && self.save_current() {
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
        if (edit || split || read) && self.novel.is_some() {
            self.close_novel(true);
        }
        if book_mode && self.novel.is_none() {
            self.open_novel_here();
        }
        if sidebar {
            self.settings.sidebar = !self.settings.sidebar;
        }
        if new_note {
            if let Some(dir) = self.current_dir() {
                self.dialog = Some(Dialog::NewNote { dir, name: String::new() });
            }
        }
        // Ctrl+滾輪：文字頁面調整字級（PDF 由檢視器自己以游標為中心縮放）
        if self.pdf.is_none() || self.novel.is_some() {
            let delta = ctx.input(|i| i.zoom_delta());
            if delta == 1.0 {
                self.wheel_zoom = 0.0;
            } else {
                self.wheel_zoom += delta.ln();
                const STEP: f32 = 0.18;
                if self.wheel_zoom >= STEP {
                    zoom_in = true;
                    self.wheel_zoom = 0.0;
                } else if self.wheel_zoom <= -STEP {
                    zoom_out = true;
                    self.wheel_zoom = 0.0;
                }
            }
        }
        if zoom_in || zoom_out {
            if self.novel.is_some() {
                let f = &mut self.settings.novel.font_size;
                *f = (*f + if zoom_in { 1.0 } else { -1.0 }).clamp(12.0, 32.0);
            } else if let Some(pdf) = self.pdf.as_mut() {
                pdf.zoom_by(if zoom_in { 1.2 } else { 1.0 / 1.2 });
            } else {
                let d = if zoom_in { 1.0 } else { -1.0 };
                if self.settings.view == ViewMode::Read {
                    self.settings.reader_size = (self.settings.reader_size + d).clamp(10.0, 40.0);
                } else {
                    self.settings.editor_size = (self.settings.editor_size + d).clamp(9.0, 40.0);
                }
            }
        }

        // 檔案拖曳進視窗
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect()
        });
        if let Some(p) = dropped.into_iter().next() {
            if p.is_dir() {
                self.actions.push(Action::OpenLibrary(p));
            } else if shosai_core::is_markdown(&p) || shosai_core::is_pdf(&p) {
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
        self.current_path()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .or_else(|| self.library.as_ref().map(|l| l.root.clone()))
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        if self.applied_scale != self.settings.ui_scale {
            self.applied_scale = self.settings.ui_scale;
            ctx.set_zoom_factor(self.settings.ui_scale.clamp(0.7, 2.0));
        }
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

    fn toolbar(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let tip = if self.settings.sidebar { "隱藏側欄（Ctrl+B）" } else { "顯示側欄（Ctrl+B）" };
            if icons::button(ui, Icon::Sidebar, tip).clicked() {
                self.settings.sidebar = !self.settings.sidebar;
            }
            if icons::button(ui, Icon::Gear, "設定（Ctrl+,）").clicked() {
                self.show_settings = true;
            }
            ui.add_space(10.0);

            let mut novel_events = Vec::new();
            if let Some(n) = self.novel.as_mut() {
                n.toolbar_left(ui, &mut self.settings.novel, &mut novel_events);
            } else if self.doc.is_some() {
                self.mode_switch(ui);
            } else if let Some(pdf) = self.pdf.as_mut() {
                pdf.toolbar_left(ui, pal);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                self.more_menu(ui);
                if let Some(n) = self.novel.as_mut() {
                    n.toolbar_right(ui, &mut self.settings.novel);
                } else if let Some(pdf) = self.pdf.as_mut() {
                    if icons::button(ui, Icon::External, "用預設程式開啟").clicked() {
                        self.actions.push(Action::Reveal(pdf.path.clone()));
                    }
                    ui.add_space(6.0);
                    pdf.toolbar_right(ui, pal);
                } else if self.doc.is_some() {
                    self.export_menu(ui);
                }
            });
            self.handle_novel_events(novel_events);
        });
    }

    fn handle_novel_events(&mut self, events: Vec<NovelEvent>) {
        for ev in events {
            match ev {
                NovelEvent::Close => self.close_novel(true),
            }
        }
    }

    /// 編輯 / 分割 / 閱讀 的分段切換。
    fn mode_switch(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new().fill(ui.visuals().faint_bg_color).corner_radius(8).inner_margin(2).show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (mode, label, key) in [
                (ViewMode::Edit, "編輯", "Ctrl+1"),
                (ViewMode::Split, "分割", "Ctrl+2"),
                (ViewMode::Read, "閱讀", "Ctrl+3"),
            ] {
                let selected = self.settings.view == mode;
                let text = if selected { RichText::new(label).strong() } else { RichText::new(label) };
                let btn = egui::Button::selectable(selected, text).corner_radius(6).min_size(egui::vec2(52.0, 24.0));
                if ui.add(btn).on_hover_text(key).clicked() {
                    self.settings.view = mode;
                }
            }
            let btn = egui::Button::selectable(false, "書本").corner_radius(6).min_size(egui::vec2(52.0, 24.0));
            if ui.add(btn).on_hover_text("以書本模式閱讀這個資料夾，從這一章開始（Ctrl+4）").clicked()
            {
                self.open_novel_here();
            }
        });
    }

    /// 以書本模式閱讀目前檔案所在的書 / 資料夾，從目前這一章開始。
    fn open_novel_here(&mut self) {
        let Some(path) = self.doc.as_ref().map(|d| d.path.clone()) else { return };
        let root = self.book_of(&path).map(|b| b.root.clone()).or_else(|| path.parent().map(Path::to_path_buf));
        if let Some(root) = root {
            self.open_novel(&root, Some(&path));
        }
    }

    fn export_menu(&mut self, ui: &mut egui::Ui) {
        let resp = icons::button(ui, Icon::Export, "輸出");
        let current = self.doc.as_ref().map(|d| d.path.clone());
        let book_root = current.as_ref().and_then(|p| self.book_of(p)).map(|b| b.root.clone());
        let busy = self.export_job.is_some();
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(170.0);
            if busy {
                ui.label(RichText::new("輸出進行中…").weak());
                return;
            }
            if let Some(root) = &book_root {
                ui.label(RichText::new("整本書").weak().small());
                for format in [Format::Pdf, Format::Html, Format::Typst] {
                    if ui.button(format!("{}…", format.label())).clicked() {
                        self.actions.push(Action::Export { source: root.clone(), format });
                        ui.close();
                    }
                }
                ui.add_space(4.0);
            }
            if let Some(p) = &current {
                ui.label(RichText::new("這個檔案").weak().small());
                for format in [Format::Pdf, Format::Html, Format::Typst] {
                    if ui.button(format!("{}…", format.label())).clicked() {
                        self.actions.push(Action::Export { source: p.clone(), format });
                        ui.close();
                    }
                }
            }
        });
    }

    fn more_menu(&mut self, ui: &mut egui::Ui) {
        let resp = icons::button(ui, Icon::More, "更多");
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(170.0);
            if let Some(root) = self.library.as_ref().map(|l| l.root.clone()) {
                if ui.button("在檔案管理員中開啟書庫").clicked() {
                    self.actions.push(Action::Reveal(root));
                    ui.close();
                }
            }
            if ui.button("開啟其他資料夾…").clicked() {
                self.actions.push(Action::PickLibrary);
                ui.close();
            }
            ui.separator();
            if ui.button("鍵盤快捷鍵").clicked() {
                self.dialog = Some(Dialog::Shortcuts);
                ui.close();
            }
            if ui.button("設定…").clicked() {
                self.show_settings = true;
                ui.close();
            }
            if ui.button("關於書齋").clicked() {
                self.settings_tab = SettingsTab::About;
                self.show_settings = true;
                ui.close();
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if let Some(n) = &self.novel {
                let (left, right) = n.status();
                ui.label(RichText::new(left).small().color(pal.weak));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(right).small().color(pal.weak));
                });
                return;
            }
            // 左：目前檔案位置
            if let Some(path) = self.current_path().map(Path::to_path_buf) {
                let rel = self
                    .library
                    .as_ref()
                    .and_then(|l| path.strip_prefix(&l.root).ok())
                    .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("  /  "))
                    .unwrap_or_else(|| path.display().to_string());
                ui.label(RichText::new(rel).small().color(pal.weak));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let mut save = false;
                if let Some(doc) = self.doc.as_mut() {
                    if doc.words_stale && doc.last_edit.elapsed() > Duration::from_millis(400) {
                        doc.words = markdown::word_count(&doc.text);
                        doc.words_stale = false;
                    }
                    let (text, color) = if doc.dirty { ("未儲存", pal.accent) } else { ("已儲存", pal.weak) };
                    let resp =
                        ui.add(egui::Label::new(RichText::new(text).small().color(color)).sense(egui::Sense::click()));
                    save = resp.on_hover_text("Ctrl+S 儲存（停止輸入後也會自動儲存）").clicked();
                    if self.settings.show_word_count {
                        ui.label(RichText::new(format!("{} 字", doc.words)).small().color(pal.weak));
                    }
                }
                if save {
                    self.save_current();
                }

                let mut clear = false;
                match &self.status {
                    Some(Status::Info(msg, t)) => {
                        ui.label(RichText::new(msg).small());
                        clear = t.elapsed() > Duration::from_secs(4);
                    }
                    Some(Status::Error(msg, t)) => {
                        ui.label(
                            RichText::new(msg.lines().next().unwrap_or("")).small().color(ui.visuals().error_fg_color),
                        )
                        .on_hover_text(msg);
                        clear = t.elapsed() > Duration::from_secs(15);
                    }
                    Some(Status::Working(msg)) => {
                        ui.label(RichText::new(msg).small());
                        ui.add(egui::Spinner::new().size(12.0));
                    }
                    Some(Status::Exported(p)) => {
                        if ui.add(egui::Button::new(RichText::new("關閉").small()).small()).clicked() {
                            clear = true;
                        }
                        if ui.add(egui::Button::new(RichText::new("開啟資料夾").small()).small()).clicked() {
                            self.actions.push(Action::Reveal(p.parent().unwrap_or(p).to_path_buf()));
                        }
                        let view = if shosai_core::is_pdf(p) && cfg!(feature = "pdf") { "檢視" } else { "開啟" };
                        if ui.add(egui::Button::new(RichText::new(view).small()).small()).clicked() {
                            if view == "檢視" {
                                self.actions.push(Action::Open(p.clone()));
                            } else {
                                self.actions.push(Action::Reveal(p.clone()));
                            }
                            clear = true;
                        }
                        ui.label(
                            RichText::new(format!("已輸出 {}", p.file_name().unwrap_or_default().to_string_lossy()))
                                .small(),
                        );
                    }
                    None => {}
                }
                if clear {
                    self.status = None;
                }
            });
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let Some(lib) = &self.library else {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("尚未開啟書庫").color(pal.weak));
                ui.add_space(6.0);
                if ui.button("開啟資料夾…").clicked() {
                    self.actions.push(Action::PickLibrary);
                }
            });
            return;
        };
        let root = lib.root.clone();
        let root_name =
            root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());

        // 標題列：書庫名稱（點擊切換書庫）＋ 新增 / 重新整理 / 同步
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let name = egui::Button::new(RichText::new(&root_name).strong().size(15.0)).frame_when_inactive(false);
            let resp = ui.add(name).on_hover_text(root.display().to_string());
            egui::Popup::menu(&resp).show(|ui| {
                ui.set_min_width(220.0);
                tree::novel_item(ui, &root, &mut self.actions);
                ui.separator();
                if ui.button("開啟其他資料夾…").clicked() {
                    self.actions.push(Action::PickLibrary);
                    ui.close();
                }
                let recent: Vec<PathBuf> =
                    self.settings.recent_libraries.iter().filter(|r| **r != root && r.is_dir()).cloned().collect();
                if !recent.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new("最近使用").weak().small());
                    for r in recent {
                        let label = r.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        if ui.button(label).on_hover_text(r.display().to_string()).clicked() {
                            self.actions.push(Action::OpenLibrary(r));
                            ui.close();
                        }
                    }
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let plus = icons::button_sized(ui, Icon::Plus, "新增", 26.0);
                egui::Popup::menu(&plus).show(|ui| {
                    ui.set_min_width(150.0);
                    tree::new_menu(ui, &root, &mut self.actions);
                });
                if icons::button_sized(ui, Icon::Refresh, "重新整理", 26.0).clicked() {
                    self.actions.push(Action::Refresh);
                }
                if icons::button_sized(ui, Icon::Collapse, "全部收合", 26.0).clicked() {
                    self.settings.expanded.clear();
                }
                let sort = icons::button_sized(ui, Icon::Sort, "排序方式", 26.0);
                egui::Popup::menu(&sort).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                    ui.set_min_width(200.0);
                    ui.set_max_width(240.0);
                    let before = self.settings.sort;
                    let st = &mut self.settings.sort;
                    ui.label(RichText::new("排序依據").weak().small());
                    for (key, label) in [
                        (SortKey::Name, "名稱"),
                        (SortKey::Modified, "修改時間（新的在前）"),
                        (SortKey::Created, "建立時間（新的在前）"),
                        (SortKey::Custom, "自訂（拖曳排列）"),
                    ] {
                        ui.radio_value(&mut st.key, key, label);
                    }
                    ui.separator();
                    ui.checkbox(&mut st.reverse, "反向排列");
                    ui.checkbox(&mut st.folders_first, "資料夾排在前面");
                    ui.add_space(2.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(
                                "拖曳檔案可以調整順序；拖到資料夾上可以移進去，拖到清單下方空白處會移到最上層。",
                            )
                            .small()
                            .color(pal.weak),
                        )
                        .wrap(),
                    );
                    if self.settings.sort != before {
                        self.resort();
                        self.refresh_novel();
                    }
                });
                #[cfg(feature = "nextcloud")]
                {
                    let busy = self.sync_job.is_some();
                    let resp = ui
                        .add_enabled_ui(!busy, |ui| {
                            icons::button_sized(ui, Icon::Cloud, "與 Nextcloud 同步（右鍵：設定）", 26.0)
                        })
                        .inner;
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
        ui.add_space(6.0);

        // 搜尋框
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .corner_radius(7)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let h = ui.text_style_height(&TextStyle::Body);
                    let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, h), egui::Sense::hover());
                    let icon_rect = egui::Rect::from_center_size(r.center(), egui::vec2(14.0, 14.0));
                    icons::paint(ui.painter(), icon_rect, Icon::Search, pal.weak);
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.filter)
                            .hint_text("搜尋檔案（Ctrl+P）")
                            .frame(egui::Frame::NONE)
                            .desired_width(f32::INFINITY),
                    );
                    if std::mem::take(&mut self.focus_filter) {
                        resp.request_focus();
                    }
                    if resp.has_focus() && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
                        // ↓：移到清單的第一列，接著用方向鍵選、Enter 開啟
                        resp.surrender_focus();
                        self.tree_nav.active = true;
                        self.tree_nav.cursor = self.tree_nav.rows.first().map(|r| r.path.clone());
                    }
                    if resp.lost_focus() {
                        // Esc（egui 會先讓輸入框失去焦點）：清掉搜尋。把按鍵用掉，免得同時結束書本模式
                        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                            self.filter.clear();
                        }
                        // Enter：直接開啟第一個符合的檔案
                        if !self.filter.trim().is_empty() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            if let Some(first) = self.tree_nav.first_file() {
                                self.actions.push(Action::Open(first.to_path_buf()));
                            }
                        }
                    }
                });
            });
        ui.add_space(8.0);

        let current = self.current_path().map(Path::to_path_buf);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let lib = self.library.as_ref().unwrap();
            self.tree_nav.rows.clear();
            let mut cx = TreeCtx {
                current: current.as_deref(),
                nav: &mut self.tree_nav,
                books: &self.books,
                actions: &mut self.actions,
                expanded: &mut self.settings.expanded,
                pal: *pal,
                show_pdfs: self.settings.show_pdfs,
                show_counts: self.settings.show_counts,
                lib_root: &root,
            };
            if !self.filter.trim().is_empty() {
                let q = self.filter.trim().to_lowercase();
                let mut any = false;
                for e in lib.all_files() {
                    let rel = e.path.strip_prefix(&root).unwrap_or(&e.path).to_string_lossy().into_owned();
                    if rel.to_lowercase().contains(&q) {
                        any = true;
                        tree::file_labeled(ui, &e.path, &rel, e.kind == EntryKind::Pdf, 0, &mut cx);
                    }
                }
                if !any {
                    ui.add_space(8.0);
                    ui.label(RichText::new("沒有符合的檔案").color(pal.weak));
                }
            } else if let Some(book) = self.books.get(&root) {
                let entry = Entry {
                    path: root.clone(),
                    name: root_name.clone(),
                    kind: EntryKind::Book,
                    children: lib.entries.clone(),
                    modified: None,
                    created: None,
                };
                tree::book_contents(ui, book, &entry, 0, &mut cx);
            } else if lib.entries.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("這個資料夾還沒有 Markdown 或 PDF 檔。\n用上方的 + 新增筆記或書本。").color(pal.weak),
                );
            } else {
                tree::entries(ui, &lib.entries, 0, &mut cx);
            }
            // 鍵盤操作（有對話框或輸入框取得焦點時不作用）
            if self.dialog.is_none() && !self.show_settings && ui.memory(|m| m.focused().is_none()) {
                tree::keyboard(ui, &mut self.tree_nav, &mut self.settings.expanded, &mut self.actions);
            }
            // 拖到清單下方的空白處 → 移到書庫最上層
            let rest = ui.available_rect_before_wrap();
            let rest = egui::Rect::from_min_size(rest.min, egui::vec2(rest.width(), rest.height().max(48.0)));
            let resp = ui.allocate_rect(rest, egui::Sense::hover());
            if let Some(item) = resp.dnd_hover_payload::<DragItem>() {
                if item.0.parent() != Some(root.as_path()) {
                    let r = egui::Rect::from_min_size(rest.min, egui::vec2(rest.width(), 36.0));
                    ui.painter().rect_stroke(
                        r.shrink(1.0),
                        6.0,
                        egui::Stroke::new(1.5, pal.accent),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        "移到書庫最上層",
                        FontId::proportional(12.5),
                        pal.accent,
                    );
                    if resp.dnd_release_payload::<DragItem>().is_some() {
                        self.actions.push(Action::Drop {
                            src: item.0.clone(),
                            target: root.clone(),
                            place: Place::Into,
                        });
                    }
                }
            }
        });

        // 拖曳中：在游標旁顯示檔名
        if let (Some(item), Some(pos)) =
            (egui::DragAndDrop::payload::<DragItem>(ui.ctx()), ui.ctx().pointer_latest_pos())
        {
            let name = item.0.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("shosai-drag"));
            let p = ui.ctx().layer_painter(layer);
            let galley = p.layout_no_wrap(name, FontId::proportional(13.0), pal.ink);
            let r = egui::Rect::from_min_size(pos + egui::vec2(14.0, 6.0), galley.size() + egui::vec2(16.0, 8.0));
            p.rect_filled(r, 6.0, pal.chrome);
            p.rect_stroke(r, 6.0, egui::Stroke::new(1.0, pal.guide), egui::StrokeKind::Inside);
            p.galley(r.min + egui::vec2(8.0, 4.0), galley, pal.ink);
        }
    }

    fn editor(&mut self, ui: &mut egui::Ui, centered: bool) {
        let size = self.settings.editor_size;
        let proportional = self.settings.editor_proportional;
        let Some(doc) = self.doc.as_mut() else { return };
        let id = editor_id(&doc.path);
        egui::ScrollArea::vertical().id_salt(("editor", &doc.path)).auto_shrink([false, false]).show(ui, |ui| {
            let full = ui.available_width();
            let width = if centered { full.min(800.0) } else { full };
            ui.add_space(if centered { 28.0 } else { 8.0 });
            let pad: i8 = if centered { 28 } else { 22 };
            ui.horizontal(|ui| {
                ui.add_space(((full - width) / 2.0).max(0.0));
                // 格式快捷鍵：要在編輯器處理按鍵之前先攔下來
                let mut formatted = false;
                if ui.memory(|m| m.has_focus(id)) {
                    let cmd = Modifiers::COMMAND;
                    let marks = ui.input_mut(|i| {
                        if i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::B)) {
                            Some(("**", "**"))
                        } else if i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::I)) {
                            Some(("*", "*"))
                        } else if i.consume_shortcut(&KeyboardShortcut::new(cmd, Key::K)) {
                            Some(("[", "]()"))
                        } else {
                            None
                        }
                    });
                    if let Some((open, close)) = marks {
                        formatted = format_selection(ui.ctx(), id, &mut doc.text, open, close);
                    }
                }
                let response = egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(pad, 6))
                    .show(ui, |ui| {
                        let edit = egui::TextEdit::multiline(&mut doc.text)
                            .id(id)
                            .font(if proportional { FontId::proportional(size) } else { FontId::monospace(size) })
                            .desired_width(width - 2.0 * pad as f32)
                            .desired_rows(30)
                            .lock_focus(true)
                            .frame(egui::Frame::NONE)
                            .hint_text("# 標題\n\n開始寫作……");
                        ui.add(edit)
                    })
                    .inner;
                if response.changed() || formatted {
                    doc.dirty = true;
                    doc.words_stale = true;
                    doc.last_edit = Instant::now();
                }
            });
        });
    }

    fn preview(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = self.doc.as_ref() else { return };
        let base = doc.path.parent().map(Path::to_path_buf).unwrap_or_default();
        let md_view = &mut self.md_view;
        let events = &mut self.view_events;
        egui::ScrollArea::vertical().id_salt(("preview", &doc.path)).auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin { left: 24, right: 24, top: 16, bottom: 32 }).show(ui, |ui| {
                events.extend(md_view.show(ui, &doc.text, &base));
            });
        });
    }

    fn reader(&mut self, ui: &mut egui::Ui, pal: &Palette) {
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

        let (desk, paper, ink, weak) = (pal.desk, pal.paper, pal.ink, pal.weak);
        let size = self.settings.reader_size;
        let max_w = self.settings.reader_width;
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let md_view = &mut self.md_view;
        let events = &mut self.view_events;
        let actions = &mut self.actions;
        let dark = ui.visuals().dark_mode;
        // 檔案樹正在用方向鍵時，不要同時捲動文件
        let scroll_keys = !self.tree_nav.active;

        egui::Frame::new().fill(desk).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt(("reader", &path)).auto_shrink([false, false]).show(ui, |ui| {
                let full = ui.available_width();
                let narrow = full < 640.0;
                let pad: i8 = if narrow { 22 } else { 60 };
                let page_w = full.min(max_w + 2.0 * pad as f32).max(200.0);
                ui.add_space(if narrow { 0.0 } else { 28.0 });
                ui.horizontal(|ui| {
                    ui.add_space(((full - page_w) / 2.0).max(0.0));
                    egui::Frame::new()
                        .fill(paper)
                        .corner_radius(if narrow { 0 } else { 6 })
                        .shadow(egui::Shadow {
                            offset: [0, 6],
                            blur: 24,
                            spread: 0,
                            color: Color32::from_black_alpha(if dark { 70 } else { 22 }),
                        })
                        .inner_margin(egui::Margin::symmetric(pad, 48))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.set_width(page_w - 2.0 * pad as f32);
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
                                        ui.label(RichText::new(t).size(size * 0.72).color(weak));
                                    });
                                    ui.add_space(size * 1.5);
                                }
                                events.extend(md_view.show(ui, &doc.text, &base));

                                // 章末：上一章 / 下一章
                                ui.add_space(size * 3.0);
                                ui.horizontal(|ui| {
                                    if let Some((p, t)) = &prev {
                                        let b = egui::Button::new(RichText::new(format!("‹  {t}")).color(weak));
                                        if ui.add(b).on_hover_text("上一章（←）").clicked() {
                                            actions.push(Action::Open(p.clone()));
                                        }
                                    }
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if let Some((p, t)) = &next {
                                            let b = egui::Button::new(RichText::new(format!("{t}  ›")).color(weak));
                                            if ui.add(b).on_hover_text("下一章（→）").clicked() {
                                                actions.push(Action::Open(p.clone()));
                                            }
                                        }
                                        if let (Some(i), true) = (pos, seq.len() > 1) {
                                            ui.centered_and_justified(|ui| {
                                                ui.label(
                                                    RichText::new(format!("{} / {}", i + 1, seq.len()))
                                                        .size(size * 0.75)
                                                        .color(weak),
                                                );
                                            });
                                        }
                                    });
                                });
                            })
                        });
                });
                ui.add_space(36.0);
                if scroll_keys {
                    crate::keys::scroll(ui, ui.min_rect(), true);
                }
            });
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.24).max(24.0));
            ui.label(RichText::new("書齋").size(46.0).color(pal.ink));
            ui.add_space(4.0);
            ui.label(RichText::new("輕量的 Markdown 寫作與閱讀器").size(15.0).color(pal.weak));
            ui.add_space(32.0);
            if self.library.is_none() {
                let open = egui::Button::new(RichText::new("開啟資料夾…").size(15.0).color(Color32::WHITE))
                    .fill(pal.accent)
                    .corner_radius(8)
                    .min_size(egui::vec2(160.0, 36.0));
                if ui.add(open).clicked() {
                    self.actions.push(Action::PickLibrary);
                }
                ui.add_space(10.0);
                ui.label(RichText::new("也可以把資料夾、Markdown 或 PDF 檔拖曳進視窗").small().color(pal.weak));
                let recent: Vec<PathBuf> =
                    self.settings.recent_libraries.iter().filter(|r| r.is_dir()).cloned().collect();
                if !recent.is_empty() {
                    ui.add_space(28.0);
                    ui.label(RichText::new("最近使用").small().color(pal.weak));
                    ui.add_space(4.0);
                    for r in recent {
                        let name = r.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        if ui.button(name).on_hover_text(r.display().to_string()).clicked() {
                            self.actions.push(Action::OpenLibrary(r));
                        }
                    }
                }
            } else {
                ui.label(RichText::new("從左側選一個檔案開始，或按 + 新增筆記、建立書本").color(pal.weak));
                ui.add_space(16.0);
                for (k, v) in [("Ctrl+N", "新增筆記"), ("Ctrl+1 / 2 / 3", "編輯 / 分割 / 閱讀"), ("Ctrl+B", "側欄")]
                {
                    ui.label(RichText::new(format!("{k}　{v}")).small().color(pal.weak));
                }
            }
        });
    }

    /// 設定視窗（左上角齒輪 / Ctrl+,）。調整會立即套用並自動保存。
    fn settings_window(&mut self, ctx: &egui::Context, pal: &Palette) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let modal = egui::Modal::new(egui::Id::unique("shosai-settings")).show(ctx, |ui| {
            ui.set_width(640.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("設定").size(18.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let done = egui::Button::new(RichText::new("完成").color(Color32::WHITE))
                        .fill(pal.accent)
                        .min_size(egui::vec2(68.0, 28.0));
                    if ui.add(done).clicked() {
                        open = false;
                    }
                });
            });
            ui.add_space(10.0);
            ui.horizontal_top(|ui| {
                // 左：分類
                ui.vertical(|ui| {
                    ui.set_width(128.0);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for tab in SettingsTab::ALL {
                        let selected = self.settings_tab == tab;
                        let btn = egui::Button::selectable(selected, tab.label()).min_size(egui::vec2(124.0, 30.0));
                        if ui.add(btn).clicked() {
                            self.settings_tab = tab;
                        }
                    }
                });
                ui.add_space(12.0);
                // 右：內容
                egui::Frame::new().fill(ui.visuals().faint_bg_color).corner_radius(8).inner_margin(18).show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_width(466.0);
                        ui.set_min_height(380.0);
                        egui::ScrollArea::vertical().max_height(400.0).auto_shrink([false, true]).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 10.0;
                            self.settings_page(ui, pal);
                        });
                    });
                });
            });
        });
        if modal.should_close() {
            open = false;
        }
        self.show_settings = open;
    }

    fn settings_page(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let st = &mut self.settings;
        let heading = |ui: &mut egui::Ui, t: &str| {
            ui.add_space(4.0);
            ui.label(RichText::new(t).strong().size(15.0));
        };
        let hint = |ui: &mut egui::Ui, t: &str| {
            ui.label(RichText::new(t).small().color(pal.weak));
        };
        match self.settings_tab {
            SettingsTab::Appearance => {
                heading(ui, "主題");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut st.dark, None, "跟隨系統");
                    ui.selectable_value(&mut st.dark, Some(false), "淺色");
                    ui.selectable_value(&mut st.dark, Some(true), "深色");
                });
                heading(ui, "介面縮放");
                let mut draft = self.scale_draft.unwrap_or(st.ui_scale);
                ui.horizontal(|ui| {
                    ui.spacing_mut().slider_width = 220.0;
                    if icons::button_sized(ui, Icon::Minus, "縮小 5%", 26.0).clicked() {
                        st.ui_scale = ((st.ui_scale - 0.05) * 20.0).round() / 20.0;
                        st.ui_scale = st.ui_scale.max(0.8);
                        draft = st.ui_scale;
                    }
                    let resp = ui.add(
                        egui::Slider::new(&mut draft, 0.8..=1.5)
                            .step_by(0.05)
                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                    );
                    if icons::button_sized(ui, Icon::Plus, "放大 5%", 26.0).clicked() {
                        st.ui_scale = ((st.ui_scale + 0.05) * 20.0).round() / 20.0;
                        st.ui_scale = st.ui_scale.min(1.5);
                        draft = st.ui_scale;
                    }
                    if resp.dragged() {
                        // 拖動中只更新數字，放開滑鼠才套用
                        self.scale_draft = Some(draft);
                    } else {
                        if resp.changed() || resp.drag_stopped() {
                            st.ui_scale = draft;
                        }
                        self.scale_draft = None;
                    }
                });
                ui.horizontal(|ui| {
                    for (v, label) in [(0.9, "90%"), (1.0, "100%"), (1.1, "110%"), (1.25, "125%"), (1.5, "150%")] {
                        let selected = (st.ui_scale - v).abs() < 0.001 && self.scale_draft.is_none();
                        if ui.add(egui::Button::selectable(selected, label)).clicked() {
                            st.ui_scale = v;
                        }
                    }
                });
                hint(ui, "放大整個介面（含側欄、選單）。拖動拉條時放開滑鼠才會套用；高解析度螢幕可設為 110%–125%。");
            }
            SettingsTab::Editor => {
                heading(ui, "字級");
                ui.add(egui::Slider::new(&mut st.editor_size, 10.0..=28.0).step_by(1.0).suffix(" px"));
                heading(ui, "字型");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut st.editor_proportional, false, "等寬（適合對齊表格、程式碼）");
                    ui.selectable_value(&mut st.editor_proportional, true, "一般");
                });
                heading(ui, "自動儲存");
                ui.add(egui::Slider::new(&mut st.autosave_secs, 0.5..=10.0).step_by(0.5).suffix(" 秒後"));
                hint(ui, "停止輸入一段時間後自動存檔；隨時可按 Ctrl+S 立即儲存。");
                heading(ui, "預設模式");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut st.view, ViewMode::Edit, "編輯");
                    ui.selectable_value(&mut st.view, ViewMode::Split, "分割");
                    ui.selectable_value(&mut st.view, ViewMode::Read, "閱讀");
                });
                ui.checkbox(&mut st.show_word_count, "在狀態列顯示字數");
            }
            SettingsTab::Reader => {
                heading(ui, "字級");
                ui.add(egui::Slider::new(&mut st.reader_size, 12.0..=30.0).step_by(1.0).suffix(" px"));
                heading(ui, "版面寬度");
                ui.add(egui::Slider::new(&mut st.reader_width, 480.0..=1200.0).step_by(20.0).suffix(" px"));
                hint(ui, "閱讀模式中，紙張內文的最大寬度。");
                hint(ui, "閱讀時可用 ← → 翻到上一章 / 下一章，Ctrl+= / Ctrl+- 調整字級。");

                heading(ui, "書本模式");
                hint(ui, "把整個資料夾當成一本書，一頁一頁翻著讀：Markdown 檔是章、子資料夾是卷，名為 cover 或「封面」的圖片會當成封面。在側欄的資料夾按右鍵，或在檔案上方切換到「書本」（Ctrl+4）即可開始。");
                let nv = &mut st.novel;
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut nv.vertical, true, "直排（由右至左）");
                    ui.selectable_value(&mut nv.vertical, false, "橫排");
                });
                ui.checkbox(&mut nv.spread, "視窗夠寬時左右兩頁並排");
                ui.add(egui::Slider::new(&mut nv.font_size, 12.0..=32.0).step_by(1.0).suffix(" px").text("字級"));
                ui.add(egui::Slider::new(&mut nv.line_spacing, 1.3..=2.6).step_by(0.1).suffix(" 倍").text("行距"));
                ui.horizontal(|ui| {
                    ui.label("段首縮排");
                    for (n, label) in [(0, "不縮排"), (1, "一字"), (2, "兩字")] {
                        ui.selectable_value(&mut nv.indent, n, label);
                    }
                });
                ui.checkbox(&mut nv.para_gap, "段落之間空半行");
                hint(ui, "注音寫法：｜漢字《かんじ》。*強調* 會顯示成著重號，--- 或 ◇◇◇ 為場景分隔，單獨一行的圖片會成為插圖頁。");
                ui.horizontal(|ui| {
                    ui.label(format!("已記住 {} 本書的閱讀位置", st.novel_pos.len()));
                    if ui.add_enabled(!st.novel_pos.is_empty(), egui::Button::new("清除")).clicked() {
                        st.novel_pos.clear();
                    }
                });
            }
            SettingsTab::Pdf => {
                heading(ui, "開啟 PDF 時的縮放");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut st.pdf_fit, Fit::Width, "符合寬度");
                    ui.selectable_value(&mut st.pdf_fit, Fit::Page, "整頁顯示");
                    ui.selectable_value(&mut st.pdf_fit, Fit::Zoom(1.0), "100%");
                });
                ui.checkbox(&mut st.pdf_thumbs, "顯示頁面縮圖列");
                if let Some(pdf) = self.pdf.as_mut() {
                    pdf.show_thumbs = st.pdf_thumbs;
                }
                heading(ui, "閱讀位置");
                ui.horizontal(|ui| {
                    ui.label(format!("已記住 {} 份 PDF 讀到的頁數", st.pdf_pages.len()));
                    if ui.add_enabled(!st.pdf_pages.is_empty(), egui::Button::new("清除")).clicked() {
                        st.pdf_pages.clear();
                    }
                });
                hint(ui, "快捷鍵：← → 或 PageUp / PageDown 翻頁，Home / End 第一頁 / 最後一頁，Ctrl+G 輸入頁碼。");
            }
            SettingsTab::Sidebar => {
                heading(ui, "檔案樹");
                ui.checkbox(&mut st.show_pdfs, "顯示 PDF 檔");
                ui.checkbox(&mut st.show_counts, "在資料夾與書本旁顯示檔案數量");
                heading(ui, "排序");
                let before = st.sort;
                ui.horizontal_wrapped(|ui| {
                    for (key, label) in [
                        (SortKey::Name, "名稱"),
                        (SortKey::Modified, "修改時間"),
                        (SortKey::Created, "建立時間"),
                        (SortKey::Custom, "自訂"),
                    ] {
                        ui.selectable_value(&mut st.sort.key, key, label);
                    }
                });
                ui.checkbox(&mut st.sort.reverse, "反向排列");
                ui.checkbox(&mut st.sort.folders_first, "資料夾排在前面（自訂排序時用於還沒排過的項目）");
                hint(ui, "在側欄拖曳檔案即可自訂順序（會自動切換為「自訂」），順序存在書庫的 .shosai/order.toml，會跟著 Nextcloud 同步。書本的章節順序則存在 book.toml。");
                let sort_changed = st.sort != before;
                if ui.button("全部收合").clicked() {
                    st.expanded.clear();
                }
                hint(ui, "顏色：資料夾為黃色、書本為橘色、Markdown 為藍色、PDF 為紅色。");
                if sort_changed {
                    self.resort();
                    self.refresh_novel();
                }
            }
            SettingsTab::Library => {
                heading(ui, "目前的書庫");
                match self.library.as_ref().map(|l| l.root.clone()) {
                    Some(root) => {
                        ui.add(
                            egui::Label::new(RichText::new(root.display().to_string()).monospace()).selectable(true),
                        );
                        ui.horizontal(|ui| {
                            if ui.button("在檔案管理員中開啟").clicked() {
                                self.actions.push(Action::Reveal(root.clone()));
                            }
                            if ui.button("開啟其他資料夾…").clicked() {
                                self.actions.push(Action::PickLibrary);
                            }
                        });
                    }
                    None => {
                        if ui.button("開啟資料夾…").clicked() {
                            self.actions.push(Action::PickLibrary);
                        }
                    }
                }
                heading(ui, "最近使用");
                let recent = self.settings.recent_libraries.clone();
                if recent.is_empty() {
                    hint(ui, "（無）");
                }
                for r in recent {
                    if ui.link(r.display().to_string()).clicked() {
                        self.actions.push(Action::OpenLibrary(r));
                    }
                }
                if !self.settings.recent_libraries.is_empty() && ui.button("清除紀錄").clicked() {
                    self.settings.recent_libraries.clear();
                }
                heading(ui, "Nextcloud 同步");
                #[cfg(feature = "nextcloud")]
                {
                    if let Some(root) = self.library.as_ref().map(|l| l.root.clone()) {
                        if ui.button("同步設定…").clicked() {
                            self.actions.push(Action::Dialog(nextcloud_dialog(&root)));
                            self.show_settings = false;
                        }
                    }
                }
                #[cfg(not(feature = "nextcloud"))]
                hint(ui, "此版本未啟用 Nextcloud 同步（編譯時加上 --features nextcloud）。");
            }
            SettingsTab::About => {
                ui.label(RichText::new("書齋 Shosai").size(20.0).strong());
                ui.label(format!("版本 {}", env!("CARGO_PKG_VERSION")));
                ui.label("輕量的 Markdown 寫作與閱讀器，可將資料夾整理成書、輸出 PDF，並直接閱讀 PDF。");
                hint(ui, "排版：Typst　數學式：MiTeX　PDF 檢視：hayro　介面：egui");
                if ui.button("鍵盤快捷鍵…").clicked() {
                    self.dialog = Some(Dialog::Shortcuts);
                    self.show_settings = false;
                }
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context, pal: &Palette) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let mut keep = true;
        let mut confirm = false;

        let modal = egui::Modal::new(egui::Id::unique("shosai-dialog")).show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.spacing_mut().item_spacing.y = 8.0;
            let enter = ui.input(|i| i.key_pressed(Key::Enter));
            let title = |ui: &mut egui::Ui, t: &str| {
                ui.label(RichText::new(t).size(17.0).strong());
                ui.add_space(4.0);
            };
            match &mut dialog {
                Dialog::NewNote { name, .. } => {
                    title(ui, "新增筆記");
                    focused_input(ui, name, "檔名");
                    confirm |= enter;
                }
                Dialog::NewFolder { name, .. } => {
                    title(ui, "新增資料夾");
                    focused_input(ui, name, "資料夾名稱");
                    confirm |= enter;
                }
                Dialog::NewBook { title: book_title, author, .. } => {
                    title(ui, "建立新書");
                    ui.label(RichText::new("會建立一個資料夾，裡面每個 Markdown 檔就是一章。").color(pal.weak));
                    focused_input(ui, book_title, "書名");
                    ui.add(egui::TextEdit::singleline(author).hint_text("作者（可留空）").desired_width(f32::INFINITY));
                }
                Dialog::NewChapter { title: t, .. } => {
                    title(ui, "新增章節");
                    focused_input(ui, t, "章節標題");
                    confirm |= enter;
                }
                Dialog::Rename { name, .. } => {
                    title(ui, "重新命名");
                    focused_input(ui, name, "新名稱");
                    confirm |= enter;
                }
                Dialog::Trash { path } => {
                    title(ui, "移到回收桶");
                    ui.label(format!("確定要移除「{}」？", path.file_name().unwrap_or_default().to_string_lossy()));
                    ui.label(
                        RichText::new(format!("檔案會移到書庫的 {} 資料夾，可以手動救回。", library::TRASH_DIR))
                            .color(pal.weak),
                    );
                }
                Dialog::BookSettings { meta, .. } => {
                    title(ui, "書本設定");
                    book_settings_ui(ui, meta);
                }
                Dialog::Nextcloud { server, user, password, remote_dir, insecure } => {
                    title(ui, "Nextcloud 同步設定");
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
                            .small()
                            .color(pal.weak),
                    );
                }
                Dialog::Report(report) => {
                    title(ui, "同步完成");
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        ui.label(RichText::new(report.as_str()).monospace());
                    });
                }
                Dialog::Shortcuts => {
                    title(ui, "鍵盤快捷鍵");
                    egui::Grid::new("keys").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                        for (k, v) in [
                            ("Ctrl+S", "儲存（也會自動儲存）"),
                            ("Ctrl+N", "新增筆記"),
                            ("Ctrl+1 / 2 / 3", "編輯 / 分割 / 閱讀"),
                            ("Ctrl+4", "書本模式（從這一章開始）"),
                            ("← → 空白鍵", "書本模式翻頁（直排時 ← 是下一頁）"),
                            ("Ctrl+PageDown / PageUp", "書本模式：下一章 / 回到章首、上一章"),
                            ("T / Esc", "書本模式：目錄 / 結束"),
                            ("Ctrl+B、Ctrl+\\", "顯示 / 隱藏側欄"),
                            ("Ctrl+B / I / K", "編輯時：粗體 / 斜體 / 連結"),
                            ("Ctrl+P", "搜尋檔案（↓ 選擇、Enter 開啟）"),
                            ("Ctrl+= / Ctrl+-、Ctrl+滾輪", "放大 / 縮小（PDF 為縮放）"),
                            ("← / →", "閱讀模式翻章；PDF 翻頁"),
                            ("↑ ↓ 空白鍵", "閱讀模式、PDF 捲動（Shift+空白鍵往上）"),
                            ("PageUp / PageDown", "閱讀模式捲一個畫面；PDF 上一頁 / 下一頁"),
                            ("Home / End", "閱讀模式最前 / 最後；PDF 第一頁 / 最後一頁"),
                            ("檔案樹：↑ ↓ ← →", "移動、收合 / 展開（先點一下資料夾）"),
                            ("檔案樹：Enter / F2 / Delete", "開啟 / 重新命名 / 移到回收桶"),
                            ("Ctrl+G", "PDF 輸入頁碼跳頁"),
                            ("Ctrl+,", "設定"),
                        ] {
                            ui.label(RichText::new(k).monospace());
                            ui.label(v);
                            ui.end_row();
                        }
                    });
                }
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (label, danger) = match &dialog {
                    Dialog::Trash { .. } => ("移除", true),
                    Dialog::Report(_) | Dialog::Shortcuts => ("關閉", false),
                    Dialog::BookSettings { .. } => ("儲存", false),
                    _ => ("確定", false),
                };
                let fill = if danger { ui.visuals().error_fg_color } else { pal.accent };
                let primary = egui::Button::new(RichText::new(label).color(Color32::WHITE))
                    .fill(fill)
                    .min_size(egui::vec2(72.0, 28.0));
                if ui.add(primary).clicked() {
                    confirm = true;
                }
                if !matches!(dialog, Dialog::Report(_) | Dialog::Shortcuts)
                    && ui.add(egui::Button::new("取消").min_size(egui::vec2(64.0, 28.0))).clicked()
                {
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
                let path = library::create_dir(&dir, if name.trim().is_empty() { "新資料夾" } else { &name })
                    .map_err(|e| e.to_string())?;
                self.settings.expanded.insert(path);
                self.refresh();
            }
            Dialog::NewBook { dir, title, author } => {
                let title =
                    if title.trim().is_empty() { "未命名的書".to_string() } else { title.trim().to_string() };
                let target = library::create_dir(&dir, &title).map_err(|e| e.to_string())?;
                let book = Book::create(&target, &title, author.trim()).map_err(|e| e.to_string())?;
                let first = book.chapters.first().map(|c| c.path.clone());
                self.settings.expanded.insert(target);
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
                self.save_current();
                let new = library::rename(&path, &name).map_err(|e| e.to_string())?;
                self.relocate(&path, &new);
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
                if self.pdf.as_ref().is_some_and(|p| p.path.starts_with(&path)) {
                    self.pdf = None;
                }
                if self.novel.as_ref().is_some_and(|n| n.book.root.starts_with(&path)) {
                    self.close_novel(false);
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
                self.info("同步設定已儲存，按側欄的雲朵圖示開始同步");
            }
            #[cfg(not(feature = "nextcloud"))]
            Dialog::Nextcloud { .. } => {}
            Dialog::Report(_) | Dialog::Shortcuts => {}
        }
        Ok(())
    }
}

impl eframe::App for ShosaiApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        self.shortcuts(&ctx);
        self.poll_export();
        if let Some(changed) = self.watcher.as_ref().and_then(|w| w.take(&ctx)) {
            self.on_fs_change(changed);
        }
        let pal = theme::palette(ui.visuals().dark_mode);

        egui::Panel::top("toolbar")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(pal.chrome).inner_margin(egui::Margin::symmetric(8, 6)))
            .show(ui, |ui| self.toolbar(ui, &pal));
        egui::Panel::bottom("status")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(pal.chrome).inner_margin(egui::Margin::symmetric(12, 3)))
            .show(ui, |ui| self.status_bar(ui, &pal));

        let mut sidebar_open = self.settings.sidebar;
        egui::Panel::left("sidebar")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(pal.chrome).inner_margin(egui::Margin {
                left: 10,
                right: 6,
                top: 4,
                bottom: 6,
            }))
            .resizable(true)
            .default_size(260.0)
            .size_range(180.0..=480.0)
            .show_collapsible(ui, &mut sidebar_open, |ui| self.sidebar(ui, &pal));
        self.settings.sidebar = sidebar_open;

        let surface = egui::Frame::new().fill(pal.surface);
        if self.novel.is_some() {
            let mut events = Vec::new();
            let prefs = self.settings.novel;
            egui::CentralPanel::no_frame().show(ui, |ui| {
                if let Some(n) = self.novel.as_mut() {
                    n.show(ui, &pal, &prefs, &mut events);
                }
            });
            if let Some(pos) = self.novel.as_ref().and_then(|n| n.position().map(|p| (n.book.root.clone(), p))) {
                if self.settings.novel_pos.get(&pos.0) != Some(&pos.1) {
                    self.settings.novel_pos.insert(pos.0, pos.1);
                }
            }
            self.handle_novel_events(events);
        } else if self.pdf.is_some() {
            egui::CentralPanel::no_frame().show(ui, |ui| {
                let keys = !self.tree_nav.active;
                if let Some(pdf) = self.pdf.as_mut() {
                    pdf.show(ui, &pal, keys);
                }
            });
        } else if self.doc.is_none() {
            egui::CentralPanel::no_frame().frame(surface).show(ui, |ui| self.welcome(ui, &pal));
        } else {
            match self.settings.view {
                ViewMode::Edit => {
                    egui::CentralPanel::no_frame().frame(surface).show(ui, |ui| self.editor(ui, true));
                }
                ViewMode::Split => {
                    let half = ui.available_width() / 2.0;
                    egui::Panel::right("preview")
                        .show_separator_line(false)
                        .frame(egui::Frame::new().fill(pal.paper))
                        .resizable(true)
                        .default_size(half)
                        .size_range(220.0..=f32::INFINITY)
                        .show(ui, |ui| self.preview(ui));
                    egui::CentralPanel::no_frame().frame(surface).show(ui, |ui| self.editor(ui, false));
                }
                ViewMode::Read => {
                    egui::CentralPanel::no_frame().show(ui, |ui| self.reader(ui, &pal));
                }
            }
        }

        self.settings_window(&ctx, &pal);
        self.dialogs(&ctx, &pal);
        // 記住 PDF 讀到哪一頁、縮圖列開關
        if let Some(pdf) = &self.pdf {
            if pdf.page_count() > 0 {
                self.settings.pdf_pages.insert(pdf.path.clone(), pdf.current);
            }
            self.settings.pdf_thumbs = pdf.show_thumbs;
        }
        self.handle_view_events(&ctx);
        self.process_actions(&ctx);
        self.poll_sync();

        // 自動儲存
        if let Some(doc) = &self.doc {
            if doc.dirty {
                let after = Duration::from_secs_f32(self.settings.autosave_secs.clamp(0.3, 30.0));
                if doc.last_edit.elapsed() >= after {
                    self.save_current();
                } else {
                    ctx.request_repaint_after(after);
                }
            }
        }
        if matches!(self.status, Some(Status::Info(..) | Status::Error(..))) {
            ctx.request_repaint_after(Duration::from_secs(1));
        }

        if let Some((since, frames)) = self.frame_stats.as_mut() {
            *frames += 1;
            if since.elapsed() >= Duration::from_secs(1) {
                let atlas = ctx.fonts(|f| f.font_image_size());
                let (count, bytes) = ctx
                    .tex_manager()
                    .read()
                    .allocated()
                    .fold((0, 0), |(n, b), (_, m)| (n + 1, b + m.size[0] * m.size[1] * m.bytes_per_pixel));
                eprintln!(
                    "frames/s: {frames}  font atlas: {}x{} ({:.1} MB)  textures: {count} ({:.1} MB)",
                    atlas[0],
                    atlas[1],
                    (atlas[0] * atlas[1] * 4) as f64 / 1048576.0,
                    bytes as f64 / 1048576.0
                );
                *since = Instant::now();
                *frames = 0;
            }
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
        let title = match (&self.doc, self.current_path()) {
            _ if self.novel.is_some() => {
                format!("{} — 書齋", self.novel.as_ref().map(|n| n.book.title.as_str()).unwrap_or(""))
            }
            (Some(d), _) => format!(
                "{}{} — 書齋",
                if d.dirty { "● " } else { "" },
                d.path.file_name().unwrap_or_default().to_string_lossy()
            ),
            (None, Some(p)) => format!("{} — 書齋", p.file_name().unwrap_or_default().to_string_lossy()),
            _ => "書齋".into(),
        };
        // 只有標題真的改變才送出：send_viewport_cmd 會順便要求重畫，
        // 每幀都送會變成「畫 → 要求重畫 → 再畫」的無限迴圈，閒置時也持續吃 CPU / GPU。
        if self.window_title != title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_current();
    }
}

// ---------------------------------------------------------------------------

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

/// 編輯器輸入框的 id（固定下來，才能在外面查它有沒有焦點、讀寫游標位置）。
fn editor_id(path: &Path) -> egui::Id {
    egui::Id::unique(("shosai-editor", path))
}

fn content_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// 外部版本另存的檔名：`筆記.md` → `筆記 (衝突 2025-11-04 093000).md`。
fn conflict_path(path: &Path, stamp: &str) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let name = match path.extension() {
        Some(ext) => format!("{stem} (衝突 {stamp}).{}", ext.to_string_lossy()),
        None => format!("{stem} (衝突 {stamp})"),
    };
    path.with_file_name(name)
}

/// 把編輯器目前的選取範圍用 Markdown 記號包起來（或拿掉），並更新選取範圍。
fn format_selection(ctx: &egui::Context, id: egui::Id, text: &mut String, open: &str, close: &str) -> bool {
    use egui::text::{CCursor, CCursorRange};
    let Some(mut state) = egui::TextEdit::load_state(ctx, id) else { return false };
    let Some(range) = state.cursor.char_range() else { return false };
    let [a, b] = range.sorted_cursors().map(|c| c.index.0);
    let (a, b) = wrap_selection(text, a, b, open, close);
    state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
    state.store(ctx, id);
    true
}

/// 用 `open`／`close` 包住第 `a`～`b` 個字元；已經包住的話就拿掉。回傳新的選取範圍（以字元計）。
fn wrap_selection(text: &mut String, a: usize, b: usize, open: &str, close: &str) -> (usize, usize) {
    let byte = |text: &str, ci: usize| text.char_indices().nth(ci).map_or(text.len(), |(i, _)| i);
    let (ba, bb) = (byte(text, a), byte(text, b));
    let ol = open.chars().count();
    if open == "[" {
        // 連結：游標放進小括號裡，接著貼上或輸入網址
        text.insert_str(bb, close);
        text.insert_str(ba, open);
        let at = b + ol + 2;
        return (at, at);
    }
    let (before, after) = (&text[..ba], &text[bb..]);
    // 判斷斜體時，不要把 `**粗體**` 的星號當成斜體的
    let only_bold = open == "*" && before.ends_with("**") && !before.ends_with("***");
    if before.ends_with(open) && after.starts_with(close) && !only_bold {
        text.replace_range(bb..bb + close.len(), "");
        text.replace_range(ba - open.len()..ba, "");
        (a - ol, b - ol)
    } else {
        text.insert_str(bb, close);
        text.insert_str(ba, open);
        (a + ol, b + ol)
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_and_unwrap() {
        let mut t = String::from("這是重點喔");
        assert_eq!(wrap_selection(&mut t, 2, 4, "**", "**"), (4, 6));
        assert_eq!(t, "這是**重點**喔");
        assert_eq!(wrap_selection(&mut t, 4, 6, "**", "**"), (2, 4));
        assert_eq!(t, "這是重點喔");
        // 沒有選取：插入一對記號，游標在中間
        assert_eq!(wrap_selection(&mut t, 5, 5, "*", "*"), (6, 6));
        assert_eq!(t, "這是重點喔**");
    }

    #[test]
    fn italic_inside_bold() {
        let mut t = String::from("**粗體**");
        assert_eq!(wrap_selection(&mut t, 2, 4, "*", "*"), (3, 5));
        assert_eq!(t, "***粗體***");
        assert_eq!(wrap_selection(&mut t, 3, 5, "*", "*"), (2, 4));
        assert_eq!(t, "**粗體**");
    }

    #[test]
    fn link() {
        let mut t = String::from("看書齋的說明");
        assert_eq!(wrap_selection(&mut t, 1, 3, "[", "]()"), (6, 6));
        assert_eq!(t, "看[書齋]()的說明");
    }

    #[test]
    fn conflict_names() {
        let p = conflict_path(Path::new("書/筆記.md"), "2025-11-04 093000");
        assert_eq!(p, Path::new("書/筆記 (衝突 2025-11-04 093000).md"));
        let p = conflict_path(Path::new("書/README"), "x");
        assert_eq!(p, Path::new("書/README (衝突 x)"));
    }
}
