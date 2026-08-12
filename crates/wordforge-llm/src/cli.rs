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

use crate::shell_path;
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
    /// 推理強度怎麼傳。不同 CLI 的形狀不一樣，見 [`EffortStyle`]。
    #[serde(default)]
    pub effort_style: EffortStyle,
    /// 推理強度。留空就用 CLI 自己的預設。
    ///
    /// 這是比換模型更划算的旋鈕：出題是照規格產出結構化內容，
    /// 不太需要深度推理，但預設值常常是高的。調低通常能省下大半時間。
    #[serde(default)]
    pub effort: String,
    pub timeout_secs: u64,
}

/// 推理強度要用什麼形狀傳給 CLI。
///
/// 兩個 CLI 的做法不一樣，不能用同一個「旗標 + 值」的模型硬套：
///
/// ```text
/// claude   --effort high                        獨立旗標
/// codex    -c model_reasoning_effort=high       設定覆寫
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum EffortStyle {
    /// 這個 CLI 不支援調整推理強度
    #[default]
    Unsupported,
    /// `<旗標> <值>`
    Flag(String),
    /// `-c <鍵>=<值>`
    Config { flag: String, key: String },
}

impl EffortStyle {
    /// 組出要附加的參數。強度留空或不支援時回空陣列。
    fn args(&self, effort: &str) -> Vec<String> {
        let effort = effort.trim();
        if effort.is_empty() {
            return Vec::new();
        }
        match self {
            EffortStyle::Unsupported => Vec::new(),
            EffortStyle::Flag(flag) => vec![flag.clone(), effort.to_string()],
            EffortStyle::Config { flag, key } => vec![flag.clone(), format!("{key}={effort}")],
        }
    }
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
            effort_style: EffortStyle::Flag("--effort".into()),
            // 出題是照規格產出結構化內容，中等就夠
            effort: "medium".into(),
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
            model: "gpt-5.6-luna".into(),
            // codex 沒有獨立的 effort 旗標，要走設定覆寫
            effort_style: EffortStyle::Config {
                flag: "-c".into(),
                key: "model_reasoning_effort".into(),
            },
            effort: "high".into(),
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
                effort_style: EffortStyle::Unsupported,
                effort: String::new(),
                timeout_secs: DEFAULT_TIMEOUT_SECS,
            },
        }
    }
}

/// 設定頁要顯示的選項。
///
/// 純文字輸入框對使用者不友善——他不會知道 `gpt-5.6-luna` 或 `xhigh`
/// 是不是有效的值，打錯了也要等到出題失敗才知道。
///
/// 但**清單一定會過期**：模型名稱常換，而且因帳號方案而異。所以這裡
/// 給的是「已知可用的選項」，UI 必須同時允許自己輸入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliOptions {
    pub preset: CliPreset,
    /// 建議的模型。第一個是預設值。
    pub models: Vec<String>,
    /// 支援的推理強度。空的代表這個 CLI 不支援調整。
    pub efforts: Vec<String>,
}

/// 試跑一個模型，回報它在這台機器上能不能用。
///
/// ## 為什麼是試跑而不是列清單
///
/// 兩個 CLI 都**沒有**可以程式化取得模型清單的方式：
///
/// ```text
/// codex models              不是子指令，會落回互動模式
/// codex completion bash     產出的補全腳本裡沒有模型名
/// claude models             被當成 prompt，開起互動 session
/// ```
///
/// 而且就算列得出來，「帳號方案支不支援」「CLI 版本夠不夠新」還是另一回事
/// ——`gpt-5.6-luna` 在 codex-cli 0.142.5 上會被拒絕，但它確實是個真模型。
///
/// 直接送一個最小 prompt 過去，成敗就是答案。代價是幾秒鐘與一點點額度，
/// 換來的是不會過期的正確答案。
pub async fn probe_model(mut config: CliConfig, model: &str) -> ModelProbe {
    config.model = model.trim().to_string();
    // 試跑不需要等一篇文章那麼久
    config.timeout_secs = config.timeout_secs.min(90);

    let llm = match CliLlm::new(config) {
        Ok(llm) => llm,
        Err(e) => {
            return ModelProbe {
                usable: false,
                detail: e.to_string(),
            };
        }
    };

    let req = ChatRequest {
        system: None,
        messages: vec![crate::Message::user("只輸出這個 JSON：{\"ok\":1}")],
        json_only: false,
    };

    match llm.chat(&req).await {
        Ok(_) => ModelProbe {
            usable: true,
            detail: String::new(),
        },
        Err(e) => ModelProbe {
            usable: false,
            detail: model_error_hint(&e.to_string()),
        },
    }
}

