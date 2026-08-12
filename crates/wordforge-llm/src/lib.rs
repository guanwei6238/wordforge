//! # wordforge-llm
//!
//! LLM 供應商的抽象層。AI 功能是**選配**：沒有設定任何供應商時，
//! App 仍可正常匯入字典、背單字、複習，只是不能產生閱讀理解與批改作文。
//!
//! 支援三種接法：
//! - [`Provider::Anthropic`]：填自己的 Anthropic API key
//! - [`Provider::OpenAiCompatible`]：OpenAI 或任何相容端點（含各種代理）
//! - [`Provider::Ollama`]：本機模型，完全離線、零成本
//! - [`cli`]：直接呼叫本機的 `claude` / `codex` CLI，用既有的訂閱而不另開 API 帳單
//!
//! ## 不想付兩份錢
//!
//! 已經有 Claude 或 ChatGPT 訂閱的話，[`cli`] 讓你直接用本機的
//! `claude -p` / `codex exec`，不必再開一份 API 帳單。代價是比較慢、
//! 有速率限制，而且輸出比 API 髒（要從 CLI 的雜訊裡撈 JSON）。
//!
//! ## API key 的存放
//!
//! key 存在 app 資料目錄的獨立檔案（權限 600），**不進 SQLite**——
//! 資料庫常被複製到雲端硬碟同步，不該夾帶憑證。
//! 改用系統 keychain 是之後的事，見 roadmap。

pub mod cli;
pub mod client;
pub mod prompts;
mod shell_path;

pub use cli::{
    CliAvailability, CliConfig, CliLlm, CliOptions, CliPreset, EffortStyle, ModelProbe,
    cli_options, detect_backends, probe_model,
};
pub use client::{HttpLlm, LlmProvider};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("網路請求失敗：{0}")]
    Http(#[from] reqwest::Error),

    #[error("供應商回傳錯誤（HTTP {status}）：{body}")]
    Api { status: u16, body: String },

    #[error("回應格式無法解析：{0}")]
    Decode(String),

    #[error("尚未設定 LLM 供應商，此功能需要模型才能使用")]
    NotConfigured,

    #[error("{provider} 需要 API key")]
    MissingApiKey { provider: &'static str },
}

pub type Result<T> = std::result::Result<T, LlmError>;

/// 供應商種類。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Anthropic,
    /// OpenAI 或任何相容 `/v1/chat/completions` 的端點
    OpenAiCompatible,
    /// 本機 Ollama，預設 `http://localhost:11434/v1`
    Ollama,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAiCompatible => "openai",
            Provider::Ollama => "ollama",
        }
    }

    pub fn default_base_url(&self) -> &'static str {
        match self {
            Provider::Anthropic => "https://api.anthropic.com",
            Provider::OpenAiCompatible => "https://api.openai.com/v1",
            Provider::Ollama => "http://localhost:11434/v1",
        }
    }

    /// 本機模型不需要 key。
    pub fn requires_api_key(&self) -> bool {
        !matches!(self, Provider::Ollama)
    }
}

/// 供應商設定。`api_key` 不參與序列化，避免不小心被寫進設定檔或 log。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: Provider,
    pub model: String,
    /// 留空則用 [`Provider::default_base_url`]
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(skip)]
    pub api_key: Option<String>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
}

fn default_max_tokens() -> u32 {
    4096
}

fn default_temperature() -> f32 {
    0.7
}

impl LlmConfig {
    pub fn anthropic(api_key: impl Into<String>) -> Self {
        Self {
            provider: Provider::Anthropic,
            model: "claude-sonnet-5".into(),
            base_url: None,
            api_key: Some(api_key.into()),
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
        }
    }

    /// 本機 Ollama，完全離線。
    pub fn ollama(model: impl Into<String>) -> Self {
        Self {
            provider: Provider::Ollama,
            model: model.into(),
            base_url: None,
            api_key: None,
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
        }
    }

    pub fn base_url(&self) -> &str {
        self.base_url
            .as_deref()
            .unwrap_or_else(|| self.provider.default_base_url())
    }

    pub fn validate(&self) -> Result<()> {
        if self.provider.requires_api_key() && self.api_key.as_deref().unwrap_or("").is_empty() {
            return Err(LlmError::MissingApiKey {
                provider: self.provider.as_str(),
            });
        }
        Ok(())
    }
}

/// 對話中的一則訊息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

/// 一次生成請求。
#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    pub system: Option<String>,
    pub messages: Vec<Message>,
    /// 要求模型只輸出 JSON。出題與批改都靠這個，才能穩定解析。
    pub json_only: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatResponse {
    pub text: String,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
}

