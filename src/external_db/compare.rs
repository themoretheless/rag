use super::*;
use sqlparser::{dialect::GenericDialect, parser::Parser};
use std::collections::BTreeSet;
fn key(t: &LiveTable) -> (String, String) {
    (t.schema.clone(), t.name.clone())
}
fn definition(t: &LiveTable) -> Value {
    json!({"kind":t.kind,"columns":t.columns,"foreign_keys":t.foreign_keys,"metadata":t.metadata})
}
fn column_name(name: &str) -> Option<String> {
    let mut p = Parser::new(&GenericDialect {}).try_with_sql(name).ok()?;
    let n = p.parse_object_name(false).ok()?;
    if n.0.len() != 1 {
        return None;
    }
    n.0[0].as_ident().map(|i| i.value.clone())
}
fn resolve<'a>(name: &str, tables: &'a [LiveTable]) -> Vec<&'a LiveTable> {
    let Ok(mut p) = Parser::new(&GenericDialect {}).try_with_sql(name) else {
        return vec![];
    };
    let Ok(n) = p.parse_object_name(false) else {
        return vec![];
    };
    let parts: Option<Vec<_>> =
        n.0.iter()
            .map(|p| p.as_ident().map(|i| i.value.as_str()))
            .collect();
    let Some(parts) = parts else { return vec![] };
    tables
        .iter()
        .filter(|t| match parts.as_slice() {
            [name] => *name == t.name,
            [schema, name] => *schema == t.schema && *name == t.name,
            _ => false,
        })
        .collect()
}
impl Store {
    pub(super) fn external_db_drift(&self, s: &Snapshot, q: &Query) -> Result<Value> {
        let baseline = if let Some(id) = &q.base_snapshot_id {
            Some(self.external_snapshot(&Query {
                connection: q.connection.clone(),
                snapshot_id: Some(id.clone()),
                ..Default::default()
            })?)
        } else {
            let conn = self.lock()?;
            let mut stmt=conn.prepare("SELECT payload FROM external_db_snapshots WHERE connection_alias=? AND seq < (SELECT seq FROM external_db_snapshots WHERE id=?) ORDER BY seq DESC LIMIT 1")?;
            let mut rows = stmt.query(duckdb::params![s.connection, s.id])?;
            let body: Option<String> = rows.next()?.map(|r| r.get(0)).transpose()?;
            body.map(|body| {
                serde_json::from_str::<Snapshot>(&body)
                    .map_err(|_| AppError::config("invalid baseline snapshot"))
            })
            .transpose()?
        };
        let Some(base) = baseline else {
            return Ok(
                json!({"snapshot_id":s.id,"compared":false,"reason":"no_baseline_snapshot"}),
            );
        };
        base.validate_format()?;
        if s.structural_format() != base.structural_format() {
            return Ok(
                json!({"snapshot_id":s.id,"base_snapshot_id":base.id,"compared":false,"reason":"capture_format_changed","capture_format":s.capture_format,"baseline_capture_format":base.capture_format}),
            );
        }
        if s.target_fingerprint != base.target_fingerprint {
            return Ok(
                json!({"snapshot_id":s.id,"base_snapshot_id":base.id,"compared":false,"reason":"different_database_target_or_visible_scope"}),
            );
        }
        let before: BTreeMap<_, _> = base.tables.iter().map(|t| (key(t), t)).collect();
        let after: BTreeMap<_, _> = s.tables.iter().map(|t| (key(t), t)).collect();
        let mut changes = vec![];
        for (k, t) in &after {
            match before.get(k){None=>changes.push(json!({"kind":"table_added","schema":k.0,"table":k.1})),Some(b) if definition(t)!=definition(b)=>changes.push(json!({"kind":"table_changed","schema":k.0,"table":k.1,"before":definition(b),"after":definition(t)})),_=>{}}
        }
        for k in before.keys().filter(|k| !after.contains_key(*k)) {
            changes
                .push(json!({"kind":"table_removed_from_visible_scope","schema":k.0,"table":k.1}));
        }
        let limit = q.limit.unwrap_or(50).clamp(1, 200);
        Ok(
            json!({"snapshot_id":s.id,"base_snapshot_id":base.id,"captured_ms":s.captured_ms,"baseline_captured_ms":base.captured_ms,"compared":true,"capture_format":s.capture_format,"baseline_capture_format":base.capture_format,"inference_compared":false,"changes":changes.iter().take(limit).collect::<Vec<_>>(),"total":changes.len(),"truncated":changes.len()>limit,"complete_within_visible_scope":s.complete_within_visible_scope&&base.complete_within_visible_scope,"diagnostics":s.diagnostics,"scope":"captured table kinds, columns, indexes, constraints view relation dependencies and declared type definitions; metadata visibility changes can appear as removals","not_checked":if s.capture_format<2{vec!["indexes","check_constraints","view_definitions","privilege_changes","row_data"]}else if s.capture_format<4{vec!["privilege_changes","row_data","function_dependencies","full_enum_domain_types","sequence_parameters"]}else{vec!["privilege_changes","row_data","function_dependencies","composite_and_custom_type_bodies","sequence_parameters"]},"live_query":false}),
        )
    }
    pub(super) fn external_db_cross(&self, s: &Snapshot, q: &Query) -> Result<Value> {
        let code = self.code_snapshot()?;
        let mut observed = BTreeSet::new();
        let mut uses = vec![];
        for usage in code
            .sql
            .usages
            .iter()
            .filter(|u| u.evidence.project.as_deref() == Some(s.project.as_str()))
        {
            let matches = resolve(&usage.table, &s.tables);
            for t in &matches {
                observed.insert(key(t));
            }
            uses.push(json!({"table":usage.table,"mode":usage.mode,"operation":usage.operation,"symbol":usage.symbol,"evidence":usage.evidence,"resolution":match matches.len(){0=>"not_observed_in_visible_live_schema",1=>"matched",_=>"ambiguous"},"candidates":matches.iter().map(|t|json!({"schema":t.schema,"table":t.name})).collect::<Vec<_>>()}));
        }
        let migrations:Vec<_>=code.sql.tables.iter().filter(|t|t.project.as_deref()==Some(s.project.as_str())).map(|t|{
            let matches=resolve(&t.name,&s.tables);
            let columns=if matches.len()==1{let live=matches[0];let names:BTreeSet<_>=live.columns.iter().filter_map(|c|c["name"].as_str()).collect();t.columns.keys().filter(|n|column_name(n).is_none_or(|n|!names.contains(n.as_str()))).cloned().collect::<Vec<_>>()}else{vec![]};
            json!({"table":t.name,"derived_schema_complete":t.complete,"live_matches":matches.len(),"migration_columns_not_observed_live":columns,"evidence":t.evidence})
        }).collect();
        let not_observed: Vec<_> = s
            .tables
            .iter()
            .filter(|t| !observed.contains(&key(t)))
            .map(|t| json!({"schema":t.schema,"table":t.name,"deletion_safe":false}))
            .collect();
        let limit = q.limit.unwrap_or(50).clamp(1, 200);
        Ok(
            json!({"snapshot_id":s.id,"connection":s.connection,"project":s.project,"captured_ms":s.captured_ms,"code_index_rev":code.index_revision,"code_index_rev_at_capture":s.code_index_revision,"uses":uses.iter().take(limit).collect::<Vec<_>>(),"total_uses":uses.len(),"migrations":migrations.iter().take(limit).collect::<Vec<_>>(),"total_migrations":migrations.len(),"live_tables_without_observed_static_sql_use":not_observed.iter().take(limit).collect::<Vec<_>>(),"total_unobserved_tables":not_observed.len(),"truncated":uses.len()>limit||migrations.len()>limit||not_observed.len()>limit,"code_diagnostics":code.sql.diagnostics,"stale_documents":code.stale_documents,"unindexed_documents":code.unindexed_documents,"parse_errors":code.parse_errors,"live_schema_complete_within_visible_scope":s.complete_within_visible_scope,"limitations":["dynamic SQL and ORM-generated queries may be absent","remote capture and code snapshot are not an atomic distributed snapshot","unqualified names do not infer a runtime search path","column comparison checks names only, not SQL type compatibility","no code observation does not establish an unused table"],"live_query":false}),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_resolution_preserves_qualified_names_and_ambiguity() {
        let table = |schema: &str, name: &str| LiveTable {
            schema: schema.into(),
            name: name.into(),
            kind: "BASE TABLE".into(),
            columns: vec![],
            foreign_keys: vec![],
            complete: true,
            sample: Value::Null,
            metadata: None,
        };
        let tables = vec![
            table("public", "users"),
            table("audit", "users"),
            table("public", "Users"),
        ];
        assert_eq!(column_name("\"a\"\"b\""), Some("a\"b".into()));
        assert_eq!(resolve("users", &tables).len(), 2);
        assert_eq!(resolve("public.users", &tables).len(), 1);
        assert_eq!(resolve("\"Users\"", &tables)[0].name, "Users");
        assert!(resolve("other.users", &tables).is_empty());
    }
}
