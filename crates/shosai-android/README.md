# 書齋 Android 版（實驗性）

把桌面版同一份程式編成 APK，在手機上用書本模式看小說。目前只做閱讀，沒有編輯與 PDF 功能。

## 需要的工具

- Rust 的 Android 目標：`rustup target add aarch64-linux-android`
- Android SDK（`build-tools`、一個 `platforms;android-NN`）與 NDK
- [cargo-apk](https://github.com/rust-mobile/cargo-apk)：`cargo install cargo-apk --locked`
- JDK（簽章用的 `keytool`、`apksigner` 需要）

## 編譯

在專案根目錄執行（PowerShell）：

```powershell
$env:ANDROID_HOME = "$env:LOCALAPPDATA\Android\Sdk"
$env:ANDROID_NDK_ROOT = "$env:ANDROID_HOME\ndk\27.3.13750724"
$env:CARGO_APK_RELEASE_KEYSTORE = "$env:USERPROFILE\.android\debug.keystore"
$env:CARGO_APK_RELEASE_KEYSTORE_PASSWORD = "android"
cargo apk build -p shosai-android --release
```

成品在 `target/release/apk/shosai.apk`，複製到手機上點開就能安裝（第一次要允許「安裝不明應用程式」）。

簽章用的是本機的偵錯金鑰（`~/.android/debug.keystore`）。**這個檔案要留著**：
換了金鑰之後新的 APK 蓋不過舊的，得先解除安裝，閱讀進度也會跟著不見。沒有的話用這行產生：

```powershell
keytool -genkey -v -keystore "$env:USERPROFILE\.android\debug.keystore" -storepass android -alias androiddebugkey -keypass android -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=Android Debug,O=Android,C=US"
```

## 小說放哪裡

書齋依序找這兩個地方：

1. **內部儲存空間最上層的 `Shosai` 資料夾**（`/storage/emulated/0/Shosai`）。裡面每個資料夾是一本書。
   要到「設定 → 應用程式 → 書齋」（或「特殊存取權 → 所有檔案存取權」）允許書齋存取所有檔案才讀得到。
2. 讀不到的話用 App 自己的資料夾 `Android/data/app.shosai/files`，不需要權限；第一次開啟會放一本範例書。

## 操作

- 首頁列出書庫裡的書，點一下用書本模式打開
- 點頁面左右兩側、或左右滑動翻頁；點中間開目錄（蓋滿整個畫面，選一章就收起來）
- 返回鍵：關目錄 → 回到首頁
- 工具列右邊的「Aa」可以調字級、行距、縮排、翻頁動畫
- 閱讀進度存在手機上，不會和電腦同步

## 不接手機也能檢查版面

`shosai` 的測試會用手機的畫面大小（360×780 點）把整個程式跑起來：

```bash
cargo test -p shosai phone_reads                 # 點書、翻頁、目錄、返回鍵
cargo test -p shosai mobile_shots -- --ignored   # 把幾個畫面存成 target/shots/mobile-*.png
```

這只檢查介面的排法與操作；視窗、字型、讀檔這些跟 Android 系統有關的部分，還是要實機才知道。

## 已知限制

- 畫面閃退時看不到錯誤訊息；程式自己抓得到的錯誤會寫進書庫旁的 `crash.txt`，下次開啟時顯示
- 沒有選擇資料夾的畫面，書庫位置是固定的
- 介面還是桌面版的排法，按鈕偏小
