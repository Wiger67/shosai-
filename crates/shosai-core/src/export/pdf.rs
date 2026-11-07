//! PDF 輸出。
//!
//! - 預設（`pdf` feature）：內嵌 Typst 排版引擎，不需安裝任何外部程式。
//! - 關閉 `pdf` feature 時：改呼叫系統上的 `typst` 指令（若有安裝）。

use super::typst::{render as render_typst, TypstOutput};
use super::Document;
use crate::Result;

/// 將文件排版成 PDF，回傳檔案內容。
pub fn render(doc: &Document) -> Result<Vec<u8>> {
    let out = render_typst(doc);
    compile(&out)
}

#[cfg(feature = "pdf")]
pub fn compile(out: &TypstOutput) -> Result<Vec<u8>> {
    embedded::compile(out)
}

#[cfg(not(feature = "pdf"))]
pub fn compile(out: &TypstOutput) -> Result<Vec<u8>> {
    external::compile(out)
}

/// 用 Typst 編譯一份原始碼，回傳第一頁的 SVG（給數學式預覽用）。
#[cfg(feature = "pdf")]
pub fn compile_svg(source: &str) -> Result<String> {
    embedded::compile_svg(source)
}

/// 預先在背景掃描系統字型，讓第一次輸出 PDF 時更快。
pub fn warm_up() {
    #[cfg(feature = "pdf")]
    std::thread::spawn(|| {
        let _ = embedded::fonts();
    });
}

#[cfg(feature = "pdf")]
mod embedded {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    use typst::diag::{FileError, FileResult, SourceDiagnostic};
    use typst::foundations::{Bytes, Datetime, Duration};
    use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
    use typst::text::{Font, FontBook};
    use typst::utils::LazyHash;
    use typst::{Library, LibraryExt, World};
    use typst_kit::fonts::FontStore;
    use typst_layout::PagedDocument;

    use super::TypstOutput;
    use crate::{Error, Result};

    /// New Computer Modern Math（GUST Font License），見 assets/fonts/README.md。
    static MATH_FONT: &[u8] = include_bytes!("../../assets/fonts/NewCMMath-Regular.otf");

