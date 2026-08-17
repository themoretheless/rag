//! `RAG_HTTP_BIND` parse + loopback gate (`RAG_HTTP_ALLOW_REMOTE`).

use std::net::SocketAddr;

use crate::error::AppError;

/// Parse `RAG_HTTP_BIND` like `127.0.0.1:7432`. Empty / unset → None.
///
/// Non-loopback binds require `RAG_HTTP_ALLOW_REMOTE=1|true|yes|on` (MCP is unauthenticated).
pub fn parse_bind(raw: &str) -> Result<Option<SocketAddr>, AppError> {
    let s = raw.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let addr = s
        .parse::<SocketAddr>()
        .map_err(|e| AppError::config(format!("invalid RAG_HTTP_BIND '{s}': {e}")))?;
    if !addr.ip().is_loopback() {
        let allow = std::env::var("RAG_HTTP_ALLOW_REMOTE")
            .ok()
            .and_then(|v| crate::config::parse_env_truthy(&v))
            .unwrap_or(false);
        if !allow {
            return Err(AppError::config(format!(
                "RAG_HTTP_BIND '{s}' is not loopback; set RAG_HTTP_ALLOW_REMOTE=true to expose \
                 unauthenticated HTTP/MCP on the network (dangerous)"
            )));
        }
    }
    Ok(Some(addr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::sync::Mutex;

    /// Serialize tests that mutate `RAG_HTTP_ALLOW_REMOTE`.
    static ALLOW_REMOTE_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parse_bind_empty_is_none() {
        assert_eq!(parse_bind("").unwrap(), None);
        assert_eq!(parse_bind("   ").unwrap(), None);
        assert_eq!(parse_bind("\t\n").unwrap(), None);
    }

    #[test]
    fn parse_bind_loopback_ipv4_ok_without_allow_remote() {
        let _g = ALLOW_REMOTE_LOCK.lock().unwrap();
        std::env::remove_var("RAG_HTTP_ALLOW_REMOTE");
        let addr = parse_bind("127.0.0.1:7432").unwrap().expect("some");
        assert_eq!(addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(addr.port(), 7432);
        // Trim is applied; loopback never consults allow-remote.
        let addr = parse_bind("  127.0.0.1:80  ").unwrap().expect("some");
        assert_eq!(addr.port(), 80);
    }

    #[test]
    fn parse_bind_loopback_ipv6_ok() {
        let addr = parse_bind("[::1]:7432").unwrap().expect("some");
        assert_eq!(addr.ip(), IpAddr::V6(Ipv6Addr::LOCALHOST));
        assert_eq!(addr.port(), 7432);
    }

    #[test]
    fn parse_bind_invalid_address() {
        let err = parse_bind("not-a-socket-addr").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid RAG_HTTP_BIND"), "{msg}");
        assert!(msg.contains("not-a-socket-addr"), "{msg}");
    }

    #[test]
    fn parse_bind_remote_denied_without_allow() {
        let _g = ALLOW_REMOTE_LOCK.lock().unwrap();
        std::env::remove_var("RAG_HTTP_ALLOW_REMOTE");
        let err = parse_bind("0.0.0.0:7432").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not loopback"), "{msg}");
        assert!(msg.contains("RAG_HTTP_ALLOW_REMOTE"), "{msg}");
    }

    #[test]
    fn parse_bind_remote_allowed_when_env_truthy() {
        let _g = ALLOW_REMOTE_LOCK.lock().unwrap();
        for val in ["1", "true", "TRUE", "yes", "on", " Yes "] {
            std::env::set_var("RAG_HTTP_ALLOW_REMOTE", val);
            let addr = parse_bind("0.0.0.0:9000")
                .unwrap_or_else(|e| panic!("allow={val:?}: {e}"))
                .expect("some");
            assert_eq!(addr.port(), 9000);
            assert!(!addr.ip().is_loopback());
        }
        std::env::remove_var("RAG_HTTP_ALLOW_REMOTE");
    }

    #[test]
    fn parse_bind_remote_denied_when_env_non_truthy() {
        let _g = ALLOW_REMOTE_LOCK.lock().unwrap();
        for val in ["0", "false", "no", "off", "", "maybe"] {
            std::env::set_var("RAG_HTTP_ALLOW_REMOTE", val);
            let err = parse_bind("192.168.1.10:7432").unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("not loopback"),
                "allow={val:?}: {msg}"
            );
        }
        std::env::remove_var("RAG_HTTP_ALLOW_REMOTE");
    }
}