/// 試跑一個模型的結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelProbe {
    pub usable: bool,
    /// 不能用的原因。能翻成人話的就翻，翻不了的原樣附上。
    pub detail: String,
}

/// 把模型相關的錯誤翻成使用者知道下一步該做什麼的話。
fn model_error_hint(raw: &str) -> String {
    if raw.contains("requires a newer version") {
        return format!(
            "這個模型需要更新版的 CLI。跑 `codex update` 之後再試。\n\n原始訊息：{raw}"
        );
    }
    if raw.contains("model_not_found") || raw.contains("Unknown model") {
        return format!("找不到這個模型，可能是名稱打錯或你的方案沒有。\n\n原始訊息：{raw}");
    }
    raw.to_string()
}

/// 各 CLI 的模型與推理強度選項。
///
/// 值取自各 CLI 的 `--help`，不是猜的：
///
/// ```text
/// claude --help
///   --model <model>   別名 'fable' / 'opus' / 'sonnet'，或完整名稱
///   --effort <level>  low, medium, high, xhigh, max
///
/// codex exec --help
///   -m, --model <MODEL>
///   （沒有 effort 旗標，走 -c model_reasoning_effort=）
/// ```
///
/// Claude 這邊刻意用**別名**而不是完整型號：別名永遠指向該級別的最新模型，
/// 完整型號會在下一次改版時失效，而這份清單沒有人會記得回來更新。
///
/// 這份清單**一定會過期**，而且沒辦法自動更新——兩個 CLI 都不提供可以
/// 程式化查詢的模型清單（細節見 [`probe_model`]）。所以 UI 必須允許
/// 自己輸入，並且提供「試跑看看」來取得不會過期的答案。
pub fn cli_options(preset: CliPreset) -> CliOptions {
    let (models, efforts): (&[&str], &[&str]) = match preset {
        CliPreset::ClaudeCode => (
            &["sonnet", "opus", "haiku", "fable"],
            &["low", "medium", "high", "xhigh", "max"],
        ),
        // codex 沒有公開的模型清單。新型號需要夠新的 CLI——
        // `gpt-5.6-luna` 在 0.142.5 上會回
        // "requires a newer version of Codex"（0.147.0 可以），
        // 所以選了不能用要先 `codex update`。設定頁的「試跑」會直接講。
        CliPreset::Codex => (
            &["gpt-5.6-luna", "gpt-5.5", "gpt-5"],
            &["low", "medium", "high"],
        ),
        CliPreset::Custom => (&[], &[]),
    };

    CliOptions {
        preset,
        models: models.iter().map(|s| s.to_string()).collect(),
        efforts: efforts.iter().map(|s| s.to_string()).collect(),
    }
}

/// 一個 CLI 後端在這台機器上的狀況。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliAvailability {
    pub preset: CliPreset,
    pub label: String,
    pub program: String,
    pub installed: bool,
    /// 裝了的話是哪一版，讓使用者確認自己看的是同一個東西
    pub version: Option<String>,
    /// 這個後端有哪些模型與推理強度可選
    pub options: CliOptions,
}

