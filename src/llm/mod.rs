//! Multi-provider chat LLM client: Ollama, OpenAI/Codex, Claude, Kimi, DeepSeek.

mod chat;
mod completer;
pub mod json;
mod provider;

pub(crate) use chat::truncate;
pub use chat::{
    ChatClient, ChatMessage, CompilePage, CompileResult, ConsolidateProposal, LlmStatus,
};
pub(crate) use chat::{parse_compile_json, parse_consolidate_json};
pub use completer::{ChatCompleter, system_user};
pub use provider::{LlmApiDialect, LlmProviderKind, resolve_api_key};
