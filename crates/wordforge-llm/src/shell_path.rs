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

    let output = tokio::time::timeout(
        TIMEOUT,
        tokio::process::Command::new(&shell)
            // -i 是關鍵：nvm 之類的初始化只寫在 ~/.bashrc，
            // 而登入非互動 shell 不讀它
            .args(["-ilc", &script])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await
    .inspect_err(|_| tracing::debug!(%shell, "查 PATH 逾時"))
    .ok()?
    .ok()?;

    extract(&String::from_utf8_lossy(&output.stdout))
}

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
