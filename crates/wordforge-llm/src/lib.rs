//! # wordforge-llm
//!
//! LLM 供應商的抽象層。AI 功能是**選配**：沒有設定任何供應商時，
//! App 仍可正常匯入字典、背單字、複習，只是不能產生閱讀理解與批改作文。
//!
//! 支援三種接法：
//! - [`Provider::Anthropic`]：填自己的 Anthropic API key
//! - [`Provider::OpenAiCompatible`]：OpenAI 或任何相容端點（含各種代理）
//! - [`Provider::Ollama`]：本機模型，完全離線、零成本
//!
//! ## API key 的存放
//!
//! key **不會**寫進 SQLite，而是交給作業系統的 keychain
//! （macOS Keychain / Windows Credential Manager / Linux Secret Service）。
//! 資料庫檔案常被使用者複製到雲端硬碟，不該夾帶憑證。

pub mod client;
pub mod prompts;

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
    /// 即使要求只輸出 JSON，模型仍可能包上 ```json 圍欄或加一句開場白，
    /// 所以這裡取第一個 `{` 到最後一個 `}` 之間的內容再解析。
    pub fn json(&self) -> Result<serde_json::Value> {
        let t = self.text.trim();
        let start = t.find('{');
        let end = t.rfind('}');
        let slice = match (start, end) {
            (Some(s), Some(e)) if e > s => &t[s..=e],
            _ => return Err(LlmError::Decode(format!("回應中找不到 JSON 物件：{t}"))),
        };
        serde_json::from_str(slice).map_err(|e| LlmError::Decode(e.to_string()))
    }
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
