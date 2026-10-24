//! Web page ingestion (`ingest_url`): fetch one URL over HTTP(S), extract the
//! main content with the shared readability-lite HTML extractor, and feed it
//! through the standard ingest pipeline (chunking, hashing, dedup, ops_log).
//!
//! Safety policy:
//! - Disabled unless `RAG_INGEST_URL_ALLOW` lists host patterns
//!   (comma-separated; `*.example.com` matches the host and its subdomains).
//! - `http` / `https` only; redirects are bounded and the **final** host must
//!   also be allowlisted (a redirect cannot escape the policy).
//! - Response bodies are bounded by `RAG_INGEST_URL_MAX_BYTES` (default 10 MiB)
//!   and fetches time out after `RAG_INGEST_URL_TIMEOUT_SECS` (default 30).
//!
//! Non-goal: crawling / sitemaps. One URL per call.

use std::time::Duration;

use async_trait::async_trait;

use crate::error::{AppError, Result};

/// One fetched web page, after redirects.
#[derive(Debug, Clone)]
pub struct FetchedPage {
    /// Final URL after redirects (recorded as the document source).
    pub final_url: String,
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// Injectable HTTP fetch boundary so tests can run without a network.
#[async_trait]
pub trait UrlFetcher: Send + Sync {
    async fn fetch(&self, url: &str, max_bytes: usize) -> Result<FetchedPage>;
}

/// Production fetcher: reqwest (rustls), bounded redirects, streaming byte cap.
pub struct ReqwestFetcher {
    timeout: Duration,
}

impl ReqwestFetcher {
    pub fn new(timeout_secs: u64) -> Self {
        Self {
            timeout: Duration::from_secs(timeout_secs.max(1)),
        }
    }
}

#[async_trait]
impl UrlFetcher for ReqwestFetcher {
    async fn fetch(&self, url: &str, max_bytes: usize) -> Result<FetchedPage> {
        let client = reqwest::Client::builder()
            .timeout(self.timeout)
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent(concat!(
                "rag-mcp/",
                env!("CARGO_PKG_VERSION"),
                " ingest_url"
            ))
            .build()
            .map_err(|error| AppError::config(format!("ingest_url http client: {error}")))?;
        let mut response = client.get(url).send().await.map_err(|error| {
            AppError::config(format!("ingest_url fetch failed for {url}: {error}"))
        })?;
        let status = response.status().as_u16();
        let final_url = response.url().to_string();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.split(';').next().unwrap_or(value).trim().to_string());
        if !response.status().is_success() {
            return Err(AppError::config(format!(
                "ingest_url got HTTP {status} for {url} (final: {final_url})"
            )));
        }
        if let Some(length) = response.content_length() {
            if length > max_bytes as u64 {
                return Err(AppError::config(format!(
                    "ingest_url response for {final_url} declares {length} bytes, above the {max_bytes} byte cap"
                )));
            }
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            AppError::config(format!("ingest_url read failed for {final_url}: {error}"))
        })? {
            if body.len() + chunk.len() > max_bytes {
                return Err(AppError::config(format!(
                    "ingest_url response for {final_url} exceeds the {max_bytes} byte cap"
                )));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(FetchedPage {
            final_url,
            status,
            content_type,
            body,
        })
    }
}

/// Parsed + policy-checked target URL.
#[derive(Debug, Clone)]
pub struct CheckedUrl {
    pub url: reqwest::Url,
    pub host: String,
}

/// Validate scheme and host allowlist. `allow` empty = feature disabled.
pub fn check_url(raw: &str, allow: &[String]) -> Result<CheckedUrl> {
    if allow.is_empty() {
        return Err(AppError::forbidden(
            "ingest_url is disabled: set RAG_INGEST_URL_ALLOW to a comma-separated host allowlist (e.g. 'docs.rs,*.example.com')",
        ));
    }
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|error| AppError::config(format!("ingest_url invalid url '{raw}': {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AppError::forbidden(format!(
            "ingest_url only supports http/https URLs (got scheme '{}')",
            url.scheme()
        )));
    }
    let host = url
        .host_str()
        .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
        .filter(|host| !host.is_empty())
        .ok_or_else(|| AppError::config(format!("ingest_url url '{raw}' has no host")))?;
    ensure_host_allowed(allow, &host)?;
    Ok(CheckedUrl { url, host })
}

