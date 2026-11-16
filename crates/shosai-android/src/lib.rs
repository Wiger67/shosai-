//! 書齋的 Android 版：只有進入點，程式本體在 `shosai` 函式庫裡。
//!
//! 編成 `libshosai_android.so`，由系統內建的 `NativeActivity` 載入，所以不需要任何 Java 程式碼。
//! 打包方式見這個資料夾的 `README.md`。

#![cfg(target_os = "android")]

#[no_mangle]
fn android_main(app: shosai::android::AndroidApp) {
    shosai::android::main(app);
}
