//! Chat completions against OpenAI-compatible `/chat/completions` (Ollama default).

use std::time::{Duration, Instant};

use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{AppError, Result};
use crate::llm::provider::{LlmApiDialect, LlmProviderKind};

/// Default HTTP timeout when `RAG_LLM_TIMEOUT_SECS` is unset or invalid.
/// Matches `Config` / SPEC default (long compiles on local models).
const DEFAULT_TIMEOUT_SECS: u64 = 600;
/// Cap for status probes so a hung daemon fails fast.
const STATUS_PROBE_TIMEOUT_SECS: u64 = 30;
/// Default completion budget when `RAG_LLM_MAX_TOKENS` is unset.
/// Matches `Config` default.
const DEFAULT_MAX_TOKENS: u32 = 2048;

/// One chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// One wiki page proposed by the compile model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompilePage {
    pub slug: String,
    pub title: String,
    /// `source_summary` | `entity` | `concept` | `wiki`
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub category: Option<String>,
    pub content: String,
    #[serde(default)]
    pub summary: Option<String>,
}

fn default_kind() -> String {
    "wiki".into()
}

/// Structured compile output from the local LLM.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompileResult {
    #[serde(default)]
    pub pages: Vec<CompilePage>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// One wiki page proposed by consolidating N source documents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidateProposal {
    /// Optional; derived from title when empty/missing.
    #[serde(default)]
    pub slug: String,
    pub title: String,
    /// `concept` | `entity` | `wiki` (default `concept`).
    #[serde(default = "default_consolidate_kind")]
    pub kind: String,
    #[serde(default)]
    pub category: Option<String>,
    pub content: String,
    #[serde(default)]
    pub summary: Option<String>,
    /// Suggested `[[wikilinks]]` / page titles the agent may add or keep.
    #[serde(default)]
    pub suggested_links: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

fn default_consolidate_kind() -> String {
    "concept".into()
}

/// Result of [`ChatClient::llm_status`] (rich health probe).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmStatus {
    /// True when models list or a minimal chat round-trip succeeded.
    pub ok: bool,
    /// Provider preset name (`ollama`, `claude`, `deepseek`, …).
    #[serde(default)]
    pub provider: String,
    /// Wire protocol: `openai_compat` or `anthropic_messages`.
    #[serde(default)]
    pub dialect: String,
    pub base_url: String,
    pub model: String,
    pub timeout_secs: u64,
    pub max_tokens: u32,
    pub latency_ms: u64,
    /// Probe path that succeeded or was last tried: `models` or `chat`.
    pub probe: String,
    /// Human-readable detail (error text or short success note).
    pub detail: String,
    /// Model ids from `GET /models` when available.
    #[serde(default)]
    pub available_models: Vec<String>,
    /// Whether configured `model` appeared in `available_models` (if list non-empty).
    pub model_present: Option<bool>,
}

/// Multi-provider chat client (OpenAI-compat or Anthropic Messages).
#[derive(Debug, Clone)]
pub struct ChatClient {
    client: Client,
    provider: LlmProviderKind,
    dialect: LlmApiDialect,
    base_url: String,
    api_key: String,
    model: String,
    timeout_secs: u64,
    max_tokens: u32,
}

impl ChatClient {
    /// Build a client. `base_url` is API root (e.g. `http://127.0.0.1:11434/v1`).
    ///
    /// Timeout: `RAG_LLM_TIMEOUT_SECS` (default 600).  
    /// Completion budget: `RAG_LLM_MAX_TOKENS` (default 2048).
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self> {
        Self::with_provider(
            LlmProviderKind::Custom,
            base_url,
            api_key,
            model,
            timeout_secs_from_env(),
            max_tokens_from_env(),
        )
    }

    /// Build from runtime config (provider preset + base URL / model / key).
    /// Built even when `llm_enabled` is false so status probes work;
    /// tools should refuse via server policy when disabled.
    pub fn from_config(config: &Config) -> Result<Self> {
        let max_tokens = u32::try_from(config.llm_max_tokens)
            .unwrap_or(u32::MAX)
            .max(1);
        Self::with_provider(
            config.llm_provider,
            config.llm_base_url.as_str(),
            config.llm_api_key.as_str(),
            config.llm_model.as_str(),
            config.llm_timeout_secs,
            max_tokens,
        )
    }

