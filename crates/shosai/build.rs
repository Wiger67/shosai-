//! 把圖示與版本資訊嵌入 Windows 執行檔（檔案總管、工作列會顯示這個圖示）。

fn main() {
    println!("cargo:rerun-if-changed=assets/shosai.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/shosai.ico")
        .set("ProductName", "書齋 Shosai")
        .set("FileDescription", "書齋 Shosai — 輕量的 Markdown 寫作與閱讀器")
        .set("LegalCopyright", "MIT License");
    if let Err(e) = res.compile() {
        // 沒有資源編譯器（rc.exe / windres）時仍可編譯，只是 exe 沒有圖示
        println!("cargo:warning=無法嵌入 Windows 圖示：{e}");
    }
}