/// 把「找不到直譯器」翻成使用者能動手處理的話。
///
/// `/usr/bin/env: 'node': No such file or directory` 加上退出碼 127
/// 對使用者是天書，而這正好是從應用程式選單啟動時最容易撞到的錯誤。
/// 原始訊息要留著，但前面得說清楚發生什麼事、可以怎麼辦。
fn interpreter_hint(body: &str, program: &str) -> Option<String> {
    let missing = ["node", "python", "python3", "deno", "bun", "ruby"]
        .into_iter()
        .find(|name| {
            body.contains("env:") && body.contains(&format!("'{name}'"))
                || body.contains(&format!("{name}: not found"))
        })?;

    Some(format!(
        "`{program}` 是 {missing} 程式，但系統找不到 {missing}。\n\n\
         如果你在終端機裡跑 `{program}` 是正常的，那多半是因為 {missing} 裝在 \
         nvm / asdf 之類的版本管理器底下——那些路徑只有互動式 shell 才會載入，\
         從應用程式選單啟動的程式看不到。\n\n\
         可以試試：從終端機啟動 App，或把 {missing} 的路徑加進 ~/.profile。\n\n\
         原始訊息：{body}"
    ))
}

/// 準備一個帶著使用者完整 `PATH` 的指令。
///
/// 從應用程式選單啟動的 GUI 拿到的是 session 的最小 `PATH`。`claude` 與
/// `codex` 都是 Node 程式，shebang 寫 `#!/usr/bin/env node`，而 `node`
/// 常常裝在 nvm 的版本目錄下——那個路徑只有 `~/.bashrc` 會加。
///
/// 結果是指令找得到、直譯器找不到，退出碼 127：
/// `/usr/bin/env: 'node': No such file or directory`。
async fn command_with_user_path(program: &str) -> Command {
    let mut cmd = Command::new(program);
    if let Some(shell_path) = shell_path::user_path().await {
        let current = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", shell_path::merge(&shell_path, &current));
    }
    cmd
}