impl ChatResponse {
    /// 從回應中取出 JSON 物件。
    ///
    /// 即使要求只輸出 JSON，實際拿到的東西還是很髒：模型會包上 ```json 圍欄
    /// 或加一句開場白，透過 CLI 執行時還會混進工具自己的輸出。
    /// codex 的非交互模式就長這樣：
    ///
    /// ```text
    /// warning: Codex's Linux sandbox uses bubblewrap...
    /// codex
    /// {"ok": true}
    /// tokens used
    /// 7,105
    /// {"ok": true}
    /// ```
    ///
    /// 「第一個 `{` 到最後一個 `}`」在這裡會抓到兩個物件加中間的雜訊。
    /// 所以改成掃出所有頂層物件，從最後一個開始試——
    /// 最後出現的通常才是模型的最終答案。
    pub fn json(&self) -> Result<serde_json::Value> {
        let text = self.text.trim();

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
            return Ok(value);
        }

        for candidate in top_level_objects(text).into_iter().rev() {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(candidate) {
                return Ok(value);
            }
        }

        Err(LlmError::Decode(format!("回應中找不到 JSON 物件：{text}")))
    }
}

/// 找出文字裡所有「頂層」的 `{...}` 區塊。
///
/// 括號深度必須從 0 數起，而且要跳過字串內容——
/// 釋義裡出現一個 `}` 就把物件切斷的話，什麼都解析不出來。
fn top_level_objects(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    let mut in_string = false;
    let mut escaped = false;

    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }

        match b {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0
                    && let Some(s) = start.take()
                {
                    found.push(&text[s..=i]);
                }
            }
            _ => {}
        }
    }

    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_needs_no_key() {
        let cfg = LlmConfig::ollama("qwen3:8b");
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.base_url(), "http://localhost:11434/v1");
    }

    #[test]
    fn cloud_providers_require_a_key() {
        let mut cfg = LlmConfig::anthropic("");
        assert!(matches!(
            cfg.validate(),
            Err(LlmError::MissingApiKey {
                provider: "anthropic"
            })
        ));
        cfg.api_key = Some("sk-test".into());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn api_key_never_serializes() {
        let cfg = LlmConfig::anthropic("sk-super-secret");
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(
            !json.contains("sk-super-secret"),
            "API key 外洩到設定檔：{json}"
        );
    }

    #[test]
    fn extracts_json_from_chatty_responses() {
        let r = ChatResponse {
            text: "當然！這是你的題目：\n```json\n{\"question\": \"why\"}\n```\n希望有幫助".into(),
            input_tokens: None,
            output_tokens: None,
        };
        assert_eq!(r.json().unwrap()["question"], "why");
    }

    /// codex 的非交互輸出：前面有警告、中間有 token 統計、答案印兩次。
    /// 這是實際跑出來的格式，不是想像的。
    #[test]
    fn extracts_the_final_object_from_cli_noise() {
        let r = ChatResponse {
            text: "warning: Codex's Linux sandbox uses bubblewrap...\n\
                   codex\n\
                   {\"ok\": true, \"n\": 7}\n\
                   tokens used\n\
                   7,105\n\
                   {\"ok\": true, \"n\": 7}"
                .into(),
            input_tokens: None,
            output_tokens: None,
        };
        assert_eq!(r.json().unwrap()["n"], 7);
    }

    /// 前面印了一個不完整或無關的物件時，要取最後那個完整的。
    #[test]
    fn prefers_the_last_complete_object() {
        let r = ChatResponse {
            text: "{\"status\": \"thinking\"}\n最終答案：\n{\"score\": 90}".into(),
            input_tokens: None,
            output_tokens: None,
        };
        assert_eq!(r.json().unwrap()["score"], 90);
    }

    /// 巢狀物件不能被內層的 `}` 切斷。
    #[test]
    fn handles_nested_objects() {
        let r = ChatResponse {
            text: "```json\n{\"a\": {\"b\": {\"c\": 1}}, \"d\": 2}\n```".into(),
            input_tokens: None,
            output_tokens: None,
        };
        let v = r.json().unwrap();
        assert_eq!(v["a"]["b"]["c"], 1);
        assert_eq!(v["d"], 2);
    }

    /// 字串裡的大括號是資料，不是結構。
    #[test]
    fn braces_inside_strings_do_not_end_the_object() {
        let r = ChatResponse {
            text: r#"{"gloss": "用法：{名詞} + 動詞", "ok": true}"#.into(),
            input_tokens: None,
            output_tokens: None,
        };
        let v = r.json().unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["gloss"], "用法：{名詞} + 動詞");
    }

    /// 跳脫的引號不該讓字串提早結束。
    #[test]
    fn escaped_quotes_are_handled() {
        let r = ChatResponse {
            text: r#"前言 {"quote": "他說 \"你好\" 然後離開", "n": 1} 後記"#.into(),
            input_tokens: None,
            output_tokens: None,
        };
        assert_eq!(r.json().unwrap()["n"], 1);
    }

    #[test]
    fn reports_missing_json_clearly() {
        let r = ChatResponse {
            text: "抱歉，我無法完成".into(),
            input_tokens: None,
            output_tokens: None,
        };
        assert!(matches!(r.json(), Err(LlmError::Decode(_))));
    }
}