    /// Build a client with explicit timeout and max_tokens (tests / callers).
    pub fn with_limits(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        timeout_secs: u64,
        max_tokens: u32,
    ) -> Result<Self> {
        Self::with_provider(
            LlmProviderKind::Custom,
            base_url,
            api_key,
            model,
            timeout_secs,
            max_tokens,
        )
    }

    /// Full constructor with provider dialect.
    pub fn with_provider(
        provider: LlmProviderKind,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        timeout_secs: u64,
        max_tokens: u32,
    ) -> Result<Self> {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        if base_url.is_empty() {
            return Err(AppError::llm("LLM base_url must not be empty"));
        }
        let model = model.into();
        if model.trim().is_empty() {
            return Err(AppError::llm("LLM model must not be empty"));
        }
        let timeout_secs = timeout_secs.max(1);
        let max_tokens = max_tokens.max(1);
        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| AppError::llm(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            client,
            provider,
            dialect: provider.dialect(),
            base_url,
            api_key: api_key.into(),
            model,
            timeout_secs,
            max_tokens,
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn provider(&self) -> LlmProviderKind {
        self.provider
    }

    pub fn dialect(&self) -> LlmApiDialect {
        self.dialect
    }

    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }

    pub fn max_tokens(&self) -> u32 {
        self.max_tokens
    }

    fn dialect_label(&self) -> &'static str {
        match self.dialect {
            LlmApiDialect::OpenAiCompat => "openai_compat",
            LlmApiDialect::AnthropicMessages => "anthropic_messages",
        }
    }

