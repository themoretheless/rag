//! The one LLM seam domain code depends on.
//!
//! Wiki compile/consolidate, maintenance planning and organize, and the answer
//! judge take `&dyn ChatCompleter` instead of the HTTP `ChatClient`, so tests
//! can drive them with an in-process fake and prompts live with their domain.

use async_trait::async_trait;

use super::chat::{ChatClient, ChatMessage};
use crate::error::Result;

#[async_trait]
pub trait ChatCompleter: Send + Sync {
    /// Complete with the client's configured completion budget.
    async fn complete(&self, messages: &[ChatMessage]) -> Result<String>;

    /// Complete with an explicit completion budget.
    async fn complete_with_max_tokens(
        &self,
        messages: &[ChatMessage],
        max_tokens: u32,
    ) -> Result<String>;

    /// Model identifier recorded in reports.
    fn model(&self) -> &str;
}

#[async_trait]
impl ChatCompleter for ChatClient {
    async fn complete(&self, messages: &[ChatMessage]) -> Result<String> {
        ChatClient::complete(self, messages).await
    }

    async fn complete_with_max_tokens(
        &self,
        messages: &[ChatMessage],
        max_tokens: u32,
    ) -> Result<String> {
        ChatClient::complete_with_max_tokens(self, messages, max_tokens).await
    }

    fn model(&self) -> &str {
        ChatClient::model(self)
    }
}

/// `[system, user]` message pair used by every single-turn domain prompt.
pub fn system_user(system: &str, user: String) -> [ChatMessage; 2] {
    [
        ChatMessage {
            role: "system".into(),
            content: system.into(),
        },
        ChatMessage {
            role: "user".into(),
            content: user,
        },
    ]
}