/// 這台機器上有哪些 CLI 可以用。
///
/// 直接執行 `--version` 而不是找 PATH：使用者可能用 alias、
/// wrapper script 或自訂路徑，能不能跑起來才是真正的答案。
/// 兩個指令都在 0.3 秒內回應，開設定頁時查一次不影響體感。
pub async fn detect_backends() -> Vec<CliAvailability> {
    let candidates = [
        (CliPreset::ClaudeCode, "Claude Code", "claude"),
        (CliPreset::Codex, "OpenAI Codex", "codex"),
    ];

    let mut out = Vec::new();
    for (preset, label, program) in candidates {
        let version = command_with_user_path(program)
            .await
            .arg("--version")
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|v| !v.is_empty());

        out.push(CliAvailability {
            preset,
            label: label.to_string(),
            program: program.to_string(),
            installed: version.is_some(),
            version,
            options: cli_options(preset),
        });
    }
    out
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

        args.extend(self.config.effort_style.args(&self.config.effort));

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
                // CLI 每次都是全新的行程，完全沒有對話狀態——
                // 先前的往返必須寫進這一次的輸入，否則「參考上次的回答」
                // 這種要求根本無從執行。
                //
                // 唯一要濾掉的是 json_only 用來預填開頭的單一 `{`：
                // 那是 API 的技巧，對 CLI 只是雜訊。
                Role::Assistant if message.content.trim() == "{" => continue,
                Role::Assistant => {
                    out.push_str("（你先前的回答）\n");
                    out.push_str(&message.content);
                }
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

        let mut child = command_with_user_path(&self.config.program)
            .await
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
            // 寫不進去**不能**當場放棄。
            //
            // CLI 提早結束（沒登入、參數不對、直接爆掉）時，它根本不會讀
            // stdin，這裡就會拿到 EPIPE。當場 `?` 回去的話，使用者看到的是
            // 「Broken pipe (os error 32)」——而工具其實已經在 stderr 上
            // 說清楚問題是什麼了。
            //
            // 所以寫失敗只記一筆，繼續往下等行程結束，把它自己的錯誤訊息
            // 撈出來回報。那才是使用者需要看到的東西。
            if let Err(e) = stdin.write_all(input.as_bytes()).await {
                tracing::debug!(error = %e, "prompt 沒寫完，指令可能已經結束了");
            }
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
            // 錯誤通常在 stderr，但有些工具印在 stdout
            let body = if stderr.trim().is_empty() {
                stdout.trim().to_string()
            } else {
                stderr.trim().to_string()
            };
            let code = output.status.code().unwrap_or(-1);

            if let Some(hint) = interpreter_hint(&body, &self.config.program) {
                return Err(LlmError::Decode(hint));
            }
            return Err(LlmError::Api {
                status: code as u16,
                body,
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

    /// json_only 用來預填開頭的單一 `{` 是 API 技巧，對 CLI 只是雜訊。
    #[test]
    fn assistant_priming_is_not_sent_to_a_cli() {
        let cli = CliLlm::new(CliConfig::claude_code()).unwrap();
        let mut r = req();
        r.messages.push(Message::assistant("{"));

        let stdin = cli.build_stdin(&r);
        assert_eq!(stdin.trim(), "出一題", "預填的 `{{` 不該出現在輸入裡");
    }

    /// 但真正的對話歷史一定要送進去。
    ///
    /// CLI 每次都是全新行程，沒有任何對話狀態。覆蓋率重試時
    /// 「參考你上次寫的文章」如果沒被送進來，模型只能重寫一篇不相干的。
    #[test]
    fn real_conversation_history_reaches_the_cli() {
        let cli = CliLlm::new(CliConfig::claude_code()).unwrap();
        let mut r = req();
        r.messages
            .push(Message::assistant(r#"{"passage":"The cat sat."}"#));
        r.messages.push(Message::user("這篇太難了，請重寫"));

        let stdin = cli.build_stdin(&r);
        assert!(stdin.contains("The cat sat."), "上次的回答不見了：{stdin}");
        assert!(stdin.contains("這篇太難了"));
        // 順序要對，不然看起來像是先要求重寫再給文章
        assert!(stdin.find("The cat sat.").unwrap() < stdin.find("這篇太難了").unwrap());
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

    /// claude 用獨立旗標，codex 走設定覆寫——形狀不同，不能用同一套硬套。
    #[test]
    fn each_cli_passes_effort_in_its_own_shape() {
        let mut cfg = CliConfig::claude_code();
        cfg.effort = "low".into();
        let args = CliLlm::new(cfg).unwrap().build_args(&req());
        let pos = args
            .iter()
            .position(|a| a == "--effort")
            .expect("沒有 effort");
        assert_eq!(args[pos + 1], "low");

        let mut cfg = CliConfig::codex();
        cfg.effort = "medium".into();
        let args = CliLlm::new(cfg).unwrap().build_args(&req());
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-c" && w[1] == "model_reasoning_effort=medium"),
            "{args:?}"
        );
    }

    /// 留空代表「用 CLI 自己的預設」，不能傳一個空字串進去。
    #[test]
    fn an_empty_effort_adds_nothing() {
        let mut cfg = CliConfig::claude_code();
        cfg.effort = "  ".into();
        let args = CliLlm::new(cfg).unwrap().build_args(&req());
        assert!(!args.iter().any(|a| a == "--effort"), "{args:?}");
    }

    /// 不支援的 CLI 不能被塞一個它看不懂的參數。
    #[test]
    fn an_unsupported_cli_never_gets_an_effort_argument() {
        let mut cfg = CliConfig::preset(CliPreset::Custom);
        cfg.program = "sh".into();
        cfg.effort = "high".into();
        let args = CliLlm::new(cfg).unwrap().build_args(&req());
        assert!(args.is_empty(), "{args:?}");
    }

    /// 設定頁靠這份清單做下拉選單。清單會過期，但空的清單等於沒有 UI。
    #[test]
    fn every_supported_preset_offers_choices() {
        for preset in [CliPreset::ClaudeCode, CliPreset::Codex] {
            let o = cli_options(preset);
            assert!(!o.models.is_empty(), "{preset:?} 沒有模型選項");
            assert!(!o.efforts.is_empty(), "{preset:?} 沒有強度選項");
        }
        // 預設值要嘛在清單裡，要嘛是空的（代表「用 CLI 自己的設定」）。
        // 落在兩者之外的話，設定頁一打開就會顯示成「自訂」，很奇怪。
        for (preset, default_model) in [
            (CliPreset::ClaudeCode, CliConfig::claude_code().model),
            (CliPreset::Codex, CliConfig::codex().model),
        ] {
            let models = cli_options(preset).models;
            assert!(
                default_model.is_empty() || models.contains(&default_model),
                "{preset:?} 的預設模型 {default_model:?} 不在清單裡"
            );
        }
        assert!(
            cli_options(CliPreset::ClaudeCode)
                .efforts
                .contains(&CliConfig::claude_code().effort)
        );
    }

    /// 舊的設定檔沒有 effort 欄位，讀進來不能整份壞掉。
    #[test]
    fn settings_written_before_effort_existed_still_load() {
        let old = r#"{"preset":"claude_code","program":"claude","args":["-p"],
                      "system_flag":null,"model_flag":"--model","model":"sonnet",
                      "timeout_secs":300}"#;
        let cfg: CliConfig = serde_json::from_str(old).unwrap();
        assert_eq!(cfg.model, "sonnet");
        assert_eq!(cfg.effort, "");
        assert_eq!(cfg.effort_style, EffortStyle::Unsupported);
    }

    /// 試跑失敗時要說得出下一步，不能只丟原始訊息。
    ///
    /// `gpt-5.6-luna` 在 codex-cli 0.142.5 上就是這個情況：模型是真的，
    /// 但 CLI 太舊。使用者需要知道的是「去升級 codex」，不是那串 JSON。
    #[test]
    fn a_too_old_cli_is_explained_not_just_echoed() {
        let raw = r#"{"type":"error","status":400,"error":{"message":"The 'gpt-5.6-luna' model requires a newer version of Codex."}}"#;
        let hint = model_error_hint(raw);
        assert!(hint.contains("codex update"), "{hint}");
        assert!(hint.contains(raw), "原始訊息要留著：{hint}");
    }

    /// 認不出來的錯誤原樣傳回去，不要猜。
    #[test]
    fn an_unrecognised_error_is_passed_through() {
        assert_eq!(
            model_error_hint("rate limit exceeded"),
            "rate limit exceeded"
        );
    }

    /// 試跑不該讓使用者等一篇文章那麼久。
    #[tokio::test]
    async fn probing_a_broken_program_fails_fast() {
        let mut cfg = CliConfig::codex();
        cfg.program = "definitely-not-a-real-program-xyz".into();
        let result = probe_model(cfg, "whatever").await;
        assert!(!result.usable);
        assert!(result.detail.contains("找不到指令"), "{}", result.detail);
    }

    /// 偵測要看「跑不跑得起來」而不是「PATH 裡有沒有」——
    /// 使用者可能用 alias、wrapper script 或自訂路徑。
    #[tokio::test]
    async fn detection_reports_every_known_backend() {
        let found = detect_backends().await;

        assert_eq!(found.len(), 2, "兩個後端都要回報，沒裝的也要");
        assert!(found.iter().any(|b| b.preset == CliPreset::ClaudeCode));
        assert!(found.iter().any(|b| b.preset == CliPreset::Codex));

        // 裝了的一定要帶版本，沒裝的一定不能帶
        for b in &found {
            assert_eq!(b.installed, b.version.is_some(), "{}", b.program);
        }
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
            effort_style: EffortStyle::Unsupported,
            effort: String::new(),
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
    ///
    /// 這條測試曾經是不穩的，而原因是個真的 bug：`sh -c 'echo ... >&2; exit 3'`
    /// 根本不讀 stdin，所以寫 prompt 時會拿到 EPIPE。以前那個錯誤會被
    /// 當場 `?` 回去，使用者看到「Broken pipe (os error 32)」而不是工具
    /// 自己印的訊息——沒登入、參數打錯都會變成同一句無用的話。
    ///
    /// 誰先跑完是競爭條件，所以本機常常是綠的、CI 上才掛。
    #[tokio::test]
    async fn a_failing_command_reports_stderr() {
        let cli = CliLlm::new(CliConfig {
            preset: CliPreset::Custom,
            program: "sh".into(),
            args: vec!["-c".into(), "echo 出事了 >&2; exit 3".into()],
            system_flag: None,
            model_flag: None,
            model: "failing".into(),
            effort_style: EffortStyle::Unsupported,
            effort: String::new(),
            timeout_secs: 10,
        })
        .unwrap();

        let err = cli.chat(&req()).await.unwrap_err();
        assert!(err.to_string().contains("出事了"), "{err}");
    }

    /// 指令沒讀 stdin 就結束時，要回報它自己的錯誤而不是「Broken pipe」。
    ///
    /// 這是上一條測試不穩的根因，值得單獨釘住：prompt 有好幾 KB，
    /// 一定會填滿管線緩衝區，所以寫入必定失敗。
    #[tokio::test]
    async fn a_command_that_ignores_stdin_still_reports_its_own_error() {
        let cli = CliLlm::new(CliConfig {
            preset: CliPreset::Custom,
            program: "sh".into(),
            // 立刻結束，完全不讀 stdin
            args: vec!["-c".into(), "echo 請先登入 >&2; exit 1".into()],
            system_flag: None,
            model_flag: None,
            model: String::new(),
            effort_style: EffortStyle::Unsupported,
            effort: String::new(),
            timeout_secs: 10,
        })
        .unwrap();

        // 用一個大到一定會塞爆管線緩衝區的 prompt
        let mut req = req();
        req.messages[0].content = "字".repeat(200_000);

        let err = cli.chat(&req).await.unwrap_err().to_string();
        assert!(err.contains("請先登入"), "應該回報指令自己說的話：{err}");
        assert!(!err.contains("Broken pipe"), "{err}");
    }

    /// 從選單啟動時最容易撞到的錯誤，不能只丟一個 127 給使用者。
    ///
    /// 實際發生過：`codex` 在 ~/.local/bin（找得到），但它的 shebang 是
    /// `#!/usr/bin/env node`，而 node 在 nvm 的版本目錄下（只有 ~/.bashrc
    /// 會加）。使用者看到的是「供應商回傳錯誤（HTTP 127）」。
    #[tokio::test]
    async fn a_missing_interpreter_gets_an_actionable_message() {
        let cli = CliLlm::new(CliConfig {
            preset: CliPreset::Custom,
            program: "sh".into(),
            args: vec![
                "-c".into(),
                "echo \"/usr/bin/env: 'node': No such file or directory\" >&2; exit 127".into(),
            ],
            system_flag: None,
            model_flag: None,
            model: String::new(),
            effort_style: EffortStyle::Unsupported,
            effort: String::new(),
            timeout_secs: 10,
        })
        .unwrap();

        let err = cli.chat(&req()).await.unwrap_err().to_string();
        assert!(err.contains("找不到 node"), "{err}");
        assert!(err.contains("nvm"), "要講出可能的原因：{err}");
        assert!(
            err.contains("No such file or directory"),
            "原始訊息要留著，不然沒辦法查：{err}"
        );
    }

    /// 一般的失敗不該被誤判成缺直譯器。
    #[test]
    fn ordinary_errors_are_not_mistaken_for_a_missing_interpreter() {
        assert_eq!(interpreter_hint("rate limit exceeded", "claude"), None);
        assert_eq!(
            interpreter_hint("please run `claude login`", "claude"),
            None
        );
        // 訊息裡剛好提到 node 但不是「找不到」也不該中
        assert_eq!(
            interpreter_hint("upgrading node modules, please wait", "codex"),
            None
        );
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
            effort_style: EffortStyle::Unsupported,
            effort: String::new(),
            timeout_secs: 1,
        })
        .unwrap();

        let err = cli.chat(&req()).await.unwrap_err();
        assert!(err.to_string().contains("沒有回應"), "{err}");
    }
}