    /// 字型只掃描一次，之後重複使用。
    pub fn fonts() -> &'static FontStore {
        static FONTS: OnceLock<FontStore> = OnceLock::new();
        FONTS.get_or_init(|| {
            let mut store = FontStore::new();
            // 數學字型固定內建，讓數學式在任何電腦上都一樣好看
            if let Some(font) = Font::new(Bytes::new(MATH_FONT), 0) {
                let info = font.info().clone();
                store.push((font, info));
            }
            #[cfg(feature = "bundled-fonts")]
            store.extend(typst_kit::fonts::embedded());
            // 系統字型改用記憶體映射：中文字型檔動輒 20MB，整個讀進記憶體太浪費
            store.extend(
                typst_kit::fonts::system().map(|(p, info)| (MappedFont { path: p.path, index: p.index }, info)),
            );
            store
        })
    }

    /// 以記憶體映射（mmap）載入的字型：只有實際用到的部分會讀進記憶體，
    /// 而且是可與其他程式共用的檔案頁面，不佔私有記憶體。
    struct MappedFont {
        path: PathBuf,
        index: u32,
    }

    impl typst_kit::fonts::FontSource for MappedFont {
        fn load(&self) -> Option<Font> {
            let file = std::fs::File::open(&self.path).ok()?;
            // SAFETY: 字型檔在程式執行期間被改寫的機率極低；萬一發生，最壞是該字型顯示錯誤。
            let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
            Font::new(Bytes::new(map), self.index)
        }
    }

    fn library() -> &'static LazyHash<Library> {
        static LIB: OnceLock<LazyHash<Library>> = OnceLock::new();
        LIB.get_or_init(|| LazyHash::new(Library::default()))
    }

    fn file_id(path: &str) -> Option<FileId> {
        Some(RootedPath::new(VirtualRoot::Project, VirtualPath::new(path).ok()?).intern())
    }

    struct ShosaiWorld {
        main: Source,
        files: HashMap<FileId, PathBuf>,
        /// 內建的虛擬檔案（MiTeX 數學式定義）
        memory: &'static HashMap<FileId, Bytes>,
        fonts: &'static FontStore,
    }

    fn memory_files() -> &'static HashMap<FileId, Bytes> {
        static FILES: OnceLock<HashMap<FileId, Bytes>> = OnceLock::new();
        FILES.get_or_init(|| {
            crate::math::MITEX_FILES
                .iter()
                .filter_map(|(path, text)| Some((file_id(path)?, Bytes::from_string(text.to_string()))))
                .collect()
        })
    }

    impl World for ShosaiWorld {
        fn library(&self) -> &LazyHash<Library> {
            library()
        }

        fn book(&self) -> &LazyHash<FontBook> {
            self.fonts.book()
        }

        fn main(&self) -> FileId {
            self.main.id()
        }

        fn source(&self, id: FileId) -> FileResult<Source> {
            if id == self.main.id() {
                Ok(self.main.clone())
            } else {
                let bytes = self.file(id)?;
                let text = String::from_utf8(bytes.to_vec()).map_err(|_| FileError::InvalidUtf8)?;
                Ok(Source::new(id, text))
            }
        }

        fn file(&self, id: FileId) -> FileResult<Bytes> {
            if id == self.main.id() {
                return Ok(Bytes::from_string(self.main.text().to_string()));
            }
            if let Some(bytes) = self.memory.get(&id) {
                return Ok(bytes.clone());
            }
            let path = self.files.get(&id).ok_or_else(|| FileError::NotFound(id.vpath().get_with_slash().into()))?;
            std::fs::read(path).map(Bytes::new).map_err(|e| FileError::from_io(e, path))
        }

        fn font(&self, index: usize) -> Option<Font> {
            self.fonts.font(index)
        }

        fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
            let (y, m, d) = crate::civil_from_days(secs.div_euclid(86_400));
            Datetime::from_ymd(y, m, d)
        }
    }

    fn world(source: String, images: &[(String, PathBuf)]) -> ShosaiWorld {
        let main_id = file_id("/__shosai_main.typ").expect("valid path");
        let files = images.iter().filter_map(|(vpath, real)| Some((file_id(vpath)?, real.clone()))).collect();
        ShosaiWorld { main: Source::new(main_id, source), files, memory: memory_files(), fonts: fonts() }
    }

    pub fn compile(out: &TypstOutput) -> Result<Vec<u8>> {
        let world = world(out.source.clone(), &out.images);
        let result = typst::compile::<PagedDocument>(&world);
        let document = result.output.map_err(|errs| Error::Export(format_errors(&world, &errs)))?;
        let pdf = typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default())
            .map_err(|errs| Error::Export(format_errors(&world, &errs)))?;
        typst::comemo::evict(10);
        Ok(pdf)
    }

    pub fn compile_svg(source: &str) -> Result<String> {
        let world = world(source.to_string(), &[]);
        let result = typst::compile::<PagedDocument>(&world);
        let document = result.output.map_err(|errs| Error::Export(format_errors(&world, &errs)))?;
        let page = document.pages().first().ok_or_else(|| Error::Export("沒有內容".into()))?;
        // 只保留最近幾次編譯的快取：足以加速同一份文件的數學式，又不會無限累積
        typst::comemo::evict(10);
        Ok(typst_svg::svg(page, &typst_svg::SvgOptions::default()))
    }

    fn format_errors(world: &ShosaiWorld, errs: &[SourceDiagnostic]) -> String {
        use typst::WorldExt;
        errs.iter()
            .take(5)
            .map(|e| {
                let line = world.range(e.span).and_then(|r| {
                    let text = world.main.text();
                    let ln = world.main.lines().byte_to_line(r.start)?;
                    Some(format!(" (第 {} 行：{})", ln + 1, text.lines().nth(ln).unwrap_or("").trim()))
                });
                format!("{}{}", e.message, line.unwrap_or_default())
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(not(feature = "pdf"))]
mod external {
    use super::TypstOutput;
    use crate::{Error, Result};

    /// 退路：在暫存資料夾中寫出 .typ 與圖片，呼叫 `typst compile`。
    pub fn compile(out: &TypstOutput) -> Result<Vec<u8>> {
        let dir = std::env::temp_dir().join(format!("shosai-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("__img"))?;
        for (vpath, real) in &out.images {
            std::fs::copy(real, dir.join(vpath.trim_start_matches('/')))?;
        }
        for (vpath, text) in crate::math::MITEX_FILES {
            let path = dir.join(vpath.trim_start_matches('/'));
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(path, text)?;
        }
        let main = dir.join("main.typ");
        let pdf = dir.join("main.pdf");
        std::fs::write(&main, &out.source)?;
        let status = std::process::Command::new("typst")
            .arg("compile")
            .arg("--root")
            .arg(&dir)
            .arg(&main)
            .arg(&pdf)
            .output()
            .map_err(|_| Error::Export("此版本未內建 PDF 引擎，且找不到 `typst` 指令".into()))?;
        if !status.status.success() {
            return Err(Error::Export(String::from_utf8_lossy(&status.stderr).into_owned()));
        }
        let bytes = std::fs::read(&pdf)?;
        let _ = std::fs::remove_dir_all(&dir);
        Ok(bytes)
    }
}
