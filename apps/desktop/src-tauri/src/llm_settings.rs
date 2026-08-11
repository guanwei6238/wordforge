//! LLM 設定的存放。
//!
//! 存在 app 資料目錄的獨立檔案而不是 SQLite：資料庫常被使用者複製到
//! 雲端硬碟同步、或用 SQLite 工具打開來看，不該夾帶 API key。
//! Unix 上會把權限設成 600。
//!
//! 用系統 keychain 更好，但那在 Linux 需要 Secret Service，
//! 沒有桌面環境的機器就完全設定不了。先用檔案讓每個人都能用。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wordforge_llm::{CliConfig, CliLlm, CliPreset, HttpLlm, LlmConfig, LlmProvider, Provider};

/// 要用哪一種後端。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// 還沒設定。AI 功能停用，但背單字照常。
    #[default]
    None,
    /// 本機的 claude / codex CLI，用既有訂閱
    Cli,
    /// HTTP API（Anthropic / OpenAI 相容 / Ollama）
    Api,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiSettings {
    pub provider: Provider,
    pub model: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: String,
}

impl Default for ApiSettings {
    fn default() -> Self {
        Self {
            provider: Provider::Ollama,
            model: "qwen3:8b".into(),
            base_url: None,
            api_key: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LlmSettings {
    #[serde(default)]
    pub backend: Backend,
    #[serde(default)]
    pub cli: Option<CliConfig>,
    #[serde(default)]
    pub api: Option<ApiSettings>,
}

impl LlmSettings {
    pub fn file(dir: &Path) -> PathBuf {
        dir.join("llm.json")
    }

    pub fn load(dir: &Path) -> Self {
        let path = Self::file(dir);
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        // 設定檔壞掉時退回「沒設定」而不是讓 App 開不起來
        serde_json::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "LLM 設定檔解析失敗，當作沒有設定");
            Self::default()
        })
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = Self::file(dir);
        std::fs::write(&path, serde_json::to_string_pretty(self)?)?;
        restrict_permissions(&path)?;
        Ok(())
    }

    /// 建立實際可用的 provider。沒設定就回 `None`——
    /// 呼叫端要能區分「沒設定」與「設定錯了」。
    pub fn build(&self) -> wordforge_llm::Result<Option<Box<dyn LlmProvider>>> {
        match self.backend {
            Backend::None => Ok(None),
            Backend::Cli => {
                let cfg = self
                    .cli
                    .clone()
                    .unwrap_or_else(|| CliConfig::preset(CliPreset::ClaudeCode));
                Ok(Some(Box::new(CliLlm::new(cfg)?)))
            }
            Backend::Api => {
                let api = self.api.clone().unwrap_or_default();
                let cfg = LlmConfig {
                    provider: api.provider,
                    model: api.model,
                    base_url: api.base_url,
                    api_key: Some(api.api_key),
                    max_tokens: 4096,
                    temperature: 0.7,
                };
                Ok(Some(Box::new(HttpLlm::new(cfg)?)))
            }
        }
    }

    /// 給前端看的版本：API key 換成「有沒有設定」，不把祕密送進 WebView。
    pub fn redacted(&self) -> serde_json::Value {
        let mut api = serde_json::to_value(self.api.clone().unwrap_or_default())
            .unwrap_or(serde_json::Value::Null);
        if let Some(obj) = api.as_object_mut() {
            let has_key = obj
                .get("api_key")
                .and_then(|k| k.as_str())
                .map(|k| !k.is_empty())
                .unwrap_or(false);
            obj.insert("api_key".into(), serde_json::Value::String(String::new()));
            obj.insert("has_api_key".into(), serde_json::Value::Bool(has_key));
        }

        serde_json::json!({
            "backend": self.backend,
            "cli": self.cli.clone().unwrap_or_else(|| CliConfig::preset(CliPreset::ClaudeCode)),
            "api": api,
        })
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> std::io::Result<()> {
    // Windows 的預設 ACL 已經限制在使用者自己的資料夾內
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wordforge-llm-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_means_not_configured() {
        let dir = temp_dir("missing");
        std::fs::remove_dir_all(&dir).ok();
        let s = LlmSettings::load(&dir);
        assert_eq!(s.backend, Backend::None);
        assert!(s.build().unwrap().is_none(), "沒設定就不該有 provider");
    }

    #[test]
    fn settings_round_trip() {
        let dir = temp_dir("round-trip");
        let settings = LlmSettings {
            backend: Backend::Cli,
            cli: Some(CliConfig::claude_code()),
            api: None,
        };
        settings.save(&dir).unwrap();

        let loaded = LlmSettings::load(&dir);
        assert_eq!(loaded.backend, Backend::Cli);
        assert_eq!(loaded.cli.as_ref().unwrap().program, "claude");
        assert!(loaded.build().unwrap().is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 設定檔壞掉不該讓 App 開不起來。
    #[test]
    fn a_broken_file_falls_back_to_unconfigured() {
        let dir = temp_dir("broken");
        std::fs::write(LlmSettings::file(&dir), "{ not json").unwrap();
        assert_eq!(LlmSettings::load(&dir).backend, Backend::None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// API key 絕對不能送進 WebView。
    #[test]
    fn the_redacted_view_hides_the_key() {
        let settings = LlmSettings {
            backend: Backend::Api,
            cli: None,
            api: Some(ApiSettings {
                provider: Provider::Anthropic,
                model: "claude-sonnet-5".into(),
                base_url: None,
                api_key: "sk-super-secret".into(),
            }),
        };

        let json = serde_json::to_string(&settings.redacted()).unwrap();
        assert!(!json.contains("sk-super-secret"), "API key 外洩：{json}");
        assert!(json.contains("\"has_api_key\":true"), "但要看得出有設定");
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_not_readable_by_others() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("perms");
        LlmSettings {
            backend: Backend::Api,
            cli: None,
            api: Some(ApiSettings {
                api_key: "sk-test".into(),
                ..Default::default()
            }),
        }
        .save(&dir)
        .unwrap();

        let mode = std::fs::metadata(LlmSettings::file(&dir))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "設定檔權限太寬鬆");

        std::fs::remove_dir_all(&dir).ok();
    }
}
