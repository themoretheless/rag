//! Named LLM provider presets: Ollama, OpenAI/Codex, Claude, Kimi, DeepSeek.

use std::str::FromStr;

use crate::error::AppError;

/// Wire protocol used by [`super::ChatClient`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LlmApiDialect {
    /// `POST /chat/completions` + Bearer token (Ollama, OpenAI, DeepSeek, Kimi, …).
    #[default]
    OpenAiCompat,
    /// Anthropic Messages API: `POST /v1/messages` + `x-api-key`.
    AnthropicMessages,
}

/// Selectable chat LLM backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LlmProviderKind {
    /// Local Ollama (`http://127.0.0.1:11434/v1`).
    #[default]
    Ollama,
    /// OpenAI Chat Completions API.
    OpenAi,
    /// OpenAI Codex / code models (same API host; different default model).
    Codex,
    /// Anthropic Claude (Messages API).
    Claude,
    /// Moonshot Kimi (OpenAI-compatible).
    Kimi,
    /// DeepSeek (OpenAI-compatible).
    DeepSeek,
    /// Fully custom base URL / key / model via env (OpenAI-compatible).
    Custom,
}

impl LlmProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::OpenAi => "openai",
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Kimi => "kimi",
            Self::DeepSeek => "deepseek",
            Self::Custom => "custom",
        }
    }

    pub fn dialect(self) -> LlmApiDialect {
        match self {
            Self::Claude => LlmApiDialect::AnthropicMessages,
            _ => LlmApiDialect::OpenAiCompat,
        }
    }

    /// Default API root (no trailing slash issues handled by client).
    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::Ollama => "http://127.0.0.1:11434/v1",
            Self::OpenAi | Self::Codex => "https://api.openai.com/v1",
            Self::Claude => "https://api.anthropic.com",
            Self::Kimi => "https://api.moonshot.cn/v1",
            Self::DeepSeek => "https://api.deepseek.com/v1",
            Self::Custom => "http://127.0.0.1:11434/v1",
        }
    }

    /// Default model id when `RAG_LLM_MODEL` is unset.
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Ollama => "llama3.2",
            Self::OpenAi => "gpt-4o-mini",
            Self::Codex => "gpt-4.1-mini",
            Self::Claude => "claude-sonnet-4-20250514",
            Self::Kimi => "moonshot-v1-auto",
            Self::DeepSeek => "deepseek-chat",
            Self::Custom => "llama3.2",
        }
    }

    /// Env vars checked (in order) for API key when `RAG_LLM_API_KEY` is empty.
    pub fn api_key_fallbacks(self) -> &'static [&'static str] {
        match self {
            Self::Ollama | Self::Custom => &[],
            Self::OpenAi | Self::Codex => &["OPENAI_API_KEY"],
            Self::Claude => &["ANTHROPIC_API_KEY", "CLAUDE_API_KEY"],
            Self::Kimi => &["MOONSHOT_API_KEY", "KIMI_API_KEY"],
            Self::DeepSeek => &["DEEPSEEK_API_KEY"],
        }
    }

    /// Whether an empty API key is acceptable (local servers).
    pub fn allows_empty_api_key(self) -> bool {
        matches!(self, Self::Ollama | Self::Custom)
    }

    /// Dummy key used for local OpenAI-compat servers that require a header.
    pub fn local_placeholder_key(self) -> Option<&'static str> {
        match self {
            Self::Ollama => Some("ollama"),
            Self::Custom => Some("local"),
            _ => None,
        }
    }
}

impl FromStr for LlmProviderKind {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ollama" | "local" => Ok(Self::Ollama),
            "openai" | "gpt" => Ok(Self::OpenAi),
            "codex" | "openai-codex" => Ok(Self::Codex),
            "claude" | "anthropic" => Ok(Self::Claude),
            "kimi" | "moonshot" => Ok(Self::Kimi),
            "deepseek" | "ds" => Ok(Self::DeepSeek),
            "custom" | "openai_compat" | "openai-compat" => Ok(Self::Custom),
            other => Err(AppError::config(format!(
                "invalid RAG_LLM_PROVIDER '{other}': expected ollama|openai|codex|claude|kimi|deepseek|custom"
            ))),
        }
    }
}

/// Resolve API key: explicit `RAG_LLM_API_KEY`, then provider-specific env, then local placeholder.
pub fn resolve_api_key(provider: LlmProviderKind, explicit: &str) -> String {
    let t = explicit.trim();
    if !t.is_empty() {
        return t.to_string();
    }
    for name in provider.api_key_fallbacks() {
        if let Ok(v) = std::env::var(name) {
            let v = v.trim();
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    if let Some(ph) = provider.local_placeholder_key() {
        return ph.to_string();
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_aliases() {
        assert_eq!(
            "anthropic".parse::<LlmProviderKind>().unwrap(),
            LlmProviderKind::Claude
        );
        assert_eq!(
            "moonshot".parse::<LlmProviderKind>().unwrap(),
            LlmProviderKind::Kimi
        );
        assert_eq!(
            "ds".parse::<LlmProviderKind>().unwrap(),
            LlmProviderKind::DeepSeek
        );
    }

    #[test]
    fn claude_is_anthropic_dialect() {
        assert_eq!(
            LlmProviderKind::Claude.dialect(),
            LlmApiDialect::AnthropicMessages
        );
        assert_eq!(
            LlmProviderKind::DeepSeek.dialect(),
            LlmApiDialect::OpenAiCompat
        );
    }
}