    fn status_shell(
        &self,
        ok: bool,
        latency_ms: u64,
        probe: &str,
        detail: String,
        available_models: Vec<String>,
        model_present: Option<bool>,
    ) -> LlmStatus {
        LlmStatus {
            ok,
            provider: self.provider.as_str().to_string(),
            dialect: self.dialect_label().to_string(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            timeout_secs: self.timeout_secs,
            max_tokens: self.max_tokens,
            latency_ms,
            probe: probe.into(),
            detail,
            available_models,
            model_present,
        }
    }

    /// Lightweight reachability check: `GET /models`, else minimal chat.
    ///
    /// Returns `Ok(())` when the endpoint answers; `Err` with detail otherwise.
    pub async fn ping(&self) -> Result<()> {
        let status = self.llm_status().await;
        if status.ok {
            Ok(())
        } else {
            Err(AppError::llm(status.detail))
        }
    }

    /// Free-form chat completion; returns assistant text.
    /// Uses configured `max_tokens` (`RAG_LLM_MAX_TOKENS` / config).
    pub async fn complete(&self, messages: &[ChatMessage]) -> Result<String> {
        self.complete_with_max_tokens(messages, self.max_tokens)
            .await
    }

    /// Chat completion with an explicit token budget (status probes, short tools).
    pub async fn complete_with_max_tokens(
        &self,
        messages: &[ChatMessage],
        max_tokens: u32,
    ) -> Result<String> {
        match self.dialect {
            LlmApiDialect::OpenAiCompat => {
                self.complete_openai_compat(messages, max_tokens).await
            }
            LlmApiDialect::AnthropicMessages => {
                self.complete_anthropic(messages, max_tokens).await
            }
        }
    }

    async fn complete_openai_compat(
        &self,
        messages: &[ChatMessage],
        max_tokens: u32,
    ) -> Result<String> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = ChatRequest {
            model: &self.model,
            messages,
            temperature: 0.2,
            stream: false,
            max_tokens: max_tokens.max(1),
        };
        let mut req = self.client.post(&url).json(&body);
        if !self.api_key.trim().is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| AppError::llm(format!("chat request failed: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| AppError::llm(format!("chat read body failed: {e}")))?;
        if !status.is_success() {
            return Err(AppError::llm(format!(
                "chat HTTP {status}: {}",
                truncate(&text, 500)
            )));
        }
        let parsed: ChatResponse = serde_json::from_str(&text).map_err(|e| {
            AppError::llm(format!(
                "chat JSON parse failed: {e}; body={}",
                truncate(&text, 300)
            ))
        })?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message)
            .map(|m| m.content)
            .unwrap_or_default();
        if content.trim().is_empty() {
            return Err(AppError::llm("chat returned empty assistant content"));
        }
        Ok(content)
    }

    async fn complete_anthropic(
        &self,
        messages: &[ChatMessage],
        max_tokens: u32,
    ) -> Result<String> {
        // Anthropic: system is a top-level field; messages are user/assistant only.
        let mut system = String::new();
        let mut conv: Vec<AnthropicMessage> = Vec::new();
        for m in messages {
            match m.role.as_str() {
                "system" => {
                    if !system.is_empty() {
                        system.push('\n');
                    }
                    system.push_str(&m.content);
                }
                "assistant" => conv.push(AnthropicMessage {
                    role: "assistant",
                    content: m.content.as_str(),
                }),
                _ => conv.push(AnthropicMessage {
                    role: "user",
                    content: m.content.as_str(),
                }),
            }
        }
        if conv.is_empty() {
            return Err(AppError::llm(
                "anthropic chat requires at least one non-system message",
            ));
        }
        // API root is typically https://api.anthropic.com (no /v1 in base for our preset).
        let url = if self.base_url.ends_with("/v1") {
            format!("{}/messages", self.base_url)
        } else {
            format!("{}/v1/messages", self.base_url)
        };
        let body = AnthropicRequest {
            model: &self.model,
            max_tokens: max_tokens.max(1),
            temperature: 0.2,
            system: if system.is_empty() {
                None
            } else {
                Some(system.as_str())
            },
            messages: &conv,
        };
        let mut req = self
            .client
            .post(&url)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body);
        if !self.api_key.trim().is_empty() {
            req = req.header("x-api-key", self.api_key.as_str());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| AppError::llm(format!("anthropic request failed: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| AppError::llm(format!("anthropic read body failed: {e}")))?;
        if !status.is_success() {
            return Err(AppError::llm(format!(
                "anthropic HTTP {status}: {}",
                truncate(&text, 500)
            )));
        }
        let parsed: AnthropicResponse = serde_json::from_str(&text).map_err(|e| {
            AppError::llm(format!(
                "anthropic JSON parse failed: {e}; body={}",
                truncate(&text, 300)
            ))
        })?;
        let content = parsed
            .content
            .into_iter()
            .filter(|b| b.block_type == "text" || b.block_type.is_empty())
            .map(|b| b.text.unwrap_or_default())
            .collect::<Vec<_>>()
            .join("");
        if content.trim().is_empty() {
            return Err(AppError::llm("anthropic returned empty assistant content"));
        }
        Ok(content)
    }

    /// Probe local/remote OpenAI-compatible LLM: `GET /models`, else minimal chat.
    ///
    /// Always returns [`LlmStatus`] (never Err for transport/API failure) so MCP
    /// tools can report `ok: false` with detail.
    pub async fn llm_status(&self) -> LlmStatus {
        let start = Instant::now();
        let probe_timeout = Duration::from_secs(self.timeout_secs.min(STATUS_PROBE_TIMEOUT_SECS));

        match self.ping_models(probe_timeout).await {
            Ok(models) => {
                let model_present = if models.is_empty() {
                    None
                } else {
                    Some(models.iter().any(|id| model_id_matches(id, &self.model)))
                };
                let detail = match model_present {
                    Some(true) => format!(
                        "models list ok; configured model '{}' present ({} listed)",
                        self.model,
                        models.len()
                    ),
                    Some(false) => format!(
                        "models list ok; configured model '{}' not in list ({} listed)",
                        self.model,
                        models.len()
                    ),
                    None => "models list ok (empty or unparsed ids)".into(),
                };
                // Reachable if models endpoint works; warn when configured model missing.
                let ok = model_present != Some(false);
                self.status_shell(
                    ok,
                    elapsed_ms(start),
                    "models",
                    detail,
                    models,
                    model_present,
                )
            }
            Err(models_err) => match self.ping_minimal_chat(probe_timeout).await {
                Ok(reply) => self.status_shell(
                    true,
                    elapsed_ms(start),
                    "chat",
                    format!(
                        "models failed ({}); minimal chat ok: {}",
                        truncate(&models_err, 120),
                        truncate(&reply, 80)
                    ),
                    vec![],
                    None,
                ),
                Err(chat_err) => self.status_shell(
                    false,
                    elapsed_ms(start),
                    "chat",
                    format!(
                        "models failed ({}); chat failed ({})",
                        truncate(&models_err, 120),
                        truncate(&chat_err, 160)
                    ),
                    vec![],
                    None,
                ),
            },
        }
    }

    async fn ping_models(&self, timeout: Duration) -> std::result::Result<Vec<String>, String> {
        if matches!(self.dialect, LlmApiDialect::AnthropicMessages) {
            // Anthropic has no public models list on the same path; skip to chat probe.
            return Err("anthropic: models list not used".into());
        }
        let url = format!("{}/models", self.base_url);
        let mut req = self.client.get(&url).timeout(timeout);
        if !self.api_key.trim().is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("models request failed: {e}"))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("models read body failed: {e}"))?;
        if !status.is_success() {
            return Err(format!(
                "models HTTP {status}: {}",
                truncate(&text, 200)
            ));
        }
        let parsed: ModelsResponse = serde_json::from_str(&text).map_err(|e| {
            format!(
                "models JSON parse failed: {e}; body={}",
                truncate(&text, 200)
            )
        })?;
        let ids: Vec<String> = parsed
            .data
            .into_iter()
            .filter_map(|m| {
                let id = m.id.trim().to_string();
                if id.is_empty() {
                    None
                } else {
                    Some(id)
                }
            })
            .collect();
        Ok(ids)
    }

    async fn ping_minimal_chat(&self, timeout: Duration) -> std::result::Result<String, String> {
        let messages = [ChatMessage {
            role: "user".into(),
            content: "Reply with exactly: ok".into(),
        }];
        // Temporary client with short timeout for probe.
        let probe = Self {
            client: Client::builder()
                .timeout(timeout)
                .build()
                .map_err(|e| format!("probe client: {e}"))?,
            provider: self.provider,
            dialect: self.dialect,
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            model: self.model.clone(),
            timeout_secs: timeout.as_secs().max(1),
            max_tokens: 8,
        };
        probe
            .complete_with_max_tokens(&messages, 8)
            .await
            .map_err(|e| e.to_string())
    }

    /// Compile a raw source into wiki pages (JSON object in model output).
    pub async fn compile_wiki(
        &self,
        schema_md: &str,
        source_title: &str,
        source_uri: &str,
        source_body: &str,
    ) -> Result<CompileResult> {
        let system = r#"You are a knowledge compiler for a local LLM wiki (Karpathy-style).
Given an immutable raw source, produce structured wiki pages.

Rules:
- Never invent facts not grounded in the source; mark gaps as TODO.
- Use markdown. Cross-link with [[Page Title]] and tags as #tag.
- Prefer few high-quality pages over many stubs.
- Return ONLY valid JSON (no markdown fences) matching:
{
  "pages": [
    {
      "slug": "kebab-case",
      "title": "Human Title",
      "kind": "source_summary|entity|concept|wiki",
      "category": "optional",
      "content": "markdown body",
      "summary": "one-line"
    }
  ],
  "notes": "optional compiler notes"
}
"#;
        let user = format!(
            "## Wiki schema / conventions\n\n{schema}\n\n## Source\n- title: {title}\n- uri: {uri}\n\n```\n{body}\n```\n",
            schema = truncate(schema_md, 6000),
            title = source_title,
            uri = source_uri,
            body = truncate(source_body, 24_000),
        );
        let messages = [
            ChatMessage {
                role: "system".into(),
                content: system.into(),
            },
            ChatMessage {
                role: "user".into(),
                content: user,
            },
        ];
        let raw = self.complete(&messages).await?;
        parse_compile_json(&raw)
    }

    /// Merge N document texts into one consolidated wiki page proposal (JSON).
    ///
    /// Used by the `consolidate` maintenance tool: propose first; callers write
    /// only when `apply=true`. Never invents facts beyond the provided sources.
    pub async fn consolidate_wiki(
        &self,
        schema_md: &str,
        sources: &[(String, String, String)],
    ) -> Result<ConsolidateProposal> {
        if sources.is_empty() {
            return Err(AppError::llm(
                "consolidate_wiki requires at least one source document",
            ));
        }
        let system = r#"You are a knowledge consolidator for a local LLM wiki (Karpathy-style).
Merge the given source documents into ONE durable wiki page (concept or entity).

Rules:
- Never invent facts not grounded in the sources; mark gaps as TODO.
- Deduplicate overlapping claims; prefer precise wording over length.
- Use markdown. Cross-link with [[Page Title]] and tags as #tag.
- List provenance under a ## Sources section (titles / uris).
- Return ONLY valid JSON (no markdown fences) matching:
{
  "slug": "kebab-case",
  "title": "Human Title",
  "kind": "concept|entity|wiki",
  "category": "optional",
  "content": "markdown body",
  "summary": "one-line",
  "suggested_links": ["Other Page", "[[Yet Another]]"],
  "notes": "optional consolidator notes"
}
"#;
        // Budget: schema + N sources within a reasonable prompt size.
        let per_body = (20_000usize / sources.len().max(1)).clamp(1_500, 8_000);
        let mut blocks = String::new();
        for (i, (title, uri, body)) in sources.iter().enumerate() {
            blocks.push_str(&format!(
                "### Source {}\n- title: {}\n- uri: {}\n\n```\n{}\n```\n\n",
                i + 1,
                title,
                uri,
                truncate(body, per_body)
            ));
        }
        let user = format!(
            "## Wiki schema / conventions\n\n{schema}\n\n## Sources to consolidate ({n})\n\n{blocks}",
            schema = truncate(schema_md, 4_000),
            n = sources.len(),
            blocks = blocks,
        );
        let messages = [
            ChatMessage {
                role: "system".into(),
                content: system.into(),
            },
            ChatMessage {
                role: "user".into(),
                content: user,
            },
        ];
        let raw = self.complete(&messages).await?;
        parse_consolidate_json(&raw)
    }
}

fn timeout_secs_from_env() -> u64 {
    std::env::var("RAG_LLM_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
}

fn max_tokens_from_env() -> u32 {
    std::env::var("RAG_LLM_MAX_TOKENS")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_TOKENS)
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

/// True if list entry matches configured model (exact or Ollama `name:tag` prefix).
fn model_id_matches(listed: &str, configured: &str) -> bool {
    let listed = listed.trim();
    let configured = configured.trim();
    if listed == configured {
        return true;
    }
    // Ollama often lists `llama3.2:latest` while config has `llama3.2`.
    if let Some((base, _)) = listed.split_once(':') {
        if base == configured {
            return true;
        }
    }
    if let Some((base, _)) = configured.split_once(':') {
        if base == listed {
            return true;
        }
    }
    false
}

fn strip_json_fences(raw: &str) -> &str {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest
            .strip_prefix("json")
            .or_else(|| rest.strip_prefix("JSON"))
            .unwrap_or(rest);
        let rest = rest.trim_start_matches('\n');
        rest.strip_suffix("```").unwrap_or(rest).trim()
    } else {
        trimmed
    }
}

fn parse_compile_json(raw: &str) -> Result<CompileResult> {
    let unfenced = strip_json_fences(raw);
    // Try full parse, then first {...} slice.
    if let Ok(v) = serde_json::from_str::<CompileResult>(unfenced) {
        return Ok(v);
    }
    if let (Some(start), Some(end)) = (unfenced.find('{'), unfenced.rfind('}')) {
        if end > start {
            let slice = &unfenced[start..=end];
            if let Ok(v) = serde_json::from_str::<CompileResult>(slice) {
                return Ok(v);
            }
        }
    }
    Err(AppError::llm(format!(
        "failed to parse compile JSON from model output: {}",
        truncate(raw, 400)
    )))
}

fn parse_consolidate_json(raw: &str) -> Result<ConsolidateProposal> {
    let unfenced = strip_json_fences(raw);
    if let Ok(v) = serde_json::from_str::<ConsolidateProposal>(unfenced) {
        return validate_consolidate_proposal(v);
    }
    if let (Some(start), Some(end)) = (unfenced.find('{'), unfenced.rfind('}')) {
        if end > start {
            let slice = &unfenced[start..=end];
            if let Ok(v) = serde_json::from_str::<ConsolidateProposal>(slice) {
                return validate_consolidate_proposal(v);
            }
        }
    }
    // Fallback: model returned compile-style `{ "pages": [ ... ] }` with one page.
    if let Ok(compile) = parse_compile_json(raw) {
        if let Some(page) = compile.pages.into_iter().next() {
            return validate_consolidate_proposal(ConsolidateProposal {
                slug: page.slug,
                title: page.title,
                kind: page.kind,
                category: page.category,
                content: page.content,
                summary: page.summary,
                suggested_links: vec![],
                notes: compile.notes,
            });
        }
    }
    Err(AppError::llm(format!(
        "failed to parse consolidate JSON from model output: {}",
        truncate(raw, 400)
    )))
}

fn validate_consolidate_proposal(mut p: ConsolidateProposal) -> Result<ConsolidateProposal> {
    p.slug = p.slug.trim().to_string();
    p.title = p.title.trim().to_string();
    p.content = p.content.trim().to_string();
    p.kind = p.kind.trim().to_string();
    if p.kind.is_empty() {
        p.kind = default_consolidate_kind();
    }
    if p.title.is_empty() {
        return Err(AppError::llm(
            "consolidate proposal missing non-empty title",
        ));
    }
    if p.content.is_empty() {
        return Err(AppError::llm(
            "consolidate proposal missing non-empty content",
        ));
    }
    if p.slug.is_empty() {
        // Derive a rough slug from title (ASCII-ish); wiki write will re-slugify.
        p.slug = p
            .title
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");
    }
    if p.slug.is_empty() {
        return Err(AppError::llm(
            "consolidate proposal missing slug and could not derive one from title",
        ));
    }
    Ok(p)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    temperature: f32,
    stream: bool,
    max_tokens: u32,
}

#[derive(Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    messages: &'a [AnthropicMessage<'a>],
}