/// Re-check the allowlist against the final (post-redirect) URL.
pub fn check_final_url(final_url: &str, allow: &[String]) -> Result<CheckedUrl> {
    let parsed = reqwest::Url::parse(final_url).map_err(|error| {
        AppError::config(format!(
            "ingest_url invalid final url '{final_url}': {error}"
        ))
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(AppError::forbidden(format!(
            "ingest_url redirect target uses unsupported scheme '{}'",
            parsed.scheme()
        )));
    }
    let host = parsed
        .host_str()
        .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
        .ok_or_else(|| {
            AppError::config(format!("ingest_url final url '{final_url}' has no host"))
        })?;
    ensure_host_allowed(allow, &host)?;
    Ok(CheckedUrl { url: parsed, host })
}

fn ensure_host_allowed(allow: &[String], host: &str) -> Result<()> {
    if host_allowed(allow, host) {
        return Ok(());
    }
    Err(AppError::forbidden(format!(
        "ingest_url host '{host}' is not in RAG_INGEST_URL_ALLOW"
    )))
}

/// Host-pattern match: exact host, or `*.suffix` matching the suffix host and
/// any subdomain. Matching is case-insensitive and ignores a trailing dot.
pub fn host_allowed(allow: &[String], host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    allow.iter().any(|pattern| {
        let pattern = pattern.trim().trim_end_matches('.').to_ascii_lowercase();
        if let Some(suffix) = pattern.strip_prefix("*.") {
            host == suffix || host.ends_with(&format!(".{suffix}"))
        } else {
            host == pattern
        }
    })
}

/// Page body classification for extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    Html,
    Text,
}

/// Decide extraction from the content type; refuse anything but text.
pub fn classify_content_type(content_type: Option<&str>, final_url: &str) -> Result<PageKind> {
    let Some(raw) = content_type else {
        // Missing content type: sniff the extension, default to HTML for pages.
        return Ok(
            if final_url.ends_with(".txt") || final_url.ends_with(".md") {
                PageKind::Text
            } else {
                PageKind::Html
            },
        );
    };
    let mime = raw
        .split(';')
        .next()
        .unwrap_or(raw)
        .trim()
        .to_ascii_lowercase();
    if mime == "text/html" || mime == "application/xhtml+xml" {
        return Ok(PageKind::Html);
    }
    if mime.starts_with("text/") || mime == "application/json" || mime == "application/xml" {
        return Ok(PageKind::Text);
    }
    Err(AppError::config(format!(
        "ingest_url unsupported content type '{mime}' for {final_url} (only HTML and text are ingested)"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(patterns: &[&str]) -> Vec<String> {
        patterns.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn allowlist_matches_exact_and_wildcard_hosts() {
        let patterns = allow(&["docs.rs", "*.example.com"]);
        assert!(host_allowed(&patterns, "docs.rs"));
        assert!(host_allowed(&patterns, "example.com"));
        assert!(host_allowed(&patterns, "Sub.Example.COM"));
        assert!(!host_allowed(&patterns, "notexample.com"));
        assert!(!host_allowed(&patterns, "evil-example.com"));
        assert!(!host_allowed(&patterns, "docs.rs.evil.com"));
    }

    #[test]
    fn check_url_disabled_without_allowlist() {
        let error = check_url("https://docs.rs/anyhow", &[]).expect_err("disabled");
        assert!(error.to_string().contains("ingest_url is disabled"));
    }

    #[test]
    fn check_url_refuses_non_http_schemes_and_unknown_hosts() {
        let patterns = allow(&["docs.rs"]);
        let error = check_url("file:///etc/passwd", &patterns).expect_err("scheme");
        assert!(error.to_string().contains("http/https"));
        let error = check_url("ftp://docs.rs/x", &patterns).expect_err("scheme");
        assert!(error.to_string().contains("http/https"));
        let error = check_url("https://evil.com/x", &patterns).expect_err("host");
        assert!(error.to_string().contains("not in RAG_INGEST_URL_ALLOW"));
        assert!(check_url("https://docs.rs/anyhow", &patterns).is_ok());
    }

    #[test]
    fn check_final_url_catches_redirect_escape() {
        let patterns = allow(&["docs.rs"]);
        assert!(check_final_url("https://docs.rs/anyhow", &patterns).is_ok());
        let error = check_final_url("https://evil.com/stolen", &patterns).expect_err("escape");
        assert!(error.to_string().contains("not in RAG_INGEST_URL_ALLOW"));
    }

    #[test]
    fn classify_content_types() {
        assert_eq!(
            classify_content_type(Some("text/html; charset=utf-8"), "https://a/").unwrap(),
            PageKind::Html
        );
        assert_eq!(
            classify_content_type(Some("text/plain"), "https://a/").unwrap(),
            PageKind::Text
        );
        assert_eq!(
            classify_content_type(None, "https://a/notes.txt").unwrap(),
            PageKind::Text
        );
        assert!(classify_content_type(Some("application/pdf"), "https://a/x.pdf").is_err());
    }
}
