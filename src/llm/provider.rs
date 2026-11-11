//! Named LLM provider presets: Ollama, OpenAI/Codex, Claude, Kimi, DeepSeek.

// The provider presets are configuration value types; they live in the
// domain-layer `config::providers` and are re-exported here.
pub use crate::config::providers::{resolve_api_key, LlmApiDialect, LlmProviderKind};

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
