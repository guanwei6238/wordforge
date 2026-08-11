//! 用本機已安裝的 AI CLI 當作模型後端。
//!
//! ## 為什麼值得做
//!
//! 已經付了 Claude 或 ChatGPT 訂閱的人，不該為了同一個模型再開一份 API 帳單。
//! `claude -p` 與 `codex exec` 都能非交互執行、從 stdin 讀 prompt、把結果印到
//! stdout——這正好就是一個 LLM 後端需要的介面。
//!
//! ## 代價
//!
//! - **慢**：每次都要啟動一個行程，加上模型本身的時間，一題可能要幾十秒。
//! - **有速率限制**：訂閱的額度是給人用的，連續產生大量練習會撞到。
//! - **輸出比 API 髒**：CLI 會混進自己的警告與統計，靠
//!   [`crate::ChatResponse::json`] 從雜訊裡撈出最後一個完整物件。
//!
//! 所以這是「不想多付一份錢」的選項，不是效能最好的選項。

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::{ChatRequest, ChatResponse, LlmError, LlmProvider, Result, Role};

/// 預設等多久。一篇閱讀理解要模型想一陣子，訂閱版又比 API 慢。
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// 常見 CLI 的設定範本。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliPreset {
    /// Claude Code：`claude -p`
    ClaudeCode,
    /// OpenAI Codex：`codex exec`
    Codex,
    /// 自訂指令
    Custom,
}

/// 一個「執行外部指令當模型」的設定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CliConfig {
    pub preset: CliPreset,
    /// 執行檔名稱或完整路徑
    pub program: String,
    /// 固定參數
    pub args: Vec<String>,
    /// 傳遞 system prompt 用的參數名。`None` 就把 system 併進 prompt 開頭。
    pub system_flag: Option<String>,
    /// 指定模型的參數名（`--model`、`-m`）。`None` 表示這個 CLI 不支援。
    pub model_flag: Option<String>,
    /// 要用哪個模型。留空就用 CLI 自己的預設。
    ///
    /// 出題與批改不需要最強的模型：這些任務是「照著明確規格產生結構化輸出」，
    /// 中等模型就夠，而且快得多、也比較不會撞到訂閱的速率限制。
    pub model: String,
    pub timeout_secs: u64,
}

