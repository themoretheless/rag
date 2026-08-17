# LLM providers (Ollama, Codex, Claude, Kimi, DeepSeek)

`rag-mcp` talks to chat models through **`RAG_LLM_PROVIDER`** presets.  
Embeddings stay independent (`RAG_EMBEDDING_*`).

## Quick switch

```bash
# Local (default)
export RAG_LLM_PROVIDER=ollama
export RAG_LLM_MODEL=llama3.2
# key optional; defaults to "ollama"

# OpenAI
export RAG_LLM_PROVIDER=openai
export RAG_LLM_API_KEY=$OPENAI_API_KEY   # or set OPENAI_API_KEY alone
export RAG_LLM_MODEL=gpt-4o-mini

# OpenAI Codex / code-oriented models (same host)
export RAG_LLM_PROVIDER=codex
export OPENAI_API_KEY=sk-...
export RAG_LLM_MODEL=gpt-4.1-mini

# Anthropic Claude (Messages API — not OpenAI wire format)
export RAG_LLM_PROVIDER=claude
export ANTHROPIC_API_KEY=sk-ant-...
# optional: RAG_LLM_MODEL=claude-sonnet-4-20250514

# Moonshot Kimi
export RAG_LLM_PROVIDER=kimi
export MOONSHOT_API_KEY=sk-...
# or KIMI_API_KEY / RAG_LLM_API_KEY

# DeepSeek
export RAG_LLM_PROVIDER=deepseek
export DEEPSEEK_API_KEY=sk-...

# Any OpenAI-compatible proxy (LM Studio, vLLM, …)
export RAG_LLM_PROVIDER=custom
export RAG_LLM_BASE_URL=http://127.0.0.1:1234/v1
export RAG_LLM_MODEL=my-model
export RAG_LLM_API_KEY=local
```

## Preset table

| Provider | Env value | Default base URL | Default model | Auth | Wire API |
|----------|-----------|------------------|---------------|------|----------|
| **Ollama** | `ollama` | `http://127.0.0.1:11434/v1` | `llama3.2` | optional / `ollama` | OpenAI chat |
| **OpenAI** | `openai` | `https://api.openai.com/v1` | `gpt-4o-mini` | `OPENAI_API_KEY` | OpenAI chat |
| **Codex** | `codex` | `https://api.openai.com/v1` | `gpt-4.1-mini` | `OPENAI_API_KEY` | OpenAI chat |
| **Claude** | `claude` | `https://api.anthropic.com` | `claude-sonnet-4-20250514` | `ANTHROPIC_API_KEY` | Anthropic Messages |
| **Kimi** | `kimi` | `https://api.moonshot.cn/v1` | `moonshot-v1-auto` | `MOONSHOT_API_KEY` | OpenAI chat |
| **DeepSeek** | `deepseek` | `https://api.deepseek.com/v1` | `deepseek-chat` | `DEEPSEEK_API_KEY` | OpenAI chat |
| **Custom** | `custom` | (set `RAG_LLM_BASE_URL`) | (set model) | optional | OpenAI chat |

Aliases: `anthropic`→claude, `moonshot`→kimi, `ds`→deepseek, `local`→ollama.

## Env reference

| Variable | Role |
|----------|------|
| `RAG_LLM_PROVIDER` | preset (default `ollama`) |
| `RAG_LLM_BASE_URL` | override preset base |
| `RAG_LLM_MODEL` | override default model |
| `RAG_LLM_API_KEY` | explicit key (wins over provider env) |
| `RAG_LLM_ENABLED` | `true`/`false` |
| `RAG_LLM_TIMEOUT_SECS` | HTTP timeout |
| `RAG_LLM_MAX_TOKENS` | completion budget |

Provider key fallbacks when `RAG_LLM_API_KEY` empty:

- openai/codex → `OPENAI_API_KEY`
- claude → `ANTHROPIC_API_KEY`, `CLAUDE_API_KEY`
- kimi → `MOONSHOT_API_KEY`, `KIMI_API_KEY`
- deepseek → `DEEPSEEK_API_KEY`

## MCP

- `llm_status` reports `provider`, `dialect`, `base_url`, `model`, `reachable`
- Compile / maintain / consolidate use the same `ChatClient`

## Local embeddings (with Ollama)

```bash
export RAG_EMBEDDING_PROVIDER=ollama
# or: openai + BASE_URL=http://127.0.0.1:11434/v1
export RAG_EMBEDDING_MODEL=nomic-embed-text
export RAG_EMBEDDING_DIMS=768
```

## Notes

- **Claude** uses Anthropic `x-api-key` + `/v1/messages`; system prompts are split out of the message list.
- Cloud providers require a non-empty API key when `RAG_LLM_ENABLED=true`.
- Override base URL for regional / proxy endpoints without changing provider dialect (except Claude dialect is fixed to Anthropic Messages).
