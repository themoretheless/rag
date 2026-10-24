//! Versioned, frozen naming hypotheses. These never become declared constraints.
use super::*;
use std::collections::{BTreeSet, HashMap};
const MAX_LINKS: usize = 1000;
const MAX_DIAGNOSTICS: usize = 200;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inference {
    pub algorithm_version: u32,
    pub relations: Vec<Value>,
    pub diagnostics: Vec<Value>,
    pub counts: BTreeMap<String, usize>,
    pub truncated: bool,
    pub diagnostic_truncated: bool,
}
impl Inference {
    fn count(&mut self, key: &str) {
        *self.counts.entry(key.into()).or_default() += 1;
    }
    fn diagnostic(&mut self, kind: &str, source: &LiveTable, column: Option<&str>, details: Value) {
        self.count(kind);
        if self.diagnostics.len() < MAX_DIAGNOSTICS {
            self.diagnostics.push(json!({"code":kind,"schema":source.schema,"table":source.name,"column":column,"details":details}));
        } else {
            self.diagnostic_truncated = true;
        }
    }
}
// Split underscore/CamelCase/acronym boundaries without stripping significant SQL quotes.
fn words(name: &str) -> Vec<String> {
    let mut result = vec![];
    for part in name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|p| !p.is_empty())
    {
        let b = part.as_bytes();
        let mut start = 0;
        for i in 1..b.len() {
            if b[i].is_ascii_uppercase()
                && (b[i - 1].is_ascii_lowercase()
                    || b[i - 1].is_ascii_digit()
                    || b[i - 1].is_ascii_uppercase()
                        && b.get(i + 1).is_some_and(u8::is_ascii_lowercase))
            {
                result.push(part[start..i].to_ascii_lowercase());
                start = i;
            }
        }
        result.push(part[start..].to_ascii_lowercase());
    }
    result
}
fn stems(name: &str) -> Option<(Vec<String>, &'static str)> {
    if !name.is_ascii() {
        return None;
    }
    let mut tokens = words(name);
    let rule = if tokens.last().is_some_and(|w| w == "id") {
        tokens.pop();
        "id_token_suffix"
    } else if name.bytes().all(|b| b.is_ascii_lowercase())
        && name.ends_with("id")
        && name.len() >= 5
    {
        tokens = vec![name[..name.len() - 2].into()];
        "compact_lowercase_id_suffix"
    } else {
        return None;
    };
    if tokens.is_empty() {
        return None;
    }
    let mut stems = vec![tokens.join("_")];
    if tokens.len() > 1 {
        stems.push(tokens.join(""));
        stems.push(tokens.last().unwrap().clone());
    }
    stems.sort();
    stems.dedup();
    Some((stems, rule))
}
fn variants(stem: &str) -> BTreeSet<String> {
    let mut variants = BTreeSet::from([stem.into(), format!("{stem}s"), format!("{stem}es")]);
    if let Some(base) = stem.strip_suffix('y') {
        if base
            .as_bytes()
            .last()
            .is_some_and(|c| !b"aeiou".contains(c))
        {
            variants.insert(format!("{base}ies"));
        }
    }
    if let Some(singular) = stem.strip_suffix("ies") {
        variants.insert(format!("{singular}y"));
    }
    if stem.ends_with('s') && !stem.ends_with("ss") {
        variants.insert(stem[..stem.len() - 1].into());
    }
    variants
}
pub(super) fn scalar_type_key(c: &Value) -> Option<String> {
    if c["type_schema"].as_str().is_some_and(|s| s != "pg_catalog") {
        return None;
    }
    let raw = c["data_type"].as_str()?.to_ascii_lowercase();
    let base = raw.split(['(', ' ']).next()?;
    let bits = match base {
        "tinyint" => Some(8),
        "smallint" | "int2" => Some(16),
        "mediumint" => Some(24),
        "int" | "integer" | "int4" => Some(32),
        "bigint" | "int8" => Some(64),
        _ => None,
    };
    if let Some(bits) = bits {
        return Some(format!("integer:{bits}:{}", raw.contains("unsigned")));
    }
    if raw == "uuid" {
        return Some("uuid".into());
    }
    let family = if raw.starts_with("character varying") || base == "varchar" {
        "varchar"
    } else if raw.starts_with("character(") || base == "char" || base == "character" {
        "char"
    } else if base == "text" {
        "text"
    } else if matches!(base, "numeric" | "decimal") {
        "numeric"
    } else {
        return None;
    };
    Some(format!(
        "{family}:{}:{}:{}:{}:{}",
        c["character_maximum_length"],
        c["numeric_precision"],
        c["numeric_scale"],
        c["collation_schema"],
        c["collation_name"]
    ))
}
fn type_key(c: &Value) -> Option<String> {
    if !c["resolved_type"].is_null() {
        super::types::identifier_key(c)
    } else {
        scalar_type_key(c)
    }
}
fn eligible_key(table: &LiveTable, dialect: &str, name: &str) -> Option<Value> {
    if !table.complete {
        return None;
    }
    let meta = table.metadata.as_ref()?;
    if !meta.complete {
        return None;
    }
    meta.indexes.iter().find(|i|{
        if i["unique"]!=true||i["predicate"].is_string()||i["valid"]==false||i["ready"]==false{return false;}
        if dialect=="postgresql"&&(i["valid"]!=true||i["ready"]!=true){return false;}
        let Some(keys)=i["keys"].as_array()else{return false;};
        let keys=keys.iter().filter(|k|k["included"]!=true).collect::<Vec<_>>();
        keys.len()==1&&keys[0]["column"]==name&&keys[0]["expression"].is_null()&&keys[0]["prefix_length"].is_null()&&keys[0]["included"]==false
    }).map(|i|json!({"name":i["name"],"primary":i["primary"],"unique":true,"columns":[name],"source":"native_index_metadata"}))
}
pub(super) fn infer(snapshot: &Snapshot) -> Inference {
    let mut out = Inference {
        algorithm_version: 2,
        relations: vec![],
        diagnostics: vec![],
        counts: BTreeMap::new(),
        truncated: false,
        diagnostic_truncated: false,
    };
    if snapshot
        .diagnostics
        .iter()
        .any(|d| d.starts_with("TABLE_LIMIT:") || d == "GLOBAL_TABLE_LIMIT")
    {
        out.truncated = true;
        out.count("TABLE_CATALOG_INCOMPLETE");
        return out;
    }
    let mut by_name: HashMap<(&str, String), Vec<&LiveTable>> = HashMap::new();
    for t in snapshot.tables.iter().filter(|t| t.kind == "BASE TABLE") {
        by_name
            .entry((&t.schema, t.name.to_ascii_lowercase()))
            .or_default()
            .push(t);
    }
    let mut sources = snapshot
        .tables
        .iter()
        .filter(|t| t.kind == "BASE TABLE")
        .collect::<Vec<_>>();
    sources.sort_by_key(|t| (&t.schema, &t.name));
    'tables: for source in sources {
        if !source.complete {
            out.diagnostic("SOURCE_METADATA_INCOMPLETE", source, None, Value::Null);
            continue;
        }
        let mut columns = source.columns.iter().collect::<Vec<_>>();
        columns.sort_by_key(|c| c["name"].as_str().unwrap_or(""));
        for c in columns {
            let Some(name) = c["name"].as_str() else {
                continue;
            };
            let Some((stems, rule)) = stems(name) else {
                continue;
            };
            out.count("name_candidates_examined");
            if source.foreign_keys.iter().any(|fk| fk["column"] == name) {
                out.count("already_declared");
                continue;
            }
            let candidates = stems
                .iter()
                .flat_map(|s| variants(s))
                .collect::<BTreeSet<_>>();
            let mut targets = vec![];
            for candidate in &candidates {
                if let Some(tables) = by_name.get(&(source.schema.as_str(), candidate.clone())) {
                    targets.extend(tables.iter().copied().filter(|t| t.name != source.name));
                }
            }
            targets.sort_by_key(|t| (&t.schema, &t.name));
            targets.dedup_by_key(|t| (&t.schema, &t.name));
            if targets.is_empty() {
                out.count("no_visible_name_match");
                continue;
            }
            if targets.len() != 1 {
                out.diagnostic("AMBIGUOUS_TARGET_TABLE",source,Some(name),json!({"candidates":targets.iter().map(|t|json!({"schema":t.schema,"table":t.name})).collect::<Vec<_>>()}));
                continue;
            }
            let target = targets[0];
            let ids = target
                .columns
                .iter()
                .filter(|c| {
                    c["name"]
                        .as_str()
                        .is_some_and(|n| n.eq_ignore_ascii_case("id"))
                })
                .collect::<Vec<_>>();
            if ids.len() != 1 {
                out.diagnostic(
                    "TARGET_ID_UNKNOWN_OR_AMBIGUOUS",
                    source,
                    Some(name),
                    json!({"target_schema":target.schema,"target_table":target.name}),
                );
                continue;
            }
            let id = ids[0];
            let id_name = id["name"].as_str().unwrap();
            let Some(key) = eligible_key(target, &snapshot.dialect, id_name) else {
                out.diagnostic("TARGET_ID_NOT_PROVEN_INDIVIDUALLY_UNIQUE",source,Some(name),json!({"target_schema":target.schema,"target_table":target.name,"target_column":id_name}));
                continue;
            };
            let (Some(source_type), Some(target_type)) = (type_key(c), type_key(id)) else {
                out.diagnostic(
                    "IDENTIFIER_TYPE_UNSUPPORTED",
                    source,
                    Some(name),
                    json!({"source_type":c["data_type"],"target_type":id["data_type"]}),
                );
                continue;
            };
            if source_type != target_type {
                out.diagnostic(
                    "IDENTIFIER_TYPE_MISMATCH",
                    source,
                    Some(name),
                    json!({"source_type":c["data_type"],"target_type":id["data_type"]}),
                );
                continue;
            }
            if out.relations.len() >= MAX_LINKS {
                out.truncated = true;
                out.count("INFERRED_RELATION_LIMIT");
                break 'tables;
            }
            let confidence = if rule == "compact_lowercase_id_suffix" {
                0.25
            } else if stems.len() > 1 {
                0.3
            } else {
                0.4
            };
            let identity = json!([
                2,
                snapshot.connection,
                snapshot.project,
                source.schema,
                source.name,
                name,
                target.schema,
                target.name,
                id_name
            ]);
            out.relations.push(json!({"id":crate::util::content_hash(&identity.to_string()),"schema":source.schema,"table":source.name,"kind":"name_inference","provenance":"inferred","confidence":confidence,"confidence_kind":"uncalibrated_heuristic_score","mapping":{"column":name,"target_schema":target.schema,"target_table":target.name,"target_column":id_name},"evidence":{"name_rule":rule,"name_stems":stems,"source_type":c["data_type"],"target_type":id["data_type"],"source_type_resolution":c["resolved_type"],"target_type_resolution":id["resolved_type"],"target_key":key},"not_verified":["declared_foreign_key","actual_row_values","referential_integrity","collation_or_type_coercions","runtime_usage","domain_constraints"]}));
            out.count("inferred");
        }
    }
    out
}
pub(super) fn response(s: &Snapshot, q: &Query) -> Result<Value> {
    let kind = q.relation_kind.as_deref().unwrap_or("all");
    if !matches!(kind, "all" | "declared" | "inferred") {
        return Err(AppError::config(
            "relation_kind must be all, declared or inferred",
        ));
    }
    let selected = |schema: &str, table: &str| {
        q.schema.as_ref().is_none_or(|v| v == schema) && q.table.as_ref().is_none_or(|v| v == table)
    };
    let declared:Vec<_>=s.tables.iter().filter(|t|selected(&t.schema,&t.name)).flat_map(|t|{
        let mut links=t.foreign_keys.iter().map(|f|json!({"schema":t.schema,"table":t.name,"kind":"foreign_key","provenance":"declared","confidence":1.0,"confidence_kind":"catalog_declaration","mapping":f})).collect::<Vec<_>>();
        if let Some(v)=t.metadata.as_ref().and_then(|m|m.view.as_ref()){links.extend(v.dependencies.iter().map(|d|json!({"schema":t.schema,"table":t.name,"kind":"view_dependency","provenance":"declared","confidence":1.0,"confidence_kind":"catalog_declaration","mapping":d})));}links
    }).collect();
    let inferred = s
        .inferred_relations
        .as_ref()
        .map(|a| {
            a.relations
                .iter()
                .filter(|r| {
                    selected(
                        r["schema"].as_str().unwrap_or(""),
                        r["table"].as_str().unwrap_or(""),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let diagnostics = s
        .inferred_relations
        .as_ref()
        .map(|a| {
            a.diagnostics
                .iter()
                .filter(|r| {
                    selected(
                        r["schema"].as_str().unwrap_or(""),
                        r["table"].as_str().unwrap_or(""),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let declared_shown = if kind == "inferred" {
        vec![]
    } else {
        declared.iter().take(limit).collect::<Vec<_>>()
    };
    let inferred_shown = if kind == "declared" {
        vec![]
    } else {
        inferred.iter().take(limit).copied().collect::<Vec<_>>()
    };
    Ok(
        json!({"snapshot_id":s.id,"connection":s.connection,"project":s.project,"dialect":s.dialect,"captured_ms":s.captured_ms,"relation_kind":kind,"capture_format":s.capture_format,"code_index_revision_at_capture":s.code_index_revision,"items":declared_shown,"declared":declared_shown,"total":declared.len(),"truncated":kind!="inferred"&&declared.len()>limit,"inferred":inferred_shown,"total_inferred":inferred.len(),"inferred_truncated":kind!="declared"&&inferred.len()>limit,"inference_available":s.inferred_relations.is_some(),"inference":s.inferred_relations.as_ref().map(|a|json!({"algorithm_version":a.algorithm_version,"counts_in_entire_capture":a.counts,"analysis_truncated":a.truncated,"diagnostics":diagnostics.iter().take(limit).copied().collect::<Vec<_>>(),"total_visible_diagnostics":diagnostics.len(),"diagnostics_truncated":a.diagnostic_truncated||diagnostics.len()>limit,"scope":"captured same-schema base tables, scalar identifier types and proven single-column unique id keys; guesses are not constraints"})),"inference_unavailable_reason":if s.inferred_relations.is_none(){Some("legacy_capture_requires_refresh")}else{None},"complete_within_visible_scope":s.complete_within_visible_scope,"diagnostics":s.diagnostics,"live_query":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn column(name: &str, kind: &str) -> Value {
        json!({"name":name,"data_type":kind,"type_schema":"pg_catalog"})
    }
    fn table(schema: &str, name: &str, columns: Vec<Value>, keys: &[&str]) -> LiveTable {
        LiveTable {
            schema: schema.into(),
            name: name.into(),
            kind: "BASE TABLE".into(),
            columns,
            foreign_keys: vec![],
            complete: true,
            sample: Value::Null,
            metadata: Some(metadata::TableMetadata {
                types: None,
                indexes: if keys.is_empty() {
                    vec![]
                } else {
                    vec![
                        json!({"name":"pk","primary":true,"unique":true,"valid":true,"ready":true,"keys":keys.iter().map(|n|json!({"column":n,"included":false})).collect::<Vec<_>>()}),
                    ]
                },
                constraints: vec![],
                view: None,
                complete: true,
                diagnostics: vec![],
            }),
        }
    }
    fn snapshot(tables: Vec<LiveTable>) -> Snapshot {
        Snapshot {
            capture_format: 3,
            id: "capture".into(),
            connection: "alias".into(),
            project: "p".into(),
            dialect: "postgresql".into(),
            schemas: vec!["app".into()],
            captured_ms: 0,
            code_index_revision: None,
            target_fingerprint: "scope".into(),
            tables,
            complete_within_visible_scope: true,
            diagnostics: vec![],
            inferred_relations: None,
        }
    }
    #[test]
    fn naming_rules_use_real_keys_and_type_evidence_without_promoting_guesses() {
        let s = snapshot(vec![
            table("app", "users", vec![column("id", "integer")], &["id"]),
            table("app", "companies", vec![column("id", "integer")], &["id"]),
            table("app", "pas", vec![column("id", "integer")], &["id"]),
            table(
                "app",
                "events",
                [
                    "user_id",
                    "UserID",
                    "User_ID",
                    "AuthorUserID",
                    "userid",
                    "company_id",
                    "paid",
                    "id",
                ]
                .into_iter()
                .map(|n| column(n, "integer"))
                .collect(),
                &[],
            ),
        ]);
        let a = infer(&s);
        assert_eq!(a.relations.len(), 6, "{a:?}");
        for r in &a.relations {
            assert_eq!(r["provenance"], "inferred");
            assert!(r["confidence"].as_f64().unwrap() <= 0.4);
            assert_eq!(r["evidence"]["target_key"]["unique"], true);
            assert_eq!(r["mapping"]["target_schema"], "app");
        }
        assert_eq!(
            a.relations
                .iter()
                .find(|r| r["mapping"]["column"] == "company_id")
                .unwrap()["mapping"]["target_table"],
            "companies"
        );
        assert!(!a
            .relations
            .iter()
            .any(|r| r["mapping"]["column"] == "paid" || r["mapping"]["column"] == "id"));
        let mut reversed = s.clone();
        reversed.tables.reverse();
        for t in &mut reversed.tables {
            t.columns.reverse();
        }
        assert_eq!(
            serde_json::to_value(a).unwrap(),
            serde_json::to_value(infer(&reversed)).unwrap()
        );
    }
    #[test]
    fn schema_case_and_role_prefix_ambiguity_never_selects_the_first_candidate() {
        let mut s = snapshot(vec![
            table(
                "app",
                "events",
                vec![column("AuthorUserID", "integer")],
                &[],
            ),
            table("app", "users", vec![column("id", "integer")], &["id"]),
            table("audit", "users", vec![column("id", "integer")], &["id"]),
        ]);
        assert_eq!(infer(&s).relations.len(), 1);
        s.tables.push(table(
            "app",
            "Users",
            vec![column("id", "integer")],
            &["id"],
        ));
        let a = infer(&s);
        assert!(a.relations.is_empty());
        assert_eq!(a.diagnostics[0]["code"], "AMBIGUOUS_TARGET_TABLE");
        assert_eq!(
            a.diagnostics[0]["details"]["candidates"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        s.tables.pop();
        s.tables.push(table(
            "app",
            "author_users",
            vec![column("id", "integer")],
            &["id"],
        ));
        assert!(infer(&s).relations.is_empty());
        s.tables.retain(|t| t.schema != "app" || t.name == "events");
        assert!(
            infer(&s).relations.is_empty(),
            "cross-schema fallback must not guess a search path"
        );
        let mut quoted = snapshot(vec![
            table("app", "events", vec![column("user_id", "integer")], &[]),
            table("app", "users", vec![column("ID", "integer")], &["ID"]),
        ]);
        assert_eq!(
            infer(&quoted).relations[0]["mapping"]["target_column"],
            "ID"
        );
        quoted.tables[1].columns.push(column("id", "integer"));
        let a = infer(&quoted);
        assert!(a.relations.is_empty());
        assert_eq!(a.diagnostics[0]["code"], "TARGET_ID_UNKNOWN_OR_AMBIGUOUS");
    }
    #[test]
    fn composite_partial_invalid_unknown_and_declared_targets_are_not_fabricated() {
        let mut s = snapshot(vec![
            table("app", "events", vec![column("user_id", "integer")], &[]),
            table(
                "app",
                "users",
                vec![column("id", "integer"), column("tenant", "integer")],
                &["id", "tenant"],
            ),
        ]);
        assert_eq!(
            infer(&s).diagnostics[0]["code"],
            "TARGET_ID_NOT_PROVEN_INDIVIDUALLY_UNIQUE"
        );
        s.tables[1].metadata.as_mut().unwrap().indexes[0]["keys"] =
            json!([{"column":"id","included":false}]);
        s.tables[1].metadata.as_mut().unwrap().indexes[0]["predicate"] = json!("id>0");
        assert!(infer(&s).relations.is_empty());
        s.tables[1].metadata.as_mut().unwrap().indexes[0]["predicate"] = Value::Null;
        s.tables[1].metadata.as_mut().unwrap().indexes[0]["valid"] = json!(false);
        assert!(infer(&s).relations.is_empty());
        s.tables[1].metadata.as_mut().unwrap().indexes[0]["valid"] = json!(true);
        s.tables[0].columns[0]["data_type"] = json!("uuid");
        assert_eq!(infer(&s).diagnostics[0]["code"], "IDENTIFIER_TYPE_MISMATCH");
        s.tables[0].columns[0]["data_type"] = json!("public.custom_id");
        assert_eq!(
            infer(&s).diagnostics[0]["code"],
            "IDENTIFIER_TYPE_UNSUPPORTED"
        );
        s.tables[0].columns[0]["data_type"] = json!("integer");
        s.tables[0].foreign_keys.push(json!({"column":"user_id"}));
        let a = infer(&s);
        assert!(a.relations.is_empty());
        assert_eq!(a.counts["already_declared"], 1);
        s.tables[0].foreign_keys.clear();
        s.diagnostics.push("TABLE_LIMIT:app".into());
        let a = infer(&s);
        assert!(a.truncated);
        assert!(a.relations.is_empty());
    }
    #[test]
    fn analysis_and_diagnostic_budgets_are_reported_explicitly() {
        let mut tables = vec![table(
            "app",
            "users",
            vec![column("id", "integer")],
            &["id"],
        )];
        for source in 0..5 {
            tables.push(table(
                "app",
                &format!("events{source}"),
                (0..210)
                    .map(|n| column(&format!("Role{n}UserID"), "integer"))
                    .collect(),
                &[],
            ));
        }
        let a = infer(&snapshot(tables));
        assert_eq!(a.relations.len(), 1000);
        assert!(a.truncated);
        let s = snapshot(vec![
            table("app", "users", vec![column("id", "integer")], &["id"]),
            table(
                "app",
                "events",
                (0..230)
                    .map(|n| column(&format!("Role{n}UserID"), "boolean"))
                    .collect(),
                &[],
            ),
        ]);
        let a = infer(&s);
        assert_eq!(a.diagnostics.len(), 200);
        assert!(a.diagnostic_truncated);
        assert_eq!(a.counts["IDENTIFIER_TYPE_UNSUPPORTED"], 230);
    }
    #[test]
    fn persisted_hypotheses_and_legacy_formats_are_not_recomputed_on_read() {
        let mut s = snapshot(vec![
            table("app", "events", vec![column("user_id", "integer")], &[]),
            table("app", "users", vec![column("id", "integer")], &["id"]),
        ]);
        s.inferred_relations = Some(infer(&s));
        s.validate_format().unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("frozen.duckdb");
        let store = Store::open(&path).unwrap();
        let save = |s: &Snapshot| {
            store.lock().unwrap().execute("INSERT INTO external_db_snapshots (id,connection_alias,captured_ms,payload) VALUES (?,?,?,?)",duckdb::params![s.id,s.connection,s.captured_ms,serde_json::to_string(s).unwrap()]).unwrap();
        };
        save(&s);
        let q = Query {
            connection: "alias".into(),
            table: Some("events".into()),
            ..Default::default()
        };
        let original = store.external_db_query("db_relations", &q).unwrap();
        assert_eq!(original["total_inferred"], 1);
        assert!(original["items"].as_array().unwrap().is_empty());
        let mut old = s.clone();
        old.id = "old".into();
        old.capture_format = 2;
        old.inferred_relations = None;
        save(&old);
        let legacy = store
            .external_db_query(
                "db_relations",
                &Query {
                    snapshot_id: Some("old".into()),
                    ..Query {
                        connection: "alias".into(),
                        ..Default::default()
                    }
                },
            )
            .unwrap();
        assert_eq!(legacy["inference_available"], false);
        let drift = store
            .external_db_query(
                "db_drift",
                &Query {
                    connection: "alias".into(),
                    snapshot_id: Some("capture".into()),
                    base_snapshot_id: Some("old".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(drift["compared"], true);
        assert_eq!(drift["total"], 0);
        old.id = "analysis-only".into();
        old.capture_format = 3;
        old.inferred_relations = Some(infer(&old));
        old.inferred_relations.as_mut().unwrap().relations.clear();
        save(&old);
        let drift = store
            .external_db_query(
                "db_drift",
                &Query {
                    connection: "alias".into(),
                    base_snapshot_id: Some("capture".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(drift["total"], 0, "hypotheses are never DDL facts");
        drop(store);
        let reopened = Store::open(&path).unwrap();
        let historic = reopened
            .external_db_query(
                "db_relations",
                &Query {
                    snapshot_id: Some("capture".into()),
                    ..q
                },
            )
            .unwrap();
        assert_eq!(historic["inferred"], original["inferred"]);
        assert!(reopened
            .external_db_query(
                "db_relations",
                &Query {
                    connection: "alias".into(),
                    relation_kind: Some("bad".into()),
                    ..Default::default()
                }
            )
            .is_err());
    }
}
