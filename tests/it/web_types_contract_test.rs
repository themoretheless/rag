//! Contract between `web-svelte/src/api/types.ts` and the Rust DTOs it mirrors.
//!
//! Each listed TS interface field is filled with a sample value, deserialized
//! into the Rust type and serialized back. A field the server never sends
//! (typo, rename, removal) disappears on the round-trip and fails the test.

use std::collections::BTreeMap;

use rag_mcp::models::{GraphEdge, GraphNode, SearchExplanation, SearchHit};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Map, Value};

type Interfaces = BTreeMap<String, Vec<(String, String)>>;

/// `name -> [(field, type)]` for top-level fields of every `export interface`.
fn parse_interfaces(src: &str) -> Interfaces {
    let mut out = Interfaces::new();
    let mut current: Option<String> = None;
    for line in src.lines() {
        if let Some(rest) = line.strip_prefix("export interface ") {
            current = rest.split_whitespace().next().map(str::to_string);
            out.entry(current.clone().unwrap()).or_default();
            continue;
        }
        if line.starts_with('}') {
            current = None;
            continue;
        }
        let Some(name) = &current else { continue };
        // Only direct members (two-space indent); skip comments and nested blocks.
        let Some(member) = line.strip_prefix("  ") else {
            continue;
        };
        if member.starts_with(' ') || member.starts_with("/*") || member.starts_with('*') {
            continue;
        }
        let Some((field, ty)) = member.split_once(':') else {
            continue;
        };
        let field = field.trim().trim_end_matches('?').to_string();
        out.get_mut(name)
            .unwrap()
            .push((field, ty.trim().to_string()));
    }
    out
}

fn sample(ty: &str, interfaces: &Interfaces) -> Value {
    let ty = ty.trim().trim_end_matches("| null").trim();
    if let Some(elem) = ty.strip_suffix("[]") {
        return json!([sample(elem, interfaces)]);
    }
    match ty {
        "string" => json!("x"),
        "number" => json!(1),
        "boolean" => json!(true),
        other if interfaces.contains_key(other) => sample_object(other, interfaces),
        other => panic!("contract test cannot sample TS type `{other}`"),
    }
}

fn sample_object(name: &str, interfaces: &Interfaces) -> Value {
    let mut obj = Map::new();
    for (field, ty) in &interfaces[name] {
        obj.insert(field.clone(), sample(ty, interfaces));
    }
    Value::Object(obj)
}

fn assert_round_trip<T: DeserializeOwned + Serialize>(name: &str, interfaces: &Interfaces) {
    let input = sample_object(name, interfaces);
    let typed: T = serde_json::from_value(input.clone())
        .unwrap_or_else(|e| panic!("TS `{name}` does not deserialize into Rust: {e}"));
    let output = serde_json::to_value(typed).unwrap();
    for (field, _) in &interfaces[name] {
        assert!(
            output.get(field).is_some(),
            "TS `{name}.{field}` is not serialized by the Rust type"
        );
    }
}

#[test]
fn web_client_types_match_rust_dtos() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/web-svelte/src/api/types.ts");
    let interfaces = parse_interfaces(&std::fs::read_to_string(path).unwrap());
    assert_round_trip::<GraphNode>("GraphNode", &interfaces);
    assert_round_trip::<GraphEdge>("GraphEdge", &interfaces);
    assert_round_trip::<SearchExplanation>("SearchExplanation", &interfaces);
    assert_round_trip::<SearchHit>("SearchHit", &interfaces);
}

#[test]
fn contract_parser_reads_fields_and_types() {
    let parsed = parse_interfaces(
        "export interface A {\n  id: string\n  /** doc */\n  n?: number | null\n  rt?: {\n    x?: string\n  }\n}\n",
    );
    let fields: Vec<_> = parsed["A"].iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(fields, ["id", "n", "rt"]);
}
