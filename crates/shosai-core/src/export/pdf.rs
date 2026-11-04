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

    /// 字型只掃描一次，之後重複使用。
    pub fn fonts() -> &'static FontStore {
        static FONTS: OnceLock<FontStore> = OnceLock::new();
        FONTS.get_or_init(|| {
            let mut store = FontStore::new();
            #[cfg(feature = "bundled-fonts")]
            store.extend(typst_kit::fonts::embedded());
            store.extend(typst_kit::fonts::system());
            store
        })
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
        fonts: &'static FontStore,
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

    pub fn compile(out: &TypstOutput) -> Result<Vec<u8>> {
        let main_id = file_id("/__shosai_main.typ").expect("valid path");
        let files = out.images.iter().filter_map(|(vpath, real)| Some((file_id(vpath)?, real.clone()))).collect();
        let world = ShosaiWorld { main: Source::new(main_id, out.source.clone()), files, fonts: fonts() };

        let result = typst::compile::<PagedDocument>(&world);
        let document = result.output.map_err(|errs| Error::Export(format_errors(&world, &errs)))?;
        let pdf = typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default())
            .map_err(|errs| Error::Export(format_errors(&world, &errs)))?;
        Ok(pdf)
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
