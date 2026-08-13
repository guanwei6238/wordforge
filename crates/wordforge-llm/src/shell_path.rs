//! 取得使用者 shell 裡真正的 `PATH`。
//!
//! ## 為什麼需要這個
//!
//! 從應用程式選單啟動的 GUI 程式，拿到的是 session 的最小 `PATH`，
//! **不是**使用者在終端機裡看到的那個。差別來自 `~/.bashrc`、`~/.zshrc`
//! 這類檔案——它們只在互動式 shell 裡執行。
//!
//! nvm 正好是這樣裝的。實際踩到的情況：
//!
//! ```text
//! $ codex          # 終端機裡：正常
//! （從選單開啟 App）→ 供應商回傳錯誤（HTTP 127）：
//!                     /usr/bin/env: 'node': No such file or directory
//! ```
//!
//! `codex` 本身在 `~/.local/bin`（那個目錄 `~/.profile` 會加進去，所以找得到），
//! 但它的 shebang 是 `#!/usr/bin/env node`，而 `node` 在
//! `~/.nvm/versions/node/<版本>/bin` ——那是 `~/.bashrc` 裡的 nvm 初始化加的。
//! 於是指令找得到、直譯器找不到，退出碼 127。
//!
//! ## 為什麼是互動式 shell
//!
//! 實測這台機器：
//!
//! | 呼叫方式 | 找得到 node 嗎 |
//! | --- | --- |
//! | `bash -l -c`（登入、非互動） | ✗ |
//! | `bash -i -c`（互動） | ✓ |
//!
//! nvm 的初始化寫在 `~/.bashrc`，而登入非互動 shell 不讀它。所以要用 `-ilc`。
//!
//! 代價是 rc 檔會被完整執行一次（可能有幾百毫秒），所以整個程序只做一次。

use std::sync::OnceLock;
use std::time::Duration;

/// 把 `PATH` 夾在兩個標記之間輸出，避免被 rc 檔的雜訊污染。
///
/// 互動式 shell 會印各種東西（nvm 的警告、歡迎訊息、shell 提示字元），
/// 直接讀 stdout 會拿到一堆垃圾。用 ASCII 的 Unit Separator 當界線，
/// 那個字元不會出現在正常的路徑裡。
const MARKER: &str = "\u{1f}wordforge-path\u{1f}";

/// rc 檔壞掉或很慢時的上限。寧可用不到完整 PATH，也不能讓設定頁卡住。
const TIMEOUT: Duration = Duration::from_secs(5);

static RESOLVED: OnceLock<Option<String>> = OnceLock::new();

/// 使用者 shell 裡的 `PATH`。查一次就快取。
///
/// 回傳 `None` 表示查不到（Windows、沒有 `SHELL`、逾時、rc 檔壞掉），
/// 呼叫端應該退回現有的 `PATH`。
pub async fn user_path() -> Option<String> {
    if let Some(cached) = RESOLVED.get() {
        return cached.clone();
    }
    let found = query().await;
    // 競爭時第一個寫進去的贏，兩邊查到的東西一樣，無所謂
    let _ = RESOLVED.set(found.clone());
    found
}

#[cfg(windows)]
async fn query() -> Option<String> {
    // Windows 沒有「rc 檔只在互動 shell 執行」這個問題：
    // PATH 來自登錄檔，GUI 程式拿到的跟終端機一樣。
    None
}

#[cfg(not(windows))]
async fn query() -> Option<String> {
    let shell = std::env::var("SHELL").ok()?;
    if shell.trim().is_empty() {
        return None;
    }

    let script = format!("printf '%s%s%s' '{MARKER}' \"$PATH\" '{MARKER}'");

    let mut cmd = tokio::process::Command::new(&shell);
    // -i 是關鍵：nvm 之類的初始化只寫在 ~/.bashrc，
    // 而登入非互動 shell 不讀它
    cmd.args(["-ilc", &script])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    detach_from_terminal(&mut cmd);

    let output = tokio::time::timeout(TIMEOUT, cmd.output())
    .await
    .inspect_err(|_| tracing::debug!(%shell, "查 PATH 逾時"))
    .ok()?
    .ok()?;

    extract(&String::from_utf8_lossy(&output.stdout))
}