#[derive(Serialize)]
struct AnthropicMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    #[serde(default)]
    content: Vec<AnthropicContentBlock>,
}

#[derive(Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type", default)]
    block_type: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: Option<ChatMsgBody>,
}

#[derive(Deserialize)]
struct ChatMsgBody {
    #[serde(default)]
    content: String,
}

#[derive(Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    #[serde(default)]
    id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_json() {
        let raw = r#"{"pages":[{"slug":"a","title":"A","kind":"wiki","content":"hi"}],"notes":null}"#;
        let r = parse_compile_json(raw).unwrap();
        assert_eq!(r.pages.len(), 1);
        assert_eq!(r.pages[0].slug, "a");
    }

    #[test]
    fn parse_fenced_json() {
        let raw = "```json\n{\"pages\":[{\"slug\":\"b\",\"title\":\"B\",\"content\":\"x\"}]}\n```";
        let r = parse_compile_json(raw).unwrap();
        assert_eq!(r.pages[0].slug, "b");
    }

    #[test]
    fn parse_consolidate_plain_json() {
        let raw = r#"{
            "slug": "hybrid-search",
            "title": "Hybrid Search",
            "kind": "concept",
            "content": "Merges BM25 and vectors. [[RRF]] #search",
            "summary": "BM25 + vec",
            "suggested_links": ["RRF", "[[FTS]]"],
            "notes": "merged 2 notes"
        }"#;
        let p = parse_consolidate_json(raw).unwrap();
        assert_eq!(p.slug, "hybrid-search");
        assert_eq!(p.kind, "concept");
        assert_eq!(p.suggested_links.len(), 2);
        assert!(p.content.contains("BM25"));
    }

    #[test]
    fn parse_consolidate_fenced_and_slug_from_title() {
        let raw = "```json\n{\"title\":\"My Topic\",\"content\":\"body text\"}\n```";
        let p = parse_consolidate_json(raw).unwrap();
        assert_eq!(p.slug, "my-topic");
        assert_eq!(p.kind, "concept");
    }

    #[test]
    fn parse_consolidate_from_compile_pages_shape() {
        let raw = r#"{"pages":[{"slug":"c","title":"C","kind":"entity","content":"hi"}],"notes":"n"}"#;
        let p = parse_consolidate_json(raw).unwrap();
        assert_eq!(p.slug, "c");
        assert_eq!(p.notes.as_deref(), Some("n"));
    }

    #[test]
    fn parse_consolidate_rejects_empty_content() {
        let err = parse_consolidate_json(r#"{"slug":"a","title":"A","content":"  "}"#).unwrap_err();
        assert!(err.to_string().contains("content"));
    }

    #[test]
    fn with_limits_rejects_empty_base() {
        let err = ChatClient::with_limits("", "k", "m", 10, 100).unwrap_err();
        assert!(err.to_string().contains("base_url"));
    }

    #[test]
    fn with_limits_rejects_empty_model() {
        let err =
            ChatClient::with_limits("http://127.0.0.1:11434/v1", "k", "  ", 10, 100).unwrap_err();
        assert!(err.to_string().contains("model"));
    }

    #[test]
    fn with_limits_stores_timeout_and_max_tokens() {
        let c =
            ChatClient::with_limits("http://127.0.0.1:11434/v1", "ollama", "llama3.2", 45, 512)
                .unwrap();
        assert_eq!(c.timeout_secs(), 45);
        assert_eq!(c.max_tokens(), 512);
        assert_eq!(c.model(), "llama3.2");
        assert_eq!(c.base_url(), "http://127.0.0.1:11434/v1");
    }

    #[test]
    fn with_limits_clamps_zero_to_one() {
        let c = ChatClient::with_limits("http://127.0.0.1:1/v1", "", "m", 0, 0).unwrap();
        assert_eq!(c.timeout_secs(), 1);
        assert_eq!(c.max_tokens(), 1);
    }

    #[test]
    fn model_id_matches_variants() {
        assert!(model_id_matches("llama3.2", "llama3.2"));
        assert!(model_id_matches("llama3.2:latest", "llama3.2"));
        assert!(model_id_matches("llama3.2", "llama3.2:latest"));
        assert!(!model_id_matches("mistral", "llama3.2"));
    }

    #[test]
    fn chat_request_includes_max_tokens() {
        let messages = [ChatMessage {
            role: "user".into(),
            content: "hi".into(),
        }];
        let body = ChatRequest {
            model: "m",
            messages: &messages,
            temperature: 0.2,
            stream: false,
            max_tokens: 256,
        };
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["max_tokens"], 256);
        assert_eq!(v["stream"], false);
    }

    #[test]
    fn from_config_uses_timeout_and_max_tokens() {
        let cfg = Config {
            db_path: std::path::PathBuf::from("./x.duckdb"),
            embedding_provider: crate::config::EmbeddingProviderKind::Mock,
            embedding_base_url: "https://api.openai.com/v1".into(),
            embedding_api_key: String::new(),
            embedding_model: "mock".into(),
            embedding_dims: 8,
            chunk_size: 800,
            chunk_overlap: 120,
            default_top_k: 5,
            ingest_roots: vec![],
            max_context_tokens: 4096,
            max_chunks_per_doc: 3,
            fts_stemmer: "porter".into(),
            default_search_mode: crate::models::SearchMode::Vec,
            llm_base_url: "http://127.0.0.1:11434/v1".into(),
            llm_provider: crate::llm::LlmProviderKind::Ollama,
            llm_model: "llama3.2".into(),
            llm_api_key: "ollama".into(),
            llm_enabled: false,
            llm_timeout_secs: 90,
            llm_max_tokens: 777,
            maint_max_docs: 50,
            maint_near_dup_threshold: 0.92,
            tool_surface: crate::mcp::ToolSurface::Full,
            http_bind: None,
            wiki_require_if_match: false,
        };
        let c = ChatClient::from_config(&cfg).unwrap();
        assert_eq!(c.timeout_secs(), 90);
        assert_eq!(c.max_tokens(), 777);
        assert_eq!(c.model(), "llama3.2");
    }

    #[test]
    fn llm_status_unreachable_host_is_not_ok() {
        let c = ChatClient::with_limits("http://127.0.0.1:9/v1", "x", "nope", 1, 16).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let status = rt.block_on(c.llm_status());
        assert!(!status.ok);
        assert_eq!(status.probe, "chat");
        assert!(!status.detail.is_empty());
        assert_eq!(status.timeout_secs, 1);
        assert_eq!(status.max_tokens, 16);
        assert!(rt.block_on(c.ping()).is_err());
    }
}
