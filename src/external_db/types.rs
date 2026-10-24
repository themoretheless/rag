//! Native declared type metadata; never learned from table rows.
use super::*;
use sqlparser::{
    ast::{DataType, EnumMember},
    dialect::MySqlDialect,
    parser::Parser,
    tokenizer::{Token, Tokenizer},
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeCatalog {
    pub definitions: Vec<Value>,
    pub complete: bool,
    pub diagnostics: Vec<String>,
}
fn mysql_members(raw: &str) -> Option<(String, Vec<String>)> {
    // COLUMN_TYPE is canonical metadata, not an SQL literal in the reader's
    // sql_mode. MySQL Field_enum/Field_set render it with append_unescaped.
    let tokens = Tokenizer::new(&MySqlDialect {}, raw).tokenize().ok()?;
    if tokens.iter().skip(1).any(|t| matches!(t, Token::Word(_))) {
        return None;
    }
    let mut p = Parser::new(&MySqlDialect {}).try_with_sql(raw).ok()?;
    let parsed = p.parse_data_type().ok()?;
    if p.peek_token().token != Token::EOF {
        return None;
    }
    match parsed {
        DataType::Enum(values, None) => Some((
            "enum".into(),
            values
                .into_iter()
                .map(|v| match v {
                    EnumMember::Name(n) => Some(n),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?,
        )),
        DataType::Set(values) => Some(("set".into(), values)),
        _ => None,
    }
}
impl Remote {
    pub(super) async fn types(&mut self, schema: &str, table: &str) -> Result<TypeCatalog> {
        if matches!(self, Self::Pg(_)) {
            let mut rows=self.rows(
                "WITH RECURSIVE used(oid,depth,path) AS (SELECT DISTINCT a.atttypid,0,ARRAY[a.atttypid] FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid=a.attrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 AND a.attnum>0 AND NOT a.attisdropped UNION ALL SELECT child.oid,u.depth+1,u.path||child.oid FROM used u JOIN pg_catalog.pg_type parent ON parent.oid=u.oid CROSS JOIN LATERAL (SELECT parent.typbasetype AS oid WHERE parent.typtype='d' UNION ALL SELECT parent.typelem WHERE parent.typcategory='A' AND parent.typelem<>0 UNION ALL SELECT r.rngsubtype FROM pg_catalog.pg_range r WHERE r.rngtypid=parent.oid UNION ALL SELECT r.rngtypid FROM pg_catalog.pg_range r WHERE r.rngmultitypid=parent.oid) edge JOIN pg_catalog.pg_type child ON child.oid=edge.oid WHERE u.depth<16 AND NOT child.oid=ANY(u.path)) SELECT n.nspname::text,t.typname::text,CASE WHEN t.typtype='d' THEN 'domain' WHEN t.typtype='e' THEN 'enum' WHEN t.typcategory='A' THEN 'array' WHEN t.typtype='c' THEN 'composite' WHEN t.typtype='r' THEN 'range' WHEN t.typtype='m' THEN 'multirange' WHEN n.nspname='pg_catalog' THEN 'scalar' ELSE 'custom_scalar' END,pg_catalog.format_type(t.oid,t.typtypmod)::text,bn.nspname::text,b.typname::text,CASE WHEN t.typbasetype<>0 THEN pg_catalog.format_type(t.typbasetype,t.typtypmod) END::text,en.nspname::text,e.typname::text,t.typnotnull::text,LEFT(pg_catalog.pg_get_expr(t.typdefaultbin,0),2048)::text,encode(sha256(convert_to(pg_catalog.pg_get_expr(t.typdefaultbin,0),'UTF8')),'hex')::text,(COALESCE(length(pg_catalog.pg_get_expr(t.typdefaultbin,0)),0)>2048)::text,COALESCE((SELECT jsonb_agg(v.enumlabel ORDER BY v.enumsortorder)::text FROM (SELECT enumlabel,enumsortorder FROM pg_catalog.pg_enum WHERE enumtypid=t.oid ORDER BY enumsortorder LIMIT 513) v),'[]'),COALESCE((SELECT jsonb_agg(v.record ORDER BY v.name)::text FROM (SELECT conname AS name,jsonb_build_object('name',conname,'validated',convalidated,'definition',LEFT(pg_catalog.pg_get_constraintdef(oid,true),8192),'definition_hash',encode(sha256(convert_to(pg_catalog.pg_get_constraintdef(oid,true),'UTF8')),'hex'),'truncated',length(pg_catalog.pg_get_constraintdef(oid,true))>8192) AS record FROM pg_catalog.pg_constraint WHERE contypid=t.oid ORDER BY conname LIMIT 257) v),'[]'),EXISTS(SELECT 1 FROM used u WHERE u.oid=t.oid AND u.depth=16 AND (t.typbasetype<>0 OR t.typelem<>0 OR t.typtype IN ('r','m')))::text,(SELECT jsonb_build_object('subtype_schema',sn.nspname,'subtype_name',st.typname,'range_schema',rn.nspname,'range_name',rt.typname,'multirange_schema',mn.nspname,'multirange_name',mt.typname,'canonical_function',CASE WHEN r.rngcanonical<>0 THEN r.rngcanonical::regprocedure::text END,'difference_function',CASE WHEN r.rngsubdiff<>0 THEN r.rngsubdiff::regprocedure::text END,'collation',co.collname,'operator_class',op.opcname)::text FROM pg_catalog.pg_range r JOIN pg_catalog.pg_type st ON st.oid=r.rngsubtype JOIN pg_catalog.pg_namespace sn ON sn.oid=st.typnamespace JOIN pg_catalog.pg_type rt ON rt.oid=r.rngtypid JOIN pg_catalog.pg_namespace rn ON rn.oid=rt.typnamespace JOIN pg_catalog.pg_type mt ON mt.oid=r.rngmultitypid JOIN pg_catalog.pg_namespace mn ON mn.oid=mt.typnamespace LEFT JOIN pg_catalog.pg_collation co ON co.oid=r.rngcollation JOIN pg_catalog.pg_opclass op ON op.oid=r.rngsubopc WHERE r.rngtypid=t.oid OR r.rngmultitypid=t.oid),(SELECT cn.nspname::text FROM pg_catalog.pg_collation co JOIN pg_catalog.pg_namespace cn ON cn.oid=co.collnamespace WHERE co.oid=t.typcollation),(SELECT collname::text FROM pg_catalog.pg_collation WHERE oid=t.typcollation) FROM pg_catalog.pg_type t JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace LEFT JOIN pg_catalog.pg_type b ON b.oid=t.typbasetype LEFT JOIN pg_catalog.pg_namespace bn ON bn.oid=b.typnamespace LEFT JOIN pg_catalog.pg_type e ON e.oid=t.typelem LEFT JOIN pg_catalog.pg_namespace en ON en.oid=e.typnamespace WHERE t.oid IN (SELECT oid FROM used) ORDER BY n.nspname,t.typname LIMIT 257",
                "SELECT ''",&[schema,table],&["schema","name","kind","display_type","base_schema","base_name","base_display_type","element_schema","element_name","not_null","default_sql","default_hash","default_truncated","enum_members","constraints","depth_limit","range_details","collation_schema","collation_name"]).await?;
            let mut diagnostics = vec![];
            if rows.len() > 256 {
                diagnostics.push("TYPE_DEFINITION_LIMIT".into());
            }
            rows.truncate(256);
            for t in &mut rows {
                let members: Vec<Value> =
                    serde_json::from_str(t["enum_members"].as_str().unwrap_or("[]"))
                        .map_err(|_| remote_error())?;
                let constraints: Vec<Value> =
                    serde_json::from_str(t["constraints"].as_str().unwrap_or("[]"))
                        .map_err(|_| remote_error())?;
                let limited = members.len() > 512
                    || constraints.len() > 256
                    || constraints.iter().any(|c| c["truncated"] == true)
                    || t["default_truncated"] == "true"
                    || t["depth_limit"] == "true";
                if limited {
                    diagnostics.push("TYPE_MEMBER_CONSTRAINT_OR_DEPTH_LIMIT".into());
                }
                if matches!(t["kind"].as_str(), Some("custom_scalar" | "composite")) {
                    diagnostics.push("TYPE_BODY_NOT_COLLECTED".into());
                }
                if let Some(raw) = t["range_details"].as_str() {
                    t["range_details"] = serde_json::from_str(raw).map_err(|_| remote_error())?;
                }
                t["enum_members"] = json!(members.into_iter().take(512).collect::<Vec<_>>());
                t["constraints"] = json!(constraints.into_iter().take(256).collect::<Vec<_>>());
                t["not_null"] = json!(t["not_null"] == "true");
                t["default_truncated"] = json!(t["default_truncated"] == "true");
                t["depth_limit"] = json!(t["depth_limit"] == "true");
                t["complete"] = json!(
                    !limited && !matches!(t["kind"].as_str(), Some("custom_scalar" | "composite"))
                );
            }
            diagnostics.sort();
            diagnostics.dedup();
            return Ok(TypeCatalog {
                definitions: rows,
                complete: diagnostics.is_empty(),
                diagnostics,
            });
        }
        let rows=self.rows("SELECT ''","SELECT CAST(COLUMN_NAME AS CHAR),CAST(DATA_TYPE AS CHAR),LEFT(CAST(COLUMN_TYPE AS CHAR),65536),CAST(CHARACTER_SET_NAME AS CHAR),CAST(COLLATION_NAME AS CHAR),CAST(CHAR_LENGTH(COLUMN_TYPE)>65536 AS CHAR),SHA2(COLUMN_TYPE,256) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY ORDINAL_POSITION LIMIT 257",&[schema,table],&["name","kind","definition","charset","collation","truncated","raw_hash"]).await?;
        let mut diagnostics = vec![];
        if rows.len() > 256 {
            diagnostics.push("TYPE_COLUMN_LIMIT".into());
        }
        let mut definitions = vec![];
        for r in rows.into_iter().take(256) {
            let kind = r["kind"].as_str().unwrap_or("");
            let raw = r["definition"].as_str().unwrap_or("");
            let mut d = json!({"schema":schema,"name":r["name"],"scope":"column","kind":if matches!(kind,"enum"|"set"){kind}else{"scalar"},"display_type":raw,"charset":r["charset"],"collation":r["collation"],"complete":true});
            if matches!(kind, "enum" | "set") {
                if let Some((parsed_kind, mut members)) =
                    mysql_members(raw).filter(|(k, _)| k == kind)
                {
                    let complete = members.len() <= 512 && r["truncated"] != "1";
                    members.truncate(512);
                    d["members"] = json!(members);
                    d["display_type"] = json!(parsed_kind);
                    d["complete"] = json!(complete);
                    if !complete {
                        diagnostics.push("MYSQL_ENUM_SET_MEMBER_LIMIT".into());
                        d["partial_definition_hash"] = r["raw_hash"].clone();
                    }
                } else {
                    d["complete"] = json!(false);
                    d["partial_definition_hash"] = r["raw_hash"].clone();
                    diagnostics.push("MYSQL_ENUM_SET_DEFINITION_UNAVAILABLE".into());
                }
            }
            if r["truncated"] == "1" {
                d["complete"] = json!(false);
                diagnostics.push("MYSQL_TYPE_TEXT_TRUNCATED".into());
            }
            definitions.push(d);
        }
        diagnostics.sort();
        diagnostics.dedup();
        Ok(TypeCatalog {
            definitions,
            complete: diagnostics.is_empty(),
            diagnostics,
        })
    }
}
fn reference(schema: &Value, name: &Value) -> Value {
    json!({"schema":schema,"name":name})
}
pub(super) fn enrich(columns: &mut [Value], catalog: &TypeCatalog, dialect: &str) -> bool {
    for c in columns.iter_mut() {
        if dialect == "mysql" {
            let Some(d) = catalog.definitions.iter().find(|d| d["name"] == c["name"]) else {
                c["resolved_type"] = json!({"complete":false,"reason":"type_definition_missing"});
                continue;
            };
            if matches!(d["kind"].as_str(), Some("enum" | "set")) {
                c["data_type"] = d["kind"].clone();
            }
            c["resolved_type"] = json!({"kind":d["kind"],"complete":d["complete"],"base_type":d["display_type"],"charset":d["charset"],"collation":d["collation"],"members":d.get("members"),"domain_chain":[]});
            continue;
        }
        let mut schema = c["type_schema"].clone();
        let mut name = c["type_name"].clone();
        let mut chain = vec![];
        let mut complete = true;
        let mut base_display = None;
        let mut default = c["default_sql"].clone();
        let mut array = false;
        let mut resolved = json!({"complete":false,"reason":"type_definition_missing"});
        for _ in 0..17 {
            let Some(d) = catalog
                .definitions
                .iter()
                .find(|d| d["schema"] == schema && d["name"] == name)
            else {
                break;
            };
            complete &= d["complete"] == true;
            if d["kind"] == "domain" {
                chain.push(json!({"type":reference(&schema,&name),"applies_to":if array{"array_element"}else{"value"}}));
                if !array && d["not_null"] == true {
                    c["nullable"] = json!("NO");
                }
                if !array && default.is_null() && !d["default_sql"].is_null() {
                    default = d["default_sql"].clone();
                }
                schema = d["base_schema"].clone();
                name = d["base_name"].clone();
                base_display = d["base_display_type"].as_str().map(str::to_owned);
                continue;
            }
            if d["kind"] == "array" {
                array = true;
                base_display = None;
                chain.push(json!({"array":reference(&schema,&name)}));
                schema = d["element_schema"].clone();
                name = d["element_name"].clone();
                continue;
            }
            resolved = json!({"complete":complete,"kind":if array{"array"}else{d["kind"].as_str().unwrap_or("unknown")},"leaf_kind":d["kind"],"base_schema":schema,"base_name":name,"base_type":base_display.as_ref().map_or_else(||d["display_type"].clone(),|s|json!(s)),"domain_chain":chain,"domain_constraints_evaluated":false});
            break;
        }
        c["resolved_type"] = resolved;
        c["effective_default_sql"] = default;
    }
    columns
        .iter()
        .all(|c| c["resolved_type"]["complete"] == true)
}
pub(super) fn identifier_key(c: &Value) -> Option<String> {
    let r = &c["resolved_type"];
    if r.is_null() {
        return None;
    }
    if r["complete"] != true {
        return None;
    }
    if r["kind"] == "enum" {
        if r["base_schema"].is_string() {
            return Some(format!(
                "enum:{}",
                json!([r["base_schema"], r["base_name"]])
            ));
        }
        return Some(format!(
            "enum:{}",
            json!([r["members"], r["charset"], r["collation"]])
        ));
    }
    if r["kind"] != "scalar" {
        return None;
    }
    let mut base = c.clone();
    base.as_object_mut()?.remove("resolved_type");
    base["data_type"] = r["base_type"].clone();
    base["type_schema"] = json!("pg_catalog");
    // Preserve native precision fields; scalar domains can carry a type modifier
    // even when information_schema has no expanded nested-domain attributes.
    if let Some(raw) = base["data_type"].as_str().map(str::to_owned) {
        if let Some(args) = raw.split_once('(').and_then(|(_, s)| s.strip_suffix(')')) {
            let numbers = args.split(',').map(str::trim).collect::<Vec<_>>();
            if raw.starts_with("character")
                || raw.starts_with("varchar")
                || raw.starts_with("char(")
            {
                base["character_maximum_length"] = json!(numbers[0]);
            } else if raw.starts_with("numeric") || raw.starts_with("decimal") {
                base["numeric_precision"] = json!(numbers[0]);
                base["numeric_scale"] = json!(numbers.get(1).copied().unwrap_or("0"));
            }
        }
    }
    super::relations::scalar_type_key(&base)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn type_capture_upgrade_preserves_legacy_payload_and_does_not_report_new_metadata_as_ddl() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("types.duckdb")).unwrap();
        let base = json!({"capture_format":3,"id":"legacy","connection":"alias","project":"p","dialect":"postgresql","schemas":["public"],"captured_ms":0,"code_index_revision":null,"target_fingerprint":"h","complete_within_visible_scope":true,"diagnostics":[],"inferred_relations":{"algorithm_version":1,"relations":[],"diagnostics":[],"counts":{},"truncated":false,"diagnostic_truncated":false},"tables":[{"schema":"public","name":"t","kind":"BASE TABLE","columns":[],"foreign_keys":[],"complete":true,"sample":null,"metadata":{"indexes":[],"constraints":[],"view":null,"complete":true,"diagnostics":[]}}]});
        let payload = base.to_string();
        store.lock().unwrap().execute("INSERT INTO external_db_snapshots (id,connection_alias,captured_ms,payload) VALUES (?,?,?,?)",duckdb::params!["legacy","alias",0,payload]).unwrap();
        let q = Query {
            connection: "alias".into(),
            snapshot_id: Some("legacy".into()),
            table: Some("t".into()),
            ..Default::default()
        };
        assert!(
            store.external_db_query("db_table", &q).unwrap()["items"][0]["metadata"]["types"]
                .is_null()
        );
        let retained: String = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload FROM external_db_snapshots WHERE id='legacy'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(retained, payload);
        let mut current = base;
        current["capture_format"] = json!(4);
        current["id"] = json!("new");
        assert!(serde_json::from_value::<Snapshot>(current.clone())
            .unwrap()
            .validate_format()
            .is_err());
        current["tables"][0]["metadata"]["types"] =
            json!({"definitions":[],"complete":true,"diagnostics":[]});
        serde_json::from_value::<Snapshot>(current.clone())
            .unwrap()
            .validate_format()
            .unwrap();
        store.lock().unwrap().execute("INSERT INTO external_db_snapshots (id,connection_alias,captured_ms,payload) VALUES (?,?,?,?)",duckdb::params!["new","alias",1,current.to_string()]).unwrap();
        let diff = store
            .external_db_query(
                "db_drift",
                &Query {
                    connection: "alias".into(),
                    base_snapshot_id: Some("legacy".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(diff["compared"], false);
        assert_eq!(diff["reason"], "capture_format_changed");
    }
    #[test]
    fn domain_defaults_and_not_null_do_not_leak_from_array_elements() {
        let catalog = TypeCatalog {
            complete: true,
            diagnostics: vec![],
            definitions: vec![
                json!({"schema":"public","name":"_positive","kind":"array","element_schema":"public","element_name":"positive","complete":true}),
                json!({"schema":"public","name":"positive","kind":"domain","not_null":true,"default_sql":"7","base_schema":"pg_catalog","base_name":"int4","base_display_type":"integer","complete":true}),
                json!({"schema":"pg_catalog","name":"int4","kind":"scalar","display_type":"integer","complete":true}),
            ],
        };
        let mut columns = vec![
            json!({"name":"ids","type_schema":"public","type_name":"_positive","nullable":"YES","default_sql":null}),
            json!({"name":"id","type_schema":"public","type_name":"positive","nullable":"YES","default_sql":null}),
        ];
        assert!(enrich(&mut columns, &catalog, "postgresql"));
        assert_eq!(columns[0]["nullable"], "YES");
        assert!(columns[0]["effective_default_sql"].is_null());
        assert_eq!(columns[0]["resolved_type"]["kind"], "array");
        assert_eq!(columns[1]["nullable"], "NO");
        assert_eq!(columns[1]["effective_default_sql"], "7");
        assert_eq!(identifier_key(&columns[1]).unwrap(), "integer:32:false");
    }
    #[test]
    fn mysql_enum_set_parsing_preserves_member_order_and_literal_boundaries() {
        assert_eq!(
            mysql_members("enum('','a,b','quote''x','slash\\\\x','ж')")
                .unwrap()
                .1,
            vec!["", "a,b", "quote'x", "slash\\x", "ж"]
        );
        assert_eq!(mysql_members("set('read','write')").unwrap().0, "set");
        assert!(mysql_members("enum('x'); DROP TABLE t").is_none());
        assert!(mysql_members("enum(unquoted)").is_none());
        assert!(mysql_members("enum(x'ff')").is_none());
    }
}
