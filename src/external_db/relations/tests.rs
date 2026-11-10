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
    let original = crate::external_db::ExternalDb::new(&store)
        .external_db_query("db_relations", &q)
        .unwrap();
    assert_eq!(original["total_inferred"], 1);
    assert!(original["items"].as_array().unwrap().is_empty());
    let mut old = s.clone();
    old.id = "old".into();
    old.capture_format = 2;
    old.inferred_relations = None;
    save(&old);
    let legacy = crate::external_db::ExternalDb::new(&store)
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
    let drift = crate::external_db::ExternalDb::new(&store)
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
    let drift = crate::external_db::ExternalDb::new(&store)
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
    let historic = crate::external_db::ExternalDb::new(&reopened)
        .external_db_query(
            "db_relations",
            &Query {
                snapshot_id: Some("capture".into()),
                ..q
            },
        )
        .unwrap();
    assert_eq!(historic["inferred"], original["inferred"]);
    assert!(crate::external_db::ExternalDb::new(&reopened)
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