/// 把子行程放進自己的 session，跟控制終端機切斷。
///
/// ## 這一步不是保險，是必要的
///
/// **互動式 shell 會做 job control 初始化**：它比對自己的行程組和終端機的
/// 前景行程組，對不上就對**自己所在的行程組**送 `SIGTTIN` 把大家停住，
/// 等著被搬到前景。而它預設繼承我們的行程組——也就是整個 App 的行程組。
///
/// 從終端機啟動（`npm run tauri dev`）時，這件事的後果是：使用者一開設定頁，
/// `detect_backends` 走到這裡，然後整個 job 就被停住：
///
/// ```text
/// 287475 Tl+  wordforge-desktop            ← App 被停住
/// 289777 T+   /bin/bash -ilc printf ...    ← 兇手
/// [1]+  Stopped    npm run tauri dev
/// ```
///
/// 使用者看到的是視窗凍結，桌面環境接著跳出「強制退出」。
///
/// 打包後從桌面點開不會發生——那時候沒有控制終端機。所以這個 bug
/// 只在開發時看得到，而且看起來像 App 自己崩潰。
///
/// `setsid` 之後子行程沒有控制終端機，bash 直接關掉 job control
/// （它會往 stderr 抱怨一句，而我們本來就把 stderr 丟掉了），
/// rc 檔照樣讀，PATH 照樣查得到。
///
/// 只換行程組（`process_group`）不夠：那樣 App 不會被停，但 shell 自己
/// 仍然會卡在 `T` 直到逾時，每次開設定頁都白等五秒，還留下一個殭屍。
#[cfg(unix)]
fn detach_from_terminal(cmd: &mut tokio::process::Command) {
    // SAFETY: `pre_exec` 在 fork 之後、exec 之前執行，這中間只能呼叫
    // async-signal-safe 的東西。`setsid` 是其中之一，而且我們沒有配置
    // 記憶體、沒有碰鎖。失敗（已經是 session leader）不影響正確性。
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn detach_from_terminal(_cmd: &mut tokio::process::Command) {}

/// 從一堆雜訊裡撈出兩個標記之間的東西。
fn extract(stdout: &str) -> Option<String> {
    let start = stdout.find(MARKER)? + MARKER.len();
    let rest = &stdout[start..];
    let end = rest.find(MARKER)?;
    let path = rest[..end].trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// 把 shell 的 `PATH` 併進目前的 `PATH`。
///
/// shell 的排在前面：使用者自己裝的工具鏈（nvm、asdf、homebrew）
/// 應該優先於系統版本，那才是他在終端機裡跑到的那一個。
pub fn merge(shell_path: &str, current: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();

    for entry in shell_path.split(':').chain(current.split(':')) {
        if !entry.is_empty() && seen.insert(entry) {
            out.push(entry);
        }
    }
    out.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 這條測試存在的理由是它曾經是錯的：查 PATH 用的互動式 shell 繼承了
    /// App 的行程組，而互動式 bash 一啟動就做 job control 初始化——
    /// 發現自己不是終端機的前景行程組，就對整個行程組送 SIGTTIN。
    /// 從終端機啟動時，使用者一開設定頁整個 App 就被停住：
    ///
    /// ```text
    /// 287475 Tl+  wordforge-desktop
    /// 289777 T+   /bin/bash -ilc printf ...
    /// [1]+  Stopped    npm run tauri dev
    /// ```
    ///
    /// 這裡驗兩件事：子行程真的脫離了終端機（成為 session leader 而不是
    /// 停在 `T`），而且**PATH 仍然查得到**——脫鉤如果讓 rc 檔不再被讀，
    /// 那就是修掉一個 bug 換來另一個。
    ///
    /// 標 `#[ignore]` 是因為它會真的執行使用者的 shell 與 rc 檔，
    /// 結果取決於機器上的環境。用這個跑：
    ///
    /// ```bash
    /// cargo test -p wordforge-llm --lib -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "會執行使用者的 shell 與 rc 檔，結果依機器而異"]
    async fn the_lookup_shell_never_stops_the_app() {
        let Ok(shell) = std::env::var("SHELL") else {
            eprintln!("沒有 SHELL，跳過");
            return;
        };
        println!("shell = {shell}");

        let path = query().await;
        println!("查到的 PATH = {path:?}");
        assert!(
            path.is_some_and(|p| p.contains('/')),
            "脫離終端機之後 rc 檔沒被讀到，PATH 查不出來"
        );
    }

    /// 互動式 shell 會印一堆東西，PATH 要能從裡面撈出來。
    #[test]
    fn the_path_survives_noisy_rc_files() {
        let noisy = format!(
            "Run `nvm use --delete-prefix` to unset it.\n\
             歡迎回來！\n\
             {MARKER}/home/me/.nvm/versions/node/v24/bin:/usr/bin{MARKER}\n\
             $ "
        );
        assert_eq!(
            extract(&noisy).as_deref(),
            Some("/home/me/.nvm/versions/node/v24/bin:/usr/bin")
        );
    }

    #[test]
    fn missing_markers_mean_no_answer() {
        assert_eq!(extract("完全沒有標記"), None);
        assert_eq!(extract(&format!("{MARKER}只有一個")), None);
        assert_eq!(extract(&format!("{MARKER}{MARKER}")), None, "空的不算");
    }

    /// 使用者自己裝的工具鏈要排在系統版本前面——那才是他在終端機裡跑到的。
    #[test]
    fn the_users_toolchain_wins() {
        let merged = merge("/home/me/.nvm/bin:/usr/bin", "/usr/bin:/bin");
        assert_eq!(merged, "/home/me/.nvm/bin:/usr/bin:/bin");
    }

    #[test]
    fn merging_does_not_duplicate_entries() {
        let merged = merge("/a:/b:/a", "/b:/c");
        assert_eq!(merged, "/a:/b:/c");
    }

    #[test]
    fn empty_segments_are_dropped() {
        // PATH 裡的空段落代表「目前目錄」，那是個安全問題，不要傳下去
        assert_eq!(merge("/a::/b", ":/c"), "/a:/b:/c");
    }
}