impl CliConfig {
    pub fn claude_code() -> Self {
        Self {
            preset: CliPreset::ClaudeCode,
            program: "claude".into(),
            // --output-format text：不要 JSON 包裝，我們自己的 prompt 已經要求輸出 JSON
            args: vec!["-p".into(), "--output-format".into(), "text".into()],
            system_flag: Some("--append-system-prompt".into()),
            model_flag: Some("--model".into()),
            // 出題不需要最強的模型，預設用中等的那個
            model: "sonnet".into(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }

    pub fn codex() -> Self {
        Self {
            preset: CliPreset::Codex,
            program: "codex".into(),
            // 資料目錄不是 git repo，不加這個 flag 會直接拒絕執行
            args: vec!["exec".into(), "--skip-git-repo-check".into()],
            // codex exec 沒有獨立的 system prompt 參數
            system_flag: None,
            model_flag: Some("-m".into()),
            // 留空用 codex 自己的預設，模型名稱因帳號方案而異
            model: String::new(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }

    pub fn preset(preset: CliPreset) -> Self {
        match preset {
            CliPreset::ClaudeCode => Self::claude_code(),
            CliPreset::Codex => Self::codex(),
            CliPreset::Custom => Self {
                preset: CliPreset::Custom,
                program: String::new(),
                args: Vec::new(),
                system_flag: None,
                model_flag: None,
                model: String::new(),
                timeout_secs: DEFAULT_TIMEOUT_SECS,
            },
        }
    }
}

pub struct CliLlm {
    config: CliConfig,
}

impl CliLlm {
    pub fn new(config: CliConfig) -> Result<Self> {
        if config.program.trim().is_empty() {
            return Err(LlmError::NotConfigured);
        }
        Ok(Self { config })
    }

    /// 組出完整的參數列表。
    fn build_args(&self, req: &ChatRequest) -> Vec<String> {
        let mut args = self.config.args.clone();

        if let Some(flag) = &self.config.model_flag
            && !self.config.model.trim().is_empty()
        {
            args.push(flag.clone());
            args.push(self.config.model.trim().to_string());
        }

        if let (Some(flag), Some(system)) = (&self.config.system_flag, &req.system) {
            args.push(flag.clone());
            args.push(system.clone());
        }
        args
    }

    /// 組出要從 stdin 餵進去的內容。
    ///
    /// 一律走 stdin 而不是命令列參數：閱讀理解的 prompt 有好幾 KB，
    /// 而且裡面有引號與換行，塞進參數只是自找麻煩。
    fn build_stdin(&self, req: &ChatRequest) -> String {
        let mut out = String::new();

        // CLI 沒有獨立的 system 參數時，把它當成 prompt 的開頭
        if self.config.system_flag.is_none()
            && let Some(system) = &req.system
        {
            out.push_str(system);
            out.push_str("\n\n");
        }

        for message in &req.messages {
            match message.role {
                Role::User => out.push_str(&message.content),
                // 預填的助理開頭（json_only 模式）對 CLI 沒有意義，跳過
                Role::Assistant => continue,
            }
            out.push('\n');
        }
        out
    }
}

#[async_trait]
impl LlmProvider for CliLlm {
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse> {
        let args = self.build_args(req);
        let input = self.build_stdin(req);

        let mut child = Command::new(&self.config.program)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    LlmError::Decode(format!(
                        "找不到指令 `{}`。請確認它有安裝而且在 PATH 裡。",
                        self.config.program
                    ))
                } else {
                    LlmError::Decode(e.to_string())
                }
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(input.as_bytes())
                .await
                .map_err(|e| LlmError::Decode(e.to_string()))?;
            // 一定要關掉 stdin，否則 CLI 會一直等更多輸入
            drop(stdin);
        }

        let output = tokio::time::timeout(
            Duration::from_secs(self.config.timeout_secs),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| {
            LlmError::Decode(format!(
                "`{}` 超過 {} 秒沒有回應",
                self.config.program, self.config.timeout_secs
            ))
        })?
        .map_err(|e| LlmError::Decode(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(LlmError::Api {
                status: output.status.code().unwrap_or(-1) as u16,
                // 錯誤通常在 stderr，但有些工具印在 stdout
                body: if stderr.trim().is_empty() {
                    stdout
                } else {
                    stderr.trim().to_string()
                },
            });
        }

        if stdout.trim().is_empty() {
            return Err(LlmError::Decode(format!(
                "`{}` 沒有輸出任何內容",
                self.config.program
            )));
        }

        Ok(ChatResponse {
            text: stdout,
            // CLI 不會回報 token 數，也不需要——用訂閱本來就不是按量計費
            input_tokens: None,
            output_tokens: None,
        })
    }

    fn model(&self) -> &str {
        // 沒指定模型時，記錄執行檔名稱總比記錄空字串有用
        if self.config.model.trim().is_empty() {
            &self.config.program
        } else {
            &self.config.model
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    fn req() -> ChatRequest {
        ChatRequest {
            system: Some("你是英文老師".into()),
            messages: vec![Message::user("出一題")],
            json_only: true,
        }
    }

    #[test]
    fn claude_gets_the_system_prompt_as_a_flag() {
        let cli = CliLlm::new(CliConfig::claude_code()).unwrap();
        let args = cli.build_args(&req());

        let pos = args
            .iter()
            .position(|a| a == "--append-system-prompt")
            .expect("缺少 system prompt 參數");
        assert_eq!(args[pos + 1], "你是英文老師");

        // system 已經用參數傳了，不該再重複塞進 stdin
        let stdin = cli.build_stdin(&req());
        assert!(!stdin.contains("你是英文老師"));
        assert!(stdin.contains("出一題"));
    }

    /// codex 沒有 system prompt 參數，只能併進 prompt 開頭。
    #[test]
    fn codex_folds_the_system_prompt_into_stdin() {
        let cli = CliLlm::new(CliConfig::codex()).unwrap();
        let stdin = cli.build_stdin(&req());

        assert!(stdin.starts_with("你是英文老師"));
        assert!(stdin.contains("出一題"));
        assert!(!cli.build_args(&req()).iter().any(|a| a.contains("system")));
    }

    /// 資料目錄不是 git repo，codex 不加這個參數會直接拒絕執行。
    #[test]
    fn codex_skips_the_git_repo_check() {
        let cli = CliLlm::new(CliConfig::codex()).unwrap();
        assert!(
            cli.build_args(&req())
                .iter()
                .any(|a| a == "--skip-git-repo-check")
        );
    }

    /// json_only 會塞一則預填的助理訊息，那是 API 的技巧，對 CLI 沒有意義。
    #[test]
    fn assistant_priming_is_not_sent_to_a_cli() {
        let cli = CliLlm::new(CliConfig::claude_code()).unwrap();
        let mut r = req();
        r.messages.push(Message::assistant("{"));

        let stdin = cli.build_stdin(&r);
        assert_eq!(stdin.trim(), "出一題", "預填的 `{{` 不該出現在輸入裡");
    }

    /// 出題不需要最強的模型，而且較小的模型快得多、也比較不會撞速率限制。
    #[test]
    fn the_model_is_actually_passed_to_the_cli() {
        let cli = CliLlm::new(CliConfig::claude_code()).unwrap();
        let args = cli.build_args(&req());

        let pos = args
            .iter()
            .position(|a| a == "--model")
            .expect("沒有指定模型");
        assert_eq!(args[pos + 1], "sonnet");
        assert_eq!(cli.model(), "sonnet");
    }

    /// 留空就用 CLI 自己的預設，不要傳一個空的 --model。
    #[test]
    fn an_empty_model_means_the_cli_default() {
        let mut cfg = CliConfig::claude_code();
        cfg.model = "  ".into();
        let cli = CliLlm::new(cfg).unwrap();

        assert!(!cli.build_args(&req()).iter().any(|a| a == "--model"));
        assert_eq!(cli.model(), "claude", "記錄用的名稱退回執行檔名");
    }

    #[test]
    fn codex_uses_its_own_model_flag() {
        let mut cfg = CliConfig::codex();
        cfg.model = "gpt-5".into();
        let cli = CliLlm::new(cfg).unwrap();

        let args = cli.build_args(&req());
        let pos = args.iter().position(|a| a == "-m").expect("沒有指定模型");
        assert_eq!(args[pos + 1], "gpt-5");
    }

    #[test]
    fn empty_program_is_rejected() {
        let mut cfg = CliConfig::claude_code();
        cfg.program = "  ".into();
        assert!(matches!(CliLlm::new(cfg), Err(LlmError::NotConfigured)));
    }

    /// 找不到指令時要說得夠清楚，讓使用者知道該去裝什麼。
    #[tokio::test]
    async fn a_missing_program_gives_an_actionable_error() {
        let mut cfg = CliConfig::claude_code();
        cfg.program = "wordforge-no-such-cli".into();
        let cli = CliLlm::new(cfg).unwrap();

        let err = cli.chat(&req()).await.unwrap_err();
        let message = err.to_string();
        assert!(message.contains("wordforge-no-such-cli"), "{message}");
        assert!(message.contains("PATH"), "要提示怎麼解決：{message}");
    }

    /// 用 `cat` 當假模型：驗證 stdin 真的被送進去、stdout 真的被讀回來。
    #[tokio::test]
    async fn stdin_and_stdout_are_wired_up() {
        let cli = CliLlm::new(CliConfig {
            preset: CliPreset::Custom,
            program: "cat".into(),
            args: vec![],
            system_flag: None,
            model_flag: None,
            model: "cat".into(),
            timeout_secs: 10,
        })
        .unwrap();

        let resp = cli
            .chat(&ChatRequest {
                system: None,
                messages: vec![Message::user(r#"{"echo": true}"#)],
                json_only: false,
            })
            .await
            .unwrap();

        assert_eq!(resp.json().unwrap()["echo"], true);
    }

    /// 指令失敗時要帶著 stderr 回報，不能只說「失敗了」。
    #[tokio::test]
    async fn a_failing_command_reports_stderr() {
        let cli = CliLlm::new(CliConfig {
            preset: CliPreset::Custom,
            program: "sh".into(),
            args: vec!["-c".into(), "echo 出事了 >&2; exit 3".into()],
            system_flag: None,
            model_flag: None,
            model: "failing".into(),
            timeout_secs: 10,
        })
        .unwrap();

        let err = cli.chat(&req()).await.unwrap_err();
        assert!(err.to_string().contains("出事了"), "{err}");
    }

    /// 卡住的指令要在時限內放棄，不能讓 UI 永遠轉圈。
    #[tokio::test]
    async fn a_hanging_command_times_out() {
        let cli = CliLlm::new(CliConfig {
            preset: CliPreset::Custom,
            program: "sleep".into(),
            args: vec!["60".into()],
            system_flag: None,
            model_flag: None,
            model: "sleepy".into(),
            timeout_secs: 1,
        })
        .unwrap();

        let err = cli.chat(&req()).await.unwrap_err();
        assert!(err.to_string().contains("沒有回應"), "{err}");
    }
}
