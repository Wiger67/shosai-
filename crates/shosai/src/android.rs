//! Android 版的進入點，以及手機上才需要的啟動流程。
//!
//! 手機版只拿來閱讀。使用者可以在書櫃上按「選擇書庫資料夾」自己選（`folder_picker.rs`）；
//! 還沒選過的時候，自動找這兩個地方——
//!
//! 1. `/storage/emulated/0/Shosai`（內部儲存空間最上層的 `Shosai` 資料夾）。
//!    要在系統設定裡允許書齋「所有檔案存取權」才讀得到。
//! 2. 讀不到的話，用 App 自己的資料夾（`Android/data/<套件名稱>/files`），
//!    不需要任何權限；第一次開啟時會放一本範例書進去。

use std::fs;
use std::path::{Path, PathBuf};

use eframe::egui;
use jni::objects::JObject;
use jni::{jni_sig, jni_str, Env, JavaVM};
pub use winit::platform::android::activity::AndroidApp;

use crate::app::{Device, Startup};

/// 不必選就會自動找到的書庫位置。
const SHARED_LIBRARY: &str = "/storage/emulated/0/Shosai";
/// 上次意外結束的原因；下次開啟時顯示出來（手機上看不到標準錯誤輸出）。
const CRASH_FILE: &str = "crash.txt";
const SAMPLE_DIR: &str = "範例書";

/// 第一次開啟時放進書庫的範例書。
const SAMPLE: &[(&str, &[u8])] = &[
    ("book.toml", include_bytes!("../../../examples/範例書/book.toml")),
    ("01-開始.md", include_bytes!("../../../examples/範例書/01-開始.md")),
    ("02-語法展示.md", include_bytes!("../../../examples/範例書/02-語法展示.md")),
    ("03-HackMD與數學式.md", include_bytes!("../../../examples/範例書/03-HackMD與數學式.md")),
    ("04-排版與輸出.md", include_bytes!("../../../examples/範例書/04-排版與輸出.md")),
    ("images/logo.svg", include_bytes!("../../../examples/範例書/images/logo.svg")),
];

/// 由 `crates/shosai-android` 的 `android_main` 呼叫。
pub fn main(app: AndroidApp) {
    crate::STARTED.get_or_init(std::time::Instant::now);
    let external = app.external_data_path();
    let internal = app.internal_data_path();
    if let Some(dir) = &external {
        let _ = fs::create_dir_all(dir);
    }

    // 閃退紀錄：放在用 USB 也拿得到的地方
    let crash = external.as_ref().or(internal.as_ref()).map(|d| d.join(CRASH_FILE));
    let report = crash.as_ref().and_then(|p| {
        let text = fs::read_to_string(p).ok()?;
        let _ = fs::remove_file(p);
        (!text.trim().is_empty()).then_some(text)
    });
    if let Some(path) = crash.clone() {
        std::panic::set_hook(Box::new(move |info| {
            let _ = fs::write(&path, format!("書齋上次意外結束：\n{info}\n"));
        }));
    }

    let (library, novel) = library(external.as_deref());
    // SAFETY: android-activity 給的是這個行程裡有效的 JavaVM 指標
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) };
    let device: Box<dyn Device> = Box::new(Android { app: app.clone(), vm });
    let startup = Startup { open: None, library, novel, report, device: Some(device), mobile: true };
    let storage = internal.map(|d| d.join("settings.ron"));
    if let Err(e) = crate::app::run_android(app, startup, storage) {
        if let Some(path) = crash {
            let _ = fs::write(path, format!("無法啟動畫面：{e}\n"));
        }
    }
}

/// 透過 JNI 使用 `ShosaiActivity`（`crates/shosai-android/java`）提供的功能。
///
/// APK 沒有包進那個 Java 類別時（例如直接用 `cargo apk build` 打包），
/// 呼叫會失敗：這時退回 NativeActivity 本身回報的內容範圍，權限則當成已經有了。
struct Android {
    app: AndroidApp,
    vm: JavaVM,
}

impl Android {
    fn with_activity<T>(&self, f: impl FnOnce(&mut Env, &JObject) -> jni::errors::Result<T>) -> Option<T> {
        self.vm
            .attach_current_thread(|env| {
                // SAFETY: activity_as_ptr 是 Activity 的全域參照，在 App 結束前都有效；
                // JObject 不會在 drop 時刪掉參照
                let activity = unsafe { JObject::from_raw(env, self.app.activity_as_ptr().cast()) };
                f(env, &activity)
            })
            .ok()
    }
}

impl Device for Android {
    fn safe_insets(&self, screen: egui::Vec2) -> [f32; 4] {
        let insets = self.with_activity(|env, activity| {
            let mut field = |name| env.get_field(activity, name, jni_sig!(jint))?.i();
            Ok([
                field(jni_str!("safeLeft"))?,
                field(jni_str!("safeTop"))?,
                field(jni_str!("safeRight"))?,
                field(jni_str!("safeBottom"))?,
            ])
        });
        if let Some(insets) = insets {
            return insets.map(|v| v as f32);
        }
        let r = self.app.content_rect();
        if r.right <= r.left || r.bottom <= r.top {
            return [0.0; 4]; // 系統還沒回報
        }
        [r.left as f32, r.top as f32, screen.x - r.right as f32, screen.y - r.bottom as f32]
    }

    fn has_all_files_access(&self) -> bool {
        self.with_activity(|env, activity| {
            env.call_method(activity, jni_str!("hasAllFilesAccess"), jni_sig!(() -> jboolean), &[])?.z()
        })
        .unwrap_or(true)
    }

    fn request_all_files_access(&self) {
        let _ = self.with_activity(|env, activity| {
            env.call_method(activity, jni_str!("requestAllFilesAccess"), jni_sig!(() -> void), &[])?;
            Ok(())
        });
    }
}

/// 使用者沒有自己選書庫時用哪裡。回傳（書庫，第一次開啟時直接用書本模式打開的資料夾）。
fn library(external: Option<&Path>) -> (Option<PathBuf>, Option<PathBuf>) {
    let shared = Path::new(SHARED_LIBRARY);
    // 沒有權限時資料夾可能「開得起來但裡面是空的」，所以要真的看到東西才算
    if fs::read_dir(shared).is_ok_and(|mut d| d.next().is_some()) {
        return (Some(shared.to_path_buf()), None);
    }
    let Some(root) = external else { return (None, None) };
    let sample = root.join(SAMPLE_DIR);
    let has_books = fs::read_dir(root).is_ok_and(|d| d.flatten().any(|e| e.path().is_dir()));
    if has_books {
        return (Some(root.to_path_buf()), None);
    }
    for (name, bytes) in SAMPLE {
        let path = sample.join(name);
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(path, bytes);
    }
    (Some(root.to_path_buf()), Some(sample))
}
