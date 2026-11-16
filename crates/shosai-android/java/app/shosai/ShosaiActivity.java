package app.shosai;

import android.app.NativeActivity;
import android.content.Intent;
import android.graphics.Insets;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.provider.Settings;
import android.view.DisplayCutout;
import android.view.View;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;

/**
 * 書齋的 Android 入口：系統內建的 NativeActivity 再加上三件只有 Java 做得到的事。
 *
 * <ul>
 *   <li>全螢幕：隱藏狀態列與導覽列（從邊緣滑動會暫時出現），畫面延伸到前鏡頭挖孔旁邊</li>
 *   <li>記下挖孔佔掉的邊緣（{@link #safeTop} 等），讓 Rust 那邊把按鈕排在它以外</li>
 *   <li>查詢、要求「所有檔案存取權」（讀 Nextcloud 等其他 App 的檔案需要）</li>
 * </ul>
 *
 * 程式本體在 Rust（libshosai_android.so）；這裡的欄位與方法由 Rust 透過 JNI 讀取、呼叫。
 */
public class ShosaiActivity extends NativeActivity implements View.OnApplyWindowInsetsListener {
    /** 四邊不能放東西的寬度（實體像素）：前鏡頭挖孔，以及沒隱藏起來的系統列。 */
    public volatile int safeLeft, safeTop, safeRight, safeBottom;

    // 下面這些都只是讓畫面更好看：任何一步出錯都吞掉，退回一般的 NativeActivity。
    // 不能讓書齋因此開不起來（這裡出錯時 Rust 那邊還沒啟動，連閃退紀錄都寫不了）

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        try {
            Window window = getWindow();
            if (Build.VERSION.SDK_INT >= 28) {
                // 畫面延伸到挖孔旁邊，不要在上面留一條黑邊
                WindowManager.LayoutParams lp = window.getAttributes();
                lp.layoutInDisplayCutoutMode = Build.VERSION.SDK_INT >= 30
                        ? WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS
                        : WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
                window.setAttributes(lp);
            }
            window.getDecorView().setOnApplyWindowInsetsListener(this);
        } catch (Throwable ignored) {
        }
        immersive();
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        // 從設定頁、其他 App 回來時系統列會重新出現，再藏一次
        if (hasFocus) {
            immersive();
        }
    }

    private void immersive() {
        try {
            hideSystemBars();
        } catch (Throwable ignored) {
        }
    }

    private void hideSystemBars() {
        Window window = getWindow();
        if (Build.VERSION.SDK_INT >= 30) {
            window.setDecorFitsSystemWindows(false);
            WindowInsetsController controller = window.getInsetsController();
            if (controller != null) {
                controller.hide(WindowInsets.Type.systemBars());
                controller.setSystemBarsBehavior(WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
            }
        } else {
            window.getDecorView().setSystemUiVisibility(View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                    | View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                    | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                    | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                    | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                    | View.SYSTEM_UI_FLAG_FULLSCREEN);
        }
    }

    @Override
    public WindowInsets onApplyWindowInsets(View view, WindowInsets insets) {
        try {
            recordSafeInsets(insets);
        } catch (Throwable ignored) {
        }
        return view.onApplyWindowInsets(insets);
    }

    private void recordSafeInsets(WindowInsets insets) {
        int left = 0, top = 0, right = 0, bottom = 0;
        if (Build.VERSION.SDK_INT >= 30) {
            // 藏起來的系統列不算（從邊緣滑出來時是蓋在畫面上，不會擠開內容）
            Insets safe = insets.getInsets(WindowInsets.Type.displayCutout() | WindowInsets.Type.systemBars());
            left = safe.left;
            top = safe.top;
            right = safe.right;
            bottom = safe.bottom;
        } else if (Build.VERSION.SDK_INT >= 28) {
            DisplayCutout cutout = insets.getDisplayCutout();
            if (cutout != null) {
                left = cutout.getSafeInsetLeft();
                top = cutout.getSafeInsetTop();
                right = cutout.getSafeInsetRight();
                bottom = cutout.getSafeInsetBottom();
            }
        }
        safeLeft = left;
        safeTop = top;
        safeRight = right;
        safeBottom = bottom;
    }

    /** 是否能讀其他 App（例如 Nextcloud）放在手機裡的檔案。 */
    public boolean hasAllFilesAccess() {
        try {
            return Build.VERSION.SDK_INT < 30 || Environment.isExternalStorageManager();
        } catch (Throwable t) {
            return true;
        }
    }

    /** 打開系統設定裡「所有檔案存取權」的頁面，讓使用者允許書齋。 */
    public void requestAllFilesAccess() {
        if (Build.VERSION.SDK_INT < 30) {
            return;
        }
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                try {
                    Intent intent = new Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                            Uri.parse("package:" + getPackageName()));
                    startActivity(intent);
                } catch (Throwable e) {
                    // 有些手機沒有「單一 App」的頁面：改開全部 App 的列表
                    try {
                        startActivity(new Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION));
                    } catch (Throwable ignored) {
                    }
                }
            }
        });
    }
}
