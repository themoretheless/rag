//! Multi-provider chat LLM client: Ollama, OpenAI/Codex, Claude, Kimi, DeepSeek.

mod chat;
mod completer;
pub mod json;
mod provider;

pub use chat::{
    ChatClient, ChatMessage, CompilePage, CompileResult, ConsolidateProposal, LlmStatus,
};
pub use completer::{system_user, ChatCompleter};
pub(crate) use chat::{parse_compile_json, parse_consolidate_json};
pub use provider::{resolve_api_key, LlmApiDialect, LlmProviderKind};
