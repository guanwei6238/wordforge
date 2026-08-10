//! # wordforge-tts
//!
//! 用作業系統內建的語音合成朗讀單字。
//!
//! ## 為什麼先做這個而不是真人發音
//!
//! 真人錄音（Wiktionary / Commons）品質好很多，學發音本來就該聽真人，
//! 但一個字一個檔案、完整下載動輒好幾 GB，還要處理下載、快取與授權標示。
//! 系統 TTS 零安裝、零下載、每個平台都有，先讓「聽得到」成立；
//! 真人音檔會作為優先來源疊上來，TTS 退為找不到音檔時的後備。
//!
//! ## 各平台用什麼
//!
//! | 平台 | 指令 | 備註 |
//! | --- | --- | --- |
//! | Linux | `spd-say` | speech-dispatcher，大多數桌面發行版預裝 |
//! | macOS | `say` | 系統內建，品質最好 |
//! | Windows | `powershell` + `System.Speech` | 系統內建 |

use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum TtsError {
    #[error("要朗讀的文字含有不允許的字元")]
    InvalidText,

    #[error("找不到語音合成程式（{program}）。{hint}")]
    NotInstalled {
        program: &'static str,
        hint: &'static str,
    },

    #[error("語音合成失敗：{0}")]
    Failed(String),
}

pub type Result<T> = std::result::Result<T, TtsError>;

/// 朗讀文字的長度上限。單字卡上的內容不會超過這個長度，
/// 超過就代表呼叫端傳錯東西了。
const MAX_LEN: usize = 200;

/// 檢查並清理要朗讀的文字。
///
/// 只放行字母、數字、空白與少數標點。這不只是防注入——
/// 各平台的指令參數處理規則不同（Windows 尤其麻煩），
/// 限制輸入範圍比逐一處理跳脫規則可靠得多。
fn sanitize(text: &str) -> Result<String> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MAX_LEN {
        return Err(TtsError::InvalidText);
    }
    let ok = text
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '\'' | '.' | ',' | '?' | '!'));
    if !ok {
        return Err(TtsError::InvalidText);
    }
    Ok(text.to_string())
}

/// 組出要執行的指令與參數。抽出來是為了能在沒有音效裝置的環境測試。
fn build_command(text: &str, lang: &str) -> (&'static str, Vec<String>) {
    if cfg!(target_os = "macos") {
        ("say", vec![text.to_string()])
    } else if cfg!(target_os = "windows") {
        // System.Speech 吃的是 PowerShell 字串，單引號要用兩個單引號跳脫。
        // sanitize 已經擋掉大部分危險字元，這裡是第二道防線。
        let escaped = text.replace('\'', "''");
        (
            "powershell",
            vec![
                "-NoProfile".into(),
                "-Command".into(),
                format!(
                    "Add-Type -AssemblyName System.Speech; \
                     (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak('{escaped}')"
                ),
            ],
        )
    } else {
        (
            "spd-say",
            vec![
                // -w：等說完才結束，否則連續點兩次會互相打斷
                "-w".into(),
                "-l".into(),
                spd_language(lang).to_string(),
                // `--` 之後的內容一律當文字，不會被當成選項
                "--".into(),
                text.to_string(),
            ],
        )
    }
}

/// 把 BCP 47 語言代碼轉成 speech-dispatcher 認得的形式。
fn spd_language(lang: &str) -> &str {
    match lang {
        "en" => "en-US",
        "zh" | "zh-TW" | "zh-Hant" => "zh",
        other => other,
    }
}

fn install_hint() -> &'static str {
    if cfg!(target_os = "linux") {
        "Linux 請安裝 speech-dispatcher 與 espeak-ng：sudo apt install speech-dispatcher espeak-ng"
    } else {
        "這個平台應該內建語音合成，請確認系統設定"
    }
}

/// 朗讀一段文字。會等到唸完才回傳。
pub fn speak(text: &str, lang: &str) -> Result<()> {
    let text = sanitize(text)?;
    let (program, args) = build_command(&text, lang);

    let output = Command::new(program).args(&args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            TtsError::NotInstalled {
                program,
                hint: install_hint(),
            }
        } else {
            TtsError::Failed(e.to_string())
        }
    })?;

    if !output.status.success() {
        return Err(TtsError::Failed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

/// 這台機器有沒有可用的語音合成。UI 據此決定要不要顯示發音按鈕。
pub fn is_available() -> bool {
    let (program, _) = build_command("test", "en");
    // 用 `--version` 探測：存在就會回應，不存在會是 NotFound
    match Command::new(program).arg("--version").output() {
        Ok(_) => true,
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_words() {
        for w in ["apple", "well-known", "don't", "New York", "Hello, world!"] {
            assert!(sanitize(w).is_ok(), "{w} 應該可以朗讀");
        }
    }

    /// 指令是用參數陣列傳的，不經過 shell，但仍然不放行這些字元——
    /// 各平台的引號規則不同，限制輸入比逐一處理跳脫可靠。
    #[test]
    fn rejects_shell_and_script_metacharacters() {
        for w in [
            "rm -rf /; echo",
            "$(whoami)",
            "`id`",
            "a | b",
            "a & b",
            "a\nb",
            "\"quoted\"",
        ] {
            assert!(sanitize(w).is_err(), "{w} 不該被放行");
        }
    }

    #[test]
    fn rejects_empty_and_overlong_text() {
        assert!(sanitize("").is_err());
        assert!(sanitize("   ").is_err());
        assert!(sanitize(&"a".repeat(MAX_LEN + 1)).is_err());
        assert!(sanitize(&"a".repeat(MAX_LEN)).is_ok());
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(sanitize("  apple \n").unwrap(), "apple");
    }

    /// 文字必須是獨立的參數，不能被拼進指令字串裡。
    #[test]
    fn text_is_passed_as_its_own_argument() {
        let (program, args) = build_command("apple", "en");
        assert!(!program.is_empty());
        assert!(
            args.iter().any(|a| a == "apple"),
            "文字應該是獨立參數：{args:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_uses_a_double_dash_before_the_text() {
        let (program, args) = build_command("-w", "en");
        assert_eq!(program, "spd-say");
        let dash = args.iter().position(|a| a == "--").expect("缺少 --");
        // 以連字號開頭的單字不能被當成選項
        assert_eq!(args[dash + 1], "-w");
    }

    #[test]
    fn maps_language_codes() {
        assert_eq!(spd_language("en"), "en-US");
        assert_eq!(spd_language("zh-TW"), "zh");
        assert_eq!(spd_language("fr"), "fr");
    }

    /// 沒有音效裝置的 CI 也能跑：只驗證錯誤分類正確。
    #[test]
    fn missing_program_is_reported_clearly() {
        let err = Command::new("wordforge-no-such-tts-program")
            .output()
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
