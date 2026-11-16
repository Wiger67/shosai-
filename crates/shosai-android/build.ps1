# 編出書齋的 Android APK：target\release\apk\shosai.apk
#
#   powershell -ExecutionPolicy Bypass -File crates\shosai-android\build.ps1
#
# 1. cargo apk 編 Rust 的部分（libshosai_android.so）並打包
# 2. 把 java\ 裡的 ShosaiActivity 編成 classes.dex 加進 APK（cargo apk 不會編 Java）
# 3. 重新對齊、簽章
#
# 需要：Android SDK（build-tools、platforms）、NDK、JDK、cargo-apk，見 README.md。

$ErrorActionPreference = 'Stop'

function Latest($dir) {
    # 依名稱裡的版本號排序（35.0.0、27.3.13750724、android-34 → 34）
    $item = Get-ChildItem $dir | Sort-Object {
        $v = [regex]::Match($_.Name, '\d+(\.\d+)*').Value
        if ($v -notmatch '\.') { $v += '.0' }
        [version]$v
    } | Select-Object -Last 1
    if (-not $item) { throw "找不到 $dir 裡的任何版本" }
    $item.FullName
}

# 外部程式失敗時停下來。外部程式都直接呼叫、不經過函式轉傳參數：
# PowerShell 5.1 轉傳時會把 `-name value` 黏成 `-name:value`
function Check($what) {
    if ($LASTEXITCODE -ne 0) { throw "$what 失敗（結束代碼 $LASTEXITCODE）" }
}

$root = (Resolve-Path "$PSScriptRoot\..\..").Path
if (-not $env:ANDROID_HOME) { $env:ANDROID_HOME = "$env:LOCALAPPDATA\Android\Sdk" }
if (-not $env:ANDROID_NDK_ROOT) { $env:ANDROID_NDK_ROOT = Latest "$env:ANDROID_HOME\ndk" }
$keystore = if ($env:CARGO_APK_RELEASE_KEYSTORE) { $env:CARGO_APK_RELEASE_KEYSTORE } else { "$env:USERPROFILE\.android\debug.keystore" }
$password = if ($env:CARGO_APK_RELEASE_KEYSTORE_PASSWORD) { $env:CARGO_APK_RELEASE_KEYSTORE_PASSWORD } else { 'android' }
if (-not (Test-Path $keystore)) { throw "找不到簽章金鑰 $keystore（產生方式見 README.md）" }
$env:CARGO_APK_RELEASE_KEYSTORE = $keystore
$env:CARGO_APK_RELEASE_KEYSTORE_PASSWORD = $password

$buildTools = Latest "$env:ANDROID_HOME\build-tools"
$androidJar = Join-Path (Latest "$env:ANDROID_HOME\platforms") 'android.jar'
$apkDir = "$root\target\release\apk"
$work = "$root\target\shosai-android-java"

# 1. Rust
Push-Location $root
try {
    cargo apk build -p shosai-android --release
    Check 'cargo apk build'
} finally {
    Pop-Location
}

# 2. Java → classes.dex → 加進 APK
# cargo apk 打出來的 APK 還沒有 ShosaiActivity，裝了會一開就結束：先移走，全部完成才放回成品
if (Test-Path $work) { Remove-Item -Recurse -Force $work }
New-Item -ItemType Directory "$work\classes", "$work\dex" | Out-Null
Move-Item -Force "$apkDir\shosai.apk" "$work\unsigned.apk"
$sources = (Get-ChildItem -Recurse "$PSScriptRoot\java" -Filter *.java).FullName
javac -encoding UTF-8 -source 8 -target 8 -bootclasspath $androidJar -d "$work\classes" $sources
Check 'javac'
$classes = (Get-ChildItem -Recurse "$work\classes" -Filter *.class).FullName
& "$buildTools\d8.bat" --release --min-api 26 --lib $androidJar --output "$work\dex" $classes
Check 'd8'

Push-Location "$work\dex"
try {
    # 在 dex 資料夾裡執行，classes.dex 才會放在 APK 的最上層
    & "$buildTools\aapt.exe" add "$work\unsigned.apk" classes.dex
    Check 'aapt add'
} finally {
    Pop-Location
}

# 3. 對齊、簽章
& "$buildTools\zipalign.exe" -f -p 4 "$work\unsigned.apk" "$work\aligned.apk"
Check 'zipalign'
& "$buildTools\apksigner.bat" sign --ks $keystore --ks-pass "pass:$password" --out "$apkDir\shosai.apk" "$work\aligned.apk"
Check 'apksigner sign'
& "$buildTools\apksigner.bat" verify "$apkDir\shosai.apk"
Check 'apksigner verify'
Write-Host "完成：$apkDir\shosai.apk"
