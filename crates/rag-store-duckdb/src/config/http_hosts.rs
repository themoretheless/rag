//! Host allow-list for the HTTP gateway and its `/mcp` mount.

use std::collections::BTreeSet;
use std::net::SocketAddr;

use super::Config;

/// Exact Host authorities accepted by rmcp's DNS-rebinding guard.
///
/// Loopback is always available. A concrete bind address is safe to add
/// automatically; wildcard binds (`0.0.0.0` / `[::]`) require explicit remote
/// names or addresses in `RAG_HTTP_ALLOWED_HOSTS` (comma-separated).
pub fn configured_http_allowed_hosts(config: &Config, explicit: Option<&str>) -> Vec<String> {
    let mut hosts = BTreeSet::from([
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ]);
    if let Some(bind) = config
        .http_bind
        .as_deref()
        .and_then(|raw| raw.parse::<SocketAddr>().ok())
        .filter(|bind| !bind.ip().is_unspecified())
    {
        hosts.insert(bind.ip().to_string());
    }
    if let Some(explicit) = explicit {
        hosts.extend(
            explicit
                .split(',')
                .map(str::trim)
                .filter(|host| {
                    !host.is_empty()
                        && host.len() <= 255
                        && !host.contains('/')
                        && !host.chars().any(char::is_control)
                        && !host.chars().any(char::is_whitespace)
                })
                .map(str::to_owned),
        );
    }
    hosts.into_iter().collect()
}
