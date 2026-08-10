//! HTTP 客戶端實作。
//!
//! 兩家的協定差異不大，共用同一個 struct，只在組請求與解回應時分流。
//! 組請求與解回應都抽成純函數，這樣不需要真的打 API 就能測試。

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{ChatRequest, ChatResponse, LlmConfig, LlmError, Provider, Result, Role};

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse>;

    /// 目前使用的模型名稱，用於在 UI 顯示與記錄到 `exercise.model`。
    fn model(&self) -> &str;
}

pub struct HttpLlm {
    config: LlmConfig,
    http: reqwest::Client,
}

impl HttpLlm {
    pub fn new(config: LlmConfig) -> Result<Self> {
        config.validate()?;
        let http = reqwest::Client::builder()
            // 產生一篇閱讀理解可能要一分鐘以上，本機模型更久
            .timeout(std::time::Duration::from_secs(180))
            .build()?;
        Ok(Self { config, http })
    }

    fn endpoint(&self) -> String {
        match self.config.provider {
            Provider::Anthropic => format!("{}/v1/messages", self.config.base_url()),
            _ => format!("{}/chat/completions", self.config.base_url()),
        }
    }

    fn body(&self, req: &ChatRequest) -> Value {
        match self.config.provider {
            Provider::Anthropic => build_anthropic_body(&self.config, req),
            _ => build_openai_body(&self.config, req),
        }
    }
}

#[async_trait]
impl LlmProvider for HttpLlm {
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse> {
        let mut builder = self.http.post(self.endpoint()).json(&self.body(req));

        builder = match self.config.provider {
            Provider::Anthropic => builder
                .header("x-api-key", self.config.api_key.clone().unwrap_or_default())
                .header("anthropic-version", "2023-06-01"),
            _ => match self.config.api_key.as_deref() {
                Some(key) if !key.is_empty() => builder.bearer_auth(key),
                _ => builder,
            },
        };

        let resp = builder.send().await?;
        let status = resp.status();
        let text = resp.text().await?;

        if !status.is_success() {
            return Err(LlmError::Api {
                status: status.as_u16(),
                body: text,
            });
        }

        let value: Value =
            serde_json::from_str(&text).map_err(|e| LlmError::Decode(e.to_string()))?;

        match self.config.provider {
            Provider::Anthropic => parse_anthropic_response(&value),
            _ => parse_openai_response(&value),
        }
    }

    fn model(&self) -> &str {
        &self.config.model
    }
}

// ---------------------------------------------------------------- 請求組裝

fn role_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

pub fn build_anthropic_body(cfg: &LlmConfig, req: &ChatRequest) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| json!({ "role": role_str(m.role), "content": m.content }))
        .collect();

    let mut body = json!({
        "model": cfg.model,
        "max_tokens": cfg.max_tokens,
        "temperature": cfg.temperature,
        "messages": messages,
    });

    // Anthropic 的 system 是 top-level 欄位，不是一則 message
    if let Some(system) = &req.system {
        body["system"] = json!(system);
    }
    if req.json_only {
        // 預填助理回覆的開頭，強迫模型直接從 JSON 開始輸出
        body["messages"]
            .as_array_mut()
            .expect("messages 剛剛才建成陣列")
            .push(json!({ "role": "assistant", "content": "{" }));
    }
    body
}

pub fn build_openai_body(cfg: &LlmConfig, req: &ChatRequest) -> Value {
    let mut messages: Vec<Value> = Vec::with_capacity(req.messages.len() + 1);
    if let Some(system) = &req.system {
        messages.push(json!({ "role": "system", "content": system }));
    }
    messages.extend(
        req.messages
            .iter()
            .map(|m| json!({ "role": role_str(m.role), "content": m.content })),
    );

    let mut body = json!({
        "model": cfg.model,
        "max_tokens": cfg.max_tokens,
        "temperature": cfg.temperature,
        "messages": messages,
    });
    if req.json_only {
        body["response_format"] = json!({ "type": "json_object" });
    }
    body
}

// ---------------------------------------------------------------- 回應解析

pub fn parse_anthropic_response(v: &Value) -> Result<ChatResponse> {
    let text = v["content"]
        .as_array()
        .and_then(|blocks| {
            let joined: String = blocks
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("");
            (!joined.is_empty()).then_some(joined)
        })
        .ok_or_else(|| LlmError::Decode(format!("Anthropic 回應沒有文字內容：{v}")))?;

    Ok(ChatResponse {
        text,
        input_tokens: v["usage"]["input_tokens"].as_u64().map(|n| n as u32),
        output_tokens: v["usage"]["output_tokens"].as_u64().map(|n| n as u32),
    })
}

pub fn parse_openai_response(v: &Value) -> Result<ChatResponse> {
    let text = v["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| LlmError::Decode(format!("OpenAI 回應沒有文字內容：{v}")))?
        .to_string();

    Ok(ChatResponse {
        text,
        input_tokens: v["usage"]["prompt_tokens"].as_u64().map(|n| n as u32),
        output_tokens: v["usage"]["completion_tokens"].as_u64().map(|n| n as u32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    fn req() -> ChatRequest {
        ChatRequest {
            system: Some("你是英文老師".into()),
            messages: vec![Message::user("出一題")],
            json_only: false,
        }
    }

    #[test]
    fn anthropic_puts_system_at_top_level() {
        let body = build_anthropic_body(&LlmConfig::anthropic("k"), &req());
        assert_eq!(body["system"], "你是英文老師");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert_eq!(body["messages"][0]["role"], "user");
    }

    #[test]
    fn openai_puts_system_as_first_message() {
        let body = build_openai_body(&LlmConfig::ollama("qwen3:8b"), &req());
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["role"], "user");
    }

    #[test]
    fn json_mode_uses_each_providers_mechanism() {
        let r = ChatRequest {
            json_only: true,
            ..req()
        };

        let anthropic = build_anthropic_body(&LlmConfig::anthropic("k"), &r);
        let last = anthropic["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["role"], "assistant");
        assert_eq!(last["content"], "{");

        let openai = build_openai_body(&LlmConfig::ollama("m"), &r);
        assert_eq!(openai["response_format"]["type"], "json_object");
    }

    #[test]
    fn parses_anthropic_multi_block_response() {
        let v = json!({
            "content": [{"type": "text", "text": "第一段"}, {"type": "text", "text": "第二段"}],
            "usage": {"input_tokens": 12, "output_tokens": 34}
        });
        let r = parse_anthropic_response(&v).unwrap();
        assert_eq!(r.text, "第一段第二段");
        assert_eq!(r.input_tokens, Some(12));
        assert_eq!(r.output_tokens, Some(34));
    }

    #[test]
    fn parses_openai_response() {
        let v = json!({
            "choices": [{"message": {"role": "assistant", "content": "hello"}}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 7}
        });
        let r = parse_openai_response(&v).unwrap();
        assert_eq!(r.text, "hello");
        assert_eq!(r.input_tokens, Some(5));
    }

    #[test]
    fn malformed_responses_are_errors_not_panics() {
        assert!(parse_anthropic_response(&json!({"error": "overloaded"})).is_err());
        assert!(parse_openai_response(&json!({"choices": []})).is_err());
    }

    #[test]
    fn client_rejects_missing_key_before_any_request() {
        let cfg = LlmConfig {
            api_key: None,
            ..LlmConfig::anthropic("")
        };
        assert!(HttpLlm::new(cfg).is_err());
    }
}
