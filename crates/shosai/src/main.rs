//! 書齋 Shosai 的桌面版執行檔；程式本體在函式庫（`lib.rs`）裡。

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    shosai::desktop_main();
}
