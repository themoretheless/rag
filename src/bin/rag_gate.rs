//! Opt-in PreToolUse adapter. Installation/activation is deliberately external.
use rag_mcp::{
    gate::{self, Decision, Mode},
    Config, Store,
};
use serde_json::{json, Value};
use std::io::{Read, Write};
async fn remote(config: &Config, p: &gate::Payload, mode: Mode) -> anyhow::Result<Value> {
    let mut addr: std::net::SocketAddr = config.http_bind.as_ref().unwrap().parse()?;
    if addr.ip().is_unspecified() {
        addr.set_ip(if addr.is_ipv4() {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else {
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        });
    }
    anyhow::ensure!(
        addr.ip().is_loopback(),
        "hook bridge requires a loopback address"
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut request=client.post(format!("http://{addr}/mcp")).header("accept","application/json, text/event-stream").json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"gate","arguments":{"payload":p,"mode":mode}}}));
    if let Some(token) = std::env::var("RAG_HTTP_WRITE_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("RAG_HTTP_ADMIN_TOKEN").ok())
    {
        request = request.bearer_auth(token);
    }
    let mut response = request.send().await?.error_for_status()?;
    let json_response = response
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|s| s.contains("application/json"));
    let mut bytes = Vec::new();
    let mut envelope = None;
    while let Some(chunk) = response.chunk().await? {
        bytes.extend_from_slice(&chunk);
        anyhow::ensure!(bytes.len() <= 262144, "gate response limit");
        if !json_response {
            for line in bytes.split(|b| *b == b'\n') {
                if let Some(data) = line.strip_prefix(b"data:") {
                    if let Ok(v) = serde_json::from_slice::<Value>(data) {
                        if v["id"] == 1 {
                            envelope = Some(v);
                            break;
                        }
                    }
                }
            }
            if envelope.is_some() {
                break;
            }
        }
    }
    let envelope = if json_response {
        serde_json::from_slice::<Value>(&bytes)?
    } else {
        envelope.ok_or_else(|| anyhow::anyhow!("missing MCP response"))?
    };
    anyhow::ensure!(envelope.get("error").is_none(), "MCP gate request failed");
    let result = &envelope["result"];
    anyhow::ensure!(result["isError"] != true, "gate operation failed");
    let text = result["content"]
        .as_array()
        .and_then(|c| c.iter().find_map(|v| v["text"].as_str()))
        .ok_or_else(|| anyhow::anyhow!("missing gate response text"))?;
    let mut report: Value = serde_json::from_str(text)?;
    report["hook_backend"] = json!("loopback_http");
    Ok(report)
}
async fn run(mode: Mode) -> anyhow::Result<Value> {
    let mut input = Vec::new();
    std::io::stdin().take(65537).read_to_end(&mut input)?;
    anyhow::ensure!(input.len() <= 65536, "payload limit");
    let raw: Value = serde_json::from_slice(&input)?;
    let mut p = gate::parse_payload(&raw)?;
    let anonymous = format!("hook-{}", uuid::Uuid::new_v4());
    if p.session_id.is_none() {
        p.session_id = Some(anonymous);
    }
    let config = Config::from_env()?;
    // DuckDB's live writer cannot be reopened from another process. Use the
    // running local server rather than inventing a separate hook state store.
    if config.http_bind.is_some() {
        return remote(&config, &p, mode).await;
    }
    if !config.db_path.is_file() {
        let d = Decision {
            verdict: if mode == Mode::Off { "off" } else { "allow" }.into(),
            rule: "no_local_database".into(),
            reason: "No local index database is registered at the configured path.".into(),
            context: String::new(),
            facts: json!({"corpus":null,"database_created":false}),
        };
        return Ok(
            json!({"verdict":d.verdict,"rule":d.rule,"hook":d.hook(),"command_executed":false,"hook_backend":"direct_database"}),
        );
    }
    let store = Store::open(&config.db_path)?;
    let mut report = store.analysis_gate(
        p.session_id.as_deref().unwrap(),
        &p,
        mode,
        &config.ingest_roots,
    )?;
    report["hook_backend"] = json!("direct_database");
    Ok(report)
}

#[tokio::main]
async fn main() {
    let mut mode = std::env::var("RAG_GATE_MODE").unwrap_or("warn".into());
    let mut full_json = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--json" {
            full_json = true;
        } else if arg == "--mode" {
            mode = args.next().unwrap_or_default();
        } else if let Some(m) = arg.strip_prefix("--mode=") {
            mode = m.into();
        } else {
            eprintln!("usage: rag_gate [--mode warn|ask|deny|off] [--json]");
            std::process::exit(2);
        }
    }
    let mode = match Mode::parse(&mode) {
        Ok(m) => m,
        Err(_) => {
            eprintln!("invalid gate mode");
            std::process::exit(2);
        }
    };
    let report = match run(mode).await {
        Ok(r) => r,
        Err(_) => {
            // A warn-only integration must never silently become a blocking approval flow.
            let verdict = match mode {
                Mode::Warn => "allow",
                Mode::Ask => "ask",
                Mode::Deny => "deny",
                Mode::Off => "off",
            };
            let d=Decision{verdict:verdict.into(),rule:"gate_unavailable".into(),reason:"The index-usage gate could not verify this call.".into(),context:"Gate unavailable: check its local configuration and bounded input. The proposed command was not executed.".into(),facts:json!({"fresh":null})};
            json!({"verdict":d.verdict,"rule":d.rule,"hook":d.hook(),"command_executed":false,"decision_persisted":false})
        }
    };
    if full_json {
        println!("{report}");
        return;
    }
    let hook = &report["hook"];
    print!("{}", hook["stdout"].as_str().unwrap_or(""));
    let _ = std::io::stdout().flush();
    if let Some(s) = hook["stderr"].as_str().filter(|s| !s.is_empty()) {
        eprintln!("{s}");
    }
    std::process::exit(hook["exit_code"].as_i64().unwrap_or(2) as i32);
}
