//! Multi-provider chat LLM client: Ollama, OpenAI/Codex, Claude, Kimi, DeepSeek.

mod chat;
mod provider;

pub use chat::{
    ChatClient, ChatMessage, CompilePage, CompileResult, ConsolidateProposal, LlmStatus,
};
pub use provider::{resolve_api_key, LlmApiDialect, LlmProviderKind};
