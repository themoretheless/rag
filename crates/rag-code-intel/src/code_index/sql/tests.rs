use super::*;
fn snap(files: &[(&str, &str, Option<&str>)]) -> CodeSnapshot {
    let mut docs = HashMap::new();
    let mut out = CodeSnapshot::default();
    for (i, (path, source, project)) in files.iter().enumerate() {
        let mut doc = parse(&i.to_string(), path, source, 1).unwrap();
        doc.project = project.map(str::to_string);
        out.symbols.extend(doc.symbols.clone());
        docs.insert(i.to_string(), doc);
    }
    out.sql = snapshot(&docs);
    out
}
fn check(table: &str, operation: &str, columns: &[&str]) -> WriteCheck {
    WriteCheck {
        table: table.into(),
        project: None,
        operation: Some(operation.into()),
        columns: columns.iter().map(|s| s.to_string()).collect(),
        values: None,
        default_columns: vec![],
    }
}
#[test]
fn migration_checks_validate_literal_rows_nulls_and_unknown_assignments() {
    let s=snap(&[("/p/migrations/001.sql","CREATE TABLE users(age INT CONSTRAINT positive CHECK(AGE>0), label TEXT, CONSTRAINT small CHECK(age<100));",None)]);
    let mut q = check("users", "insert", &[]);
    q.values = Some(BTreeMap::from([("age".into(), serde_json::json!(7))]));
    let r = s.write_check(&q).unwrap();
    assert_eq!(r["status"], "valid", "{r}");
    assert_eq!(r["constraint_checks"].as_array().unwrap().len(), 2);
    q.values = Some(BTreeMap::from([("age".into(), serde_json::json!(-1))]));
    let r = s.write_check(&q).unwrap();
    assert_eq!(r["status"], "invalid");
    assert!(r["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["code"] == "CHECK_CONSTRAINT_VIOLATION"));
    q.values = Some(BTreeMap::from([("age".into(), serde_json::Value::Null)]));
    assert_eq!(s.write_check(&q).unwrap()["status"], "valid");
    q.values = Some(BTreeMap::new());
    q.columns = vec!["age".into()];
    assert_eq!(s.write_check(&q).unwrap()["status"], "indeterminate");
    q.values = None;
    assert_eq!(s.write_check(&q).unwrap()["status"], "valid");
}
#[test]
fn migration_check_defaults_updates_quoted_identifiers_and_functions_stay_honest() {
    let s = snap(&[(
        "/p/migrations/001.sql",
        "CREATE TABLE x(a INT DEFAULT 3 CHECK(a>0), b INT, CONSTRAINT ordered CHECK(a<b));",
        None,
    )]);
    let mut q = check("x", "insert", &[]);
    q.values = Some(BTreeMap::from([("b".into(), serde_json::json!(4))]));
    assert_eq!(s.write_check(&q).unwrap()["status"], "valid");
    q.operation = Some("update".into());
    q.values = Some(BTreeMap::from([("a".into(), serde_json::json!(1))]));
    assert_eq!(s.write_check(&q).unwrap()["status"], "indeterminate");
    let s = snap(&[(
        "/p/migrations/001.sql",
        r#"CREATE TABLE t("Camel" INT CHECK("Camel">0));"#,
        None,
    )]);
    let mut q = check("t", "insert", &[]);
    q.values = Some(BTreeMap::from([(
        r#""Camel""#.into(),
        serde_json::json!(-1),
    )]));
    assert_eq!(s.write_check(&q).unwrap()["status"], "invalid");
    let s = snap(&[(
        "/p/migrations/001.sql",
        "CREATE TABLE t(a TEXT CHECK(dangerous_function(a)));",
        None,
    )]);
    q.values = Some(BTreeMap::from([(
        "a".into(),
        serde_json::json!("PRIVATE_ROW_LITERAL"),
    )]));
    let result = s.write_check(&q).unwrap();
    assert_eq!(result["status"], "indeterminate");
    assert!(!result.to_string().contains("PRIVATE_ROW_LITERAL"));
}
#[test]
fn explicit_defaults_work_on_insert_update_and_reject_conflicting_values() {
    let s = snap(&[(
        "/p/migrations/001.sql",
        "CREATE TABLE t(a INT DEFAULT 7 CHECK(a>5));",
        None,
    )]);
    let mut q = check("t", "insert", &["a"]);
    q.default_columns = vec!["a".into()];
    assert_eq!(s.write_check(&q).unwrap()["status"], "valid");
    q.operation = Some("update".into());
    assert_eq!(s.write_check(&q).unwrap()["status"], "valid");
    q.values = Some(BTreeMap::from([("a".into(), serde_json::json!(1))]));
    assert!(s.write_check(&q).is_err());
    q.values = None;
    q.default_columns.push("A".into());
    assert_eq!(s.write_check(&q).unwrap()["status"], "invalid");
    q.default_columns = vec!["missing".into()];
    assert_eq!(s.write_check(&q).unwrap()["status"], "indeterminate");
}
#[test]
fn explicit_defaults_distinguish_null_implicit_and_generated_contexts() {
    let mut q = check("t", "insert", &[]);
    q.default_columns = vec!["a".into()];
    let s = snap(&[(
        "/p/migrations/001.sql",
        "CREATE TABLE t(a INT CHECK(a IS NULL));",
        None,
    )]);
    assert_eq!(s.write_check(&q).unwrap()["status"], "valid");
    let s = snap(&[(
        "/p/migrations/001.sql",
        "CREATE TABLE t(a INT NOT NULL);",
        None,
    )]);
    assert_eq!(s.write_check(&q).unwrap()["status"], "indeterminate");
    let s = snap(&[(
        "/p/migrations/001.sql",
        "CREATE TABLE t(a INT, b INT GENERATED ALWAYS AS (a+1) STORED);",
        None,
    )]);
    q.default_columns = vec!["b".into()];
    q.values = Some(BTreeMap::from([("a".into(), serde_json::json!(1))]));
    assert_eq!(s.write_check(&q).unwrap()["status"], "indeterminate");
}

#[test]
fn literal_defaults_feed_checks_without_echo_or_unknown_assignment_override() {
    let s=snap(&[("/p/migrations/001.sql","CREATE TABLE t(a INT NOT NULL DEFAULT 3 CHECK(a<5), b SMALLINT DEFAULT -7, flag BOOLEAN DEFAULT TRUE CHECK(flag), tag TEXT DEFAULT 'PRIVATE_DEFAULT_LITERAL' CHECK(length(tag)>0));",None)]);
    let mut q = check("t", "insert", &[]);
    q.values = Some(BTreeMap::new());
    let result = s.write_check(&q).unwrap();
    assert_eq!(result["status"], "valid", "{result}");
    assert_eq!(
        result["inferred_default_columns"].as_array().unwrap().len(),
        4
    );
    assert!(!result.to_string().contains("PRIVATE_DEFAULT_LITERAL"));
    q.columns = vec!["a".into()];
    assert_eq!(s.write_check(&q).unwrap()["status"], "indeterminate");
    q.columns.clear();
    q.operation = Some("update".into());
    q.values = Some(BTreeMap::from([("flag".into(), serde_json::json!(true))]));
    let result = s.write_check(&q).unwrap();
    assert_eq!(result["status"], "indeterminate");
    assert_eq!(result["inferred_default_columns"], serde_json::json!([]));
}
#[test]
fn default_null_and_unknown_functions_types_and_context_budget_are_explicit() {
    let mut q = check("t", "insert", &[]);
    q.values = Some(BTreeMap::new());
    let s = snap(&[(
        "/p/migrations/001.sql",
        "CREATE TABLE t(a INT NOT NULL DEFAULT NULL);",
        None,
    )]);
    assert_eq!(s.write_check(&q).unwrap()["status"], "invalid");
    for ddl in [
        "CREATE TABLE t(a INT DEFAULT next_value() CHECK(a>0));",
        "CREATE TABLE t(a SMALLINT DEFAULT 40000 CHECK(a>0));",
        "CREATE TABLE t(a INT DEFAULT '7' CHECK(a>0));",
    ] {
        let s = snap(&[("/p/migrations/001.sql", ddl, None)]);
        let r = s.write_check(&q).unwrap();
        assert_eq!(r["status"], "indeterminate", "{r}");
    }
    let text = format!(
        "CREATE TABLE t({});",
        (0..17)
            .map(|i| format!("c{i} TEXT DEFAULT '{}'", "x".repeat(4096)))
            .collect::<Vec<_>>()
            .join(",")
    );
    let s = snap(&[("/p/migrations/001.sql", &text, None)]);
    let r = s.write_check(&q).unwrap();
    assert_eq!(r["status"], "indeterminate");
    assert!(r["unverified"]
        .as_array()
        .unwrap()
        .iter()
        .any(|u| u["code"] == "ROW_CONTEXT_BUDGET_EXHAUSTED"));
}

#[test]
fn migration_check_not_valid_is_checked_and_budgets_fail_closed() {
    let s = snap(&[(
        "/p/migrations/001.sql",
        "CREATE TABLE t(a INT); ALTER TABLE t ADD CONSTRAINT c CHECK(a>0) NOT VALID;",
        None,
    )]);
    let mut q = check("t", "insert", &[]);
    q.values = Some(BTreeMap::from([("a".into(), serde_json::json!(0))]));
    assert_eq!(s.write_check(&q).unwrap()["status"], "invalid");
    let checks = (0..513)
        .map(|i| format!("CONSTRAINT c{i} CHECK(a>0)"))
        .collect::<Vec<_>>()
        .join(",");
    let text = format!("CREATE TABLE t(a INT,{checks});");
    let s = snap(&[("/p/migrations/001.sql", &text, None)]);
    q.values = Some(BTreeMap::from([("a".into(), serde_json::json!(1))]));
    let r = s.write_check(&q).unwrap();
    assert_eq!(r["status"], "indeterminate");
    assert!(r["unverified"]
        .as_array()
        .unwrap()
        .iter()
        .any(|u| u["code"] == "CHECK_BUDGET_EXHAUSTED"));
    q.values = Some(BTreeMap::from([(
        "a".into(),
        serde_json::json!("x".repeat(70000)),
    )]));
    assert!(s.write_check(&q).is_err());
}

#[test]
fn constraints_capture_column_table_add_drop_and_not_valid() {
    let create="CREATE TABLE parent(id INT PRIMARY KEY); CREATE TABLE child(id INT PRIMARY KEY, amount INT CONSTRAINT positive CHECK(amount > 0), parent_id INT REFERENCES parent(id), CONSTRAINT uq UNIQUE(amount), CONSTRAINT ceiling CHECK(amount < 100));";
    let snap=snap(&[("/p/migrations/001.sql",create,None),("/p/migrations/002.sql","ALTER TABLE child ADD CONSTRAINT floor CHECK(amount > 1) NOT VALID; ALTER TABLE child DROP CONSTRAINT positive;",None)]);
    let child = snap.sql.tables.iter().find(|t| t.name == "child").unwrap();
    assert!(child.complete, "{:?}", snap.sql.diagnostics);
    assert!(child.columns["amount"].constraints.is_empty());
    assert_eq!(child.columns["id"].constraints[0].kind, "primary_key");
    assert_eq!(
        child.columns["parent_id"].constraints[0].kind,
        "foreign_key"
    );
    assert!(child.constraints.iter().any(|c| c.kind == "unique"));
    assert!(child
        .constraints
        .iter()
        .any(|c| c.name.as_deref() == Some("floor") && c.not_valid));
}
#[test]
fn constraint_dependency_rewrites_and_duplicate_names_are_diagnosed() {
    let s=snap(&[("/p/migrations/001.sql","CREATE TABLE parent(id INT PRIMARY KEY); CREATE TABLE child(parent_id INT REFERENCES parent(id)); ALTER TABLE parent RENAME COLUMN id TO renamed;",None)]);
    assert!(s.sql.tables.iter().all(|t| !t.complete));
    assert!(s
        .sql
        .diagnostics
        .iter()
        .any(|d| d["code"] == "SCHEMA_DEPENDENCY_REWRITE_UNVERIFIED"));
    let s = snap(&[(
        "/p/migrations/001.sql",
        "CREATE TABLE x(a INT, CONSTRAINT c CHECK(a>0), CONSTRAINT c UNIQUE(a));",
        None,
    )]);
    assert!(!s.sql.tables[0].complete);
    assert!(s
        .sql
        .diagnostics
        .iter()
        .any(|d| d["code"] == "DUPLICATE_CONSTRAINT_NAME"));
    let s=snap(&[("/p/migrations/001.sql","CREATE TABLE x(a INT CONSTRAINT c CHECK(a>0)); ALTER TABLE x ADD COLUMN b INT CONSTRAINT c CHECK(b>0);",None)]);
    assert!(!s.sql.tables[0].complete);
    assert!(s
        .sql
        .diagnostics
        .iter()
        .any(|d| d["code"] == "DUPLICATE_CONSTRAINT_NAME"));
}

#[test]
fn explicit_default_and_generated_expressions_survive_migration_replay() {
    let create = "CREATE TABLE calc(a INT DEFAULT 1, b INT GENERATED ALWAYS AS (a + 1) STORED);";
    let initial = snap(&[("/p/migrations/001.sql", create, None)]);
    let columns = &initial.sql.tables[0].columns;
    assert_eq!(columns["a"].default_expression.as_deref(), Some("1"));
    assert!(columns["b"]
        .generated_expression
        .as_deref()
        .unwrap()
        .contains("a + 1"));
    assert!(columns["b"]
        .generation_clause
        .as_deref()
        .unwrap()
        .contains("STORED"));
    let altered = snap(&[
        ("/p/migrations/001.sql", create, None),
        (
            "/p/migrations/002.sql",
            "ALTER TABLE calc ALTER COLUMN a SET DEFAULT 2;",
            None,
        ),
    ]);
    assert_eq!(
        altered.sql.tables[0].columns["a"]
            .default_expression
            .as_deref(),
        Some("2")
    );
    let dropped = snap(&[
        ("/p/migrations/001.sql", create, None),
        (
            "/p/migrations/002.sql",
            "ALTER TABLE calc ALTER COLUMN a DROP DEFAULT;",
            None,
        ),
    ]);
    assert_eq!(dropped.sql.tables[0].columns["a"].default_expression, None);
    assert!(!dropped.sql.tables[0].columns["a"].has_default);
}

#[test]
fn sql_migrations_replay_alters_defaults_renames_drops_and_rollback_markers() {
    let snap=snap(&[
            ("/p/migrations/001.sql","CREATE TABLE users (id SERIAL, name TEXT NOT NULL, old INT); CREATE TABLE doomed(id INT);",None),
            ("/p/migrations/002.sql","ALTER TABLE users ADD COLUMN email TEXT DEFAULT 'n/a'; ALTER TABLE users RENAME COLUMN name TO display_name; ALTER TABLE users DROP COLUMN old; DROP TABLE doomed;",None),
            ("/p/migrations/003.sql","ALTER TABLE users ALTER COLUMN email SET NOT NULL; ALTER TABLE users ALTER COLUMN display_name SET DEFAULT 'unknown';\n-- +goose Down\nDROP TABLE users;",None),
            ("/p/migrations/004.down.sql","DROP TABLE users;",None),
        ]);
    assert_eq!(snap.sql.tables.len(), 1, "{:?}", snap.sql);
    let table = &snap.sql.tables[0];
    assert!(table.complete);
    assert_eq!(table.columns.len(), 3);
    assert!(!table.columns["email"].nullable);
    assert!(table.columns["email"].has_default);
    assert!(table.columns["display_name"].has_default);
    assert_eq!(
        snap.write_check(&check("users", "insert", &[])).unwrap()["status"],
        "valid"
    );
    assert!(table
        .evidence
        .iter()
        .all(|e| e.revision == 1 && !e.content_hash.is_empty()));
}
#[test]
fn sql_table_usage_tracks_joins_ctes_subqueries_insert_update_delete() {
    let snap=snap(&[("/p/query.sql","WITH recent AS (SELECT * FROM users) SELECT * FROM recent JOIN orders o ON true WHERE EXISTS (SELECT 1 FROM logs); INSERT INTO audit SELECT id FROM users; UPDATE users SET name='x' WHERE id IN (SELECT id FROM flags); DELETE FROM orders WHERE id=1;",None)]);
    let names: HashSet<_> = snap.sql.usages.iter().map(|u| u.table.as_str()).collect();
    assert!(!names.contains("recent"));
    for name in ["users", "orders", "logs", "audit", "flags"] {
        assert!(names.contains(name), "{name}: {:?}", snap.sql.usages);
    }
    for (table, operation) in [
        ("audit", "insert"),
        ("users", "update"),
        ("orders", "delete"),
    ] {
        assert!(snap
            .sql
            .usages
            .iter()
            .any(|u| u.table == table && u.mode == "write" && u.operation == operation));
    }
    let result = snap
        .sql_query(
            "table_usage",
            &SqlQuery {
                table: Some("users".into()),
                mode: Some("write".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(result["total"], 1);
}
#[test]
fn sql_merge_reads_target_source_and_subqueries_but_writes_only_mutating_actions() {
    let snap=snap(&[("/p/merge.sql","MERGE INTO target t USING staging s ON t.id=s.id WHEN MATCHED THEN UPDATE SET value=s.value WHEN NOT MATCHED THEN INSERT (id,value) VALUES (s.id,s.value); MERGE INTO untouched t USING (SELECT * FROM incoming) s ON t.id=s.id WHEN MATCHED THEN DO NOTHING;",None)]);
    let uses = &snap.sql.usages;
    for table in ["target", "staging", "untouched", "incoming"] {
        assert!(
            uses.iter()
                .any(|u| u.table == table && u.mode == "read" && u.operation == "merge"),
            "{uses:?}"
        );
    }
    assert!(uses
        .iter()
        .any(|u| u.table == "target" && u.mode == "write" && u.operation == "merge"));
    assert!(!uses
        .iter()
        .any(|u| u.table == "untouched" && u.mode == "write"));
    assert!(!snap
        .sql
        .diagnostics
        .iter()
        .any(|d| d["code"] == "UNSUPPORTED_WRITE_OPERATION"));
}
#[test]
fn sql_truncate_has_explicit_writes_and_copy_direction_is_not_reversed() {
    let snap=snap(&[("/p/ops.sql","TRUNCATE TABLE ONLY a, ONLY b; COPY inbound FROM '/tmp/not-read.csv'; COPY outbound TO STDOUT; COPY (WITH q AS (SELECT * FROM origin) SELECT * FROM q) TO STDOUT;",None)]);
    let uses = &snap.sql.usages;
    for table in ["a", "b"] {
        assert!(
            uses.iter()
                .any(|u| u.table == table && u.mode == "write" && u.operation == "truncate"),
            "{uses:?}"
        );
        assert!(!uses.iter().any(|u| u.table == table && u.mode == "read"));
    }
    assert!(uses
        .iter()
        .any(|u| u.table == "inbound" && u.mode == "write" && u.operation == "copy_from"));
    assert!(!uses
        .iter()
        .any(|u| u.table == "inbound" && u.mode == "read"));
    for table in ["outbound", "origin"] {
        assert!(
            uses.iter()
                .any(|u| u.table == table && u.mode == "read" && u.operation == "copy_to"),
            "{uses:?}"
        );
        assert!(!uses.iter().any(|u| u.table == table && u.mode == "write"));
    }
    assert!(!uses.iter().any(|u| u.table == "q"));
}
#[test]
fn sql_new_operations_keep_static_owner_lines_and_dynamic_exclusions() {
    let snap = snap(&[
            (
                "/p/a.rs",
                "fn bulk() {\n let q = r#\"COPY export_rows TO STDOUT\"#;\n let label = \"copy\"; let q2 = \"COPY(SELECT * FROM another) TO STDOUT\";\n}\n",
                None,
            ),
            (
                "/p/a.ts",
                "function purge(t) { const sql = `TRUNCATE TABLE ${t}`; }",
                None,
            ),
            ("/p/truncate.sql", "TRUNCATE TABLE parent CASCADE;", None),
        ]);
    let copy = snap
        .sql
        .usages
        .iter()
        .find(|u| u.table == "export_rows")
        .unwrap();
    assert_eq!(copy.mode, "read");
    assert_eq!(copy.operation, "copy_to");
    assert_eq!(copy.evidence.line, 2);
    assert!(copy.symbol.is_some());
    assert!(snap
        .sql
        .usages
        .iter()
        .any(|u| u.table == "another" && u.operation == "copy_to"));
    assert!(!snap
        .sql
        .diagnostics
        .iter()
        .any(|d| d["code"] == "SQL_PARSE_ERROR"));
    assert!(snap
        .sql
        .diagnostics
        .iter()
        .any(|d| d["code"] == "DYNAMIC_OR_UNSUPPORTED_SQL_LITERAL"));
    assert!(snap
        .sql
        .diagnostics
        .iter()
        .any(|d| d["code"] == "TRUNCATE_DESCENDANTS_OR_CASCADE_UNVERIFIED"));
    assert!(!snap.sql.usages.iter().any(|u| u.table.contains("${")));
}
#[test]
fn sql_merge_output_into_records_secondary_target_and_column_list_without_fake_read() {
    let snap=snap(&[("/p/merge.sql","MERGE INTO target t USING staging s ON t.id=s.id WHEN MATCHED THEN UPDATE SET value=s.value OUTPUT inserted.id INTO dbo.audit(id);",None)]);
    assert!(
        snap.sql
            .usages
            .iter()
            .any(|u| u.table == "target" && u.mode == "write" && u.operation == "merge"),
        "{:?}",
        snap.sql
    );
    assert!(
        snap.sql.usages.iter().any(|u| u.table == "dbo.audit"
            && u.mode == "write"
            && u.operation == "merge_output_into"),
        "{:?}",
        snap.sql
    );
    assert!(!snap
        .sql
        .usages
        .iter()
        .any(|u| u.table == "dbo.audit" && u.mode == "read"));
    assert!(
        !snap
            .sql
            .diagnostics
            .iter()
            .any(|d| d["code"] == "MERGE_OUTPUT_CLAUSE_UNVERIFIED"
                || d["code"] == "SQL_INTO_TARGET_UNRESOLVED"),
        "{:?}",
        snap.sql
    );
}
#[test]
fn sql_output_into_covers_insert_update_delete_and_returning_has_no_extra_write() {
    let snap=snap(&[("/p/output.sql","INSERT INTO target(a) OUTPUT inserted.a INTO insert_log(a) VALUES (1); UPDATE target SET a=2 OUTPUT inserted.a INTO update_log; DELETE FROM target OUTPUT deleted.a INTO delete_log; INSERT INTO plain(a) VALUES (1) RETURNING a;",None)]);
    for (table, operation) in [
        ("insert_log", "insert_output_into"),
        ("update_log", "update_output_into"),
        ("delete_log", "delete_output_into"),
    ] {
        assert!(
            snap.sql
                .usages
                .iter()
                .any(|u| u.table == table && u.mode == "write" && u.operation == operation),
            "{table}: {:?}",
            snap.sql
        );
    }
    assert_eq!(
        snap.sql
            .usages
            .iter()
            .filter(|u| u.operation.ends_with("output_into"))
            .count(),
        3
    );
    assert!(!snap
        .sql
        .usages
        .iter()
        .any(|u| u.table == "plain" && u.operation.ends_with("output_into")));
}
#[test]
fn sql_select_into_preserves_sources_and_quoted_targets_but_not_variables() {
    let snap=snap(&[("/p/into.sql","SELECT id INTO derived FROM original; WITH q AS (SELECT * FROM base) SELECT id INTO \"Archive\" FROM q; SELECT id INTO @capture FROM original; SELECT id,id INTO first_variable, second_variable FROM original;",None)]);
    for table in ["derived", "\"Archive\""] {
        assert!(
            snap.sql
                .usages
                .iter()
                .any(|u| u.table == table && u.mode == "write" && u.operation == "select_into"),
            "{:?}",
            snap.sql
        );
        assert!(!snap
            .sql
            .usages
            .iter()
            .any(|u| u.table == table && u.mode == "read"));
    }
    assert!(snap
        .sql
        .usages
        .iter()
        .any(|u| u.table == "base" && u.mode == "read"));
    assert!(!snap
        .sql
        .usages
        .iter()
        .any(|u| u.table == "q" || u.table.contains("capture") || u.table.ends_with("_variable")));
    assert!(snap
        .sql
        .diagnostics
        .iter()
        .any(|d| d["code"] == "SQL_INTO_TARGET_UNRESOLVED"));
}
#[test]
fn sql_select_into_migration_does_not_invent_complete_projection_schema() {
    let snap = snap(&[(
        "/p/migrations/001.sql",
        "CREATE TABLE known(id INT); SELECT id INTO derived FROM known;",
        None,
    )]);
    assert!(snap
        .sql
        .diagnostics
        .iter()
        .any(|d| d["code"] == "SELECT_INTO_SCHEMA_UNVERIFIED" && d["schema_relevant"] == true));
    assert!(!snap.sql.tables.iter().any(|t| t.name == "derived"));
    assert_eq!(
        snap.write_check(&check("known", "insert", &["id"]))
            .unwrap()["status"],
        "indeterminate"
    );
}
#[test]
fn sql_static_literals_have_owner_and_source_lines_dynamic_sql_is_not_guessed() {
    let snap=snap(&[
            ("/p/a.rs","fn run() {\n let query = r#\"SELECT * FROM rust_table\"#;\n // SELECT * FROM fake_table\n}",None),
            ("/p/a.py","def run():\n    q = '''SELECT *\nFROM python_table'''\n    q2=f\"UPDATE {table} SET value=1\"\n",None),
            ("/p/a.ts","function run() { const q = `SELECT * FROM ts_table`; const d = `SELECT * FROM ${table}`; }",None),
        ]);
    assert_eq!(snap.sql.usages.len(), 3, "{:?}", snap.sql);
    assert!(snap.sql.usages.iter().all(|u| u.symbol.is_some()));
    let python = snap
        .sql
        .usages
        .iter()
        .find(|u| u.table == "python_table")
        .unwrap();
    assert_eq!(python.evidence.line, 3);
    assert!(snap
        .sql
        .diagnostics
        .iter()
        .any(|d| d["code"] == "DYNAMIC_OR_UNSUPPORTED_SQL_LITERAL"));
}
#[test]
fn write_check_rejects_misspelled_plan_fields() {
    // A misspelled values/columns field must not silently validate a different plan.
    for field in ["value", "column", "operaton", "projet"] {
        let mut request = serde_json::json!({"table":"users"});
        request[field] = serde_json::json!({"name":null});
        assert!(serde_json::from_value::<WriteCheck>(request).is_err());
    }
    let request: WriteCheck = serde_json::from_value(serde_json::json!({
        "table":"users", "values":{"name":null}
    }))
    .unwrap();
    assert!(request.values.unwrap()["name"].is_null());
}
#[test]
fn write_check_unknown_columns_suggestions_required_nulls_and_operations() {
    let snap = snap(&[(
        "/p/migrations/001.sql",
        "CREATE TABLE users(id SERIAL, name TEXT NOT NULL, age INT, email TEXT DEFAULT '');",
        None,
    )]);
    assert_eq!(
        snap.write_check(&check("users", "insert", &["name"]))
            .unwrap()["status"],
        "valid"
    );
    let invalid = snap
        .write_check(&check("users", "insert", &["nane"]))
        .unwrap();
    assert_eq!(invalid["status"], "invalid");
    assert!(invalid["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["code"] == "UNKNOWN_COLUMN" && i["suggestions"][0] == "name"));
    assert!(invalid["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["code"] == "MISSING_REQUIRED_COLUMNS"));
    let mut q = check("users", "insert", &[]);
    q.values = Some(BTreeMap::from([("name".into(), serde_json::Value::Null)]));
    assert_eq!(
        snap.write_check(&q).unwrap()["issues"][0]["code"],
        "NULL_NOT_ALLOWED"
    );
    assert_eq!(
        snap.write_check(&check("users", "update", &["age"]))
            .unwrap()["status"],
        "valid"
    );
    assert_eq!(
        snap.write_check(&check("users", "update", &[])).unwrap()["status"],
        "invalid"
    );
    assert_eq!(
        snap.write_check(&check("users", "delete", &[])).unwrap()["status"],
        "valid"
    );
    assert!(snap
        .write_check(&check("users; DROP TABLE users", "insert", &[]))
        .is_err());
    let missing = snap
        .write_check(&check("usres", "insert", &["name"]))
        .unwrap();
    assert_eq!(missing["status"], "invalid");
    assert_eq!(missing["issues"][0]["suggestions"][0], "users");
    assert!(!missing["schema_evidence"].as_array().unwrap().is_empty());
}
#[test]
fn sql_partial_schema_ambiguous_projects_and_quoted_names_fail_closed() {
    let mut snap = snap(&[
        ("/a/001.sql", "CREATE TABLE users(name TEXT);", Some("a")),
        ("/b/001.sql", "CREATE TABLE users(other TEXT);", Some("b")),
    ]);
    assert_eq!(
        snap.write_check(&check("users", "insert", &["name"]))
            .unwrap()["status"],
        "indeterminate"
    );
    let mut q = check("users", "insert", &["name"]);
    q.project = Some("a".into());
    assert_eq!(snap.write_check(&q).unwrap()["status"], "valid");
    snap.stale_documents.push("stale".into());
    assert_eq!(snap.write_check(&q).unwrap()["status"], "indeterminate");
    let partial = super::tests::snap(&[(
        "/p/001.sql",
        "CREATE TABLE users(name TEXT); ALTER TABLE users ENABLE ROW LEVEL SECURITY;",
        None,
    )]);
    assert!(!partial.sql.tables[0].complete);
    assert_eq!(
        partial
            .write_check(&check("users", "insert", &["name"]))
            .unwrap()["status"],
        "indeterminate"
    );
    let quoted = super::tests::snap(&[(
        "/p/001.sql",
        "CREATE TABLE \"Users\" (\"DisplayName\" TEXT NOT NULL, \"lower\" INT);",
        None,
    )]);
    assert_eq!(
        quoted
            .write_check(&check("\"Users\"", "insert", &["\"DisplayName\"", "lower"]))
            .unwrap()["status"],
        "valid"
    );
    assert_eq!(
        quoted
            .write_check(&check("users", "insert", &["name"]))
            .unwrap()["status"],
        "invalid"
    );
}
#[test]
fn sql_qualified_table_renames_and_escaped_literal_locations() {
    let snap = snap(&[
        (
            "/p/001.sql",
            "CREATE TABLE public.users(name TEXT); ALTER TABLE public.users RENAME TO accounts;",
            None,
        ),
        (
            "/p/query.rs",
            "fn fetch() { let q = \"SELECT\\n* FROM public.accounts\"; }",
            None,
        ),
    ]);
    assert_eq!(snap.sql.tables[0].name, "public.accounts");
    assert_eq!(snap.sql.usages[0].evidence.line, 1);
    assert_eq!(
        snap.write_check(&check("public.accounts", "insert", &["name"]))
            .unwrap()["status"],
        "valid"
    );
}
#[test]
fn code_migration_downgrade_and_orm_limitations_are_explicit() {
    let snap=snap(&[("/p/migrations/001.py","def upgrade():\n    execute('CREATE TABLE users(name TEXT)')\ndef downgrade():\n    execute('DROP TABLE users')\n",None)]);
    assert_eq!(snap.sql.tables.len(), 1);
    assert_eq!(snap.sql.tables[0].name, "users");
    assert!(!snap.sql.tables[0].complete);
    let orm = super::tests::snap(&[(
        "/p/migrations/001.py",
        "def upgrade():\n    op.create_table('users')\n",
        None,
    )]);
    assert!(orm
        .sql
        .diagnostics
        .iter()
        .any(|d| d["code"] == "MIGRATION_DSL_NOT_SUPPORTED"));
    assert_eq!(
        orm.write_check(&check("users", "insert", &[])).unwrap()["status"],
        "indeterminate"
    );
}
