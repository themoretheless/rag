//! Bounded native structural metadata. Statistics, row counts and OIDs are not schema facts.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableMetadata {
    #[serde(default)]
    pub types: Option<types::TypeCatalog>,
    pub indexes: Vec<Value>,
    pub constraints: Vec<Value>,
    pub view: Option<ViewMetadata>,
    pub complete: bool,
    pub diagnostics: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewMetadata {
    pub definition: Option<String>,
    pub definition_hash: Option<String>,
    pub definition_truncated: bool,
    pub options: Value,
    pub dependencies: Vec<Value>,
    pub complete: bool,
}
fn available_definition(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| !s.trim().is_empty())
}
fn yes(v: &Value) -> bool {
    matches!(v.as_str(), Some("true" | "1" | "YES"))
}
fn null_or_flag(v: &Value) -> Value {
    if v.is_null() {
        Value::Null
    } else {
        json!(yes(v))
    }
}
fn group_indexes(rows: Vec<Value>) -> Vec<Value> {
    let mut groups: BTreeMap<String, Value> = BTreeMap::new();
    for r in rows {
        let name = r["name"].as_str().unwrap_or("").to_owned();
        let index=groups.entry(name.clone()).or_insert_with(||json!({"name":name,"unique":yes(&r["unique"]),"primary":yes(&r["primary"]),"method":r["method"],"valid":null_or_flag(&r["valid"]),"ready":null_or_flag(&r["ready"]),"visible":null_or_flag(&r["visible"]),"nulls_not_distinct":null_or_flag(&r["nulls_not_distinct"]),"definition":r["definition"],"definition_hash":r["definition_hash"],"predicate":r["predicate"],"metadata_text_truncated":yes(&r["text_truncated"]),"keys":[]}));
        index["metadata_text_truncated"] =
            json!(index["metadata_text_truncated"] == true || yes(&r["text_truncated"]));
        index["keys"].as_array_mut().unwrap().push(json!({"ordinal":r["ordinal"],"column":r["column"],"expression":r["expression"],"expression_hash":r["expression_hash"],"definition":r["key_definition"],"included":null_or_flag(&r["included"]),"direction":r["direction"],"prefix_length":r["prefix_length"]}));
    }
    groups.into_values().collect()
}
impl Remote {
    pub(super) async fn metadata(
        &mut self,
        schema: &str,
        table: &str,
        kind: &str,
    ) -> Result<TableMetadata> {
        let index_rows=self.rows(
            "SELECT ic.relname::text,i.indisunique::text,i.indisprimary::text,am.amname::text,i.indisvalid::text,i.indisready::text,NULL,i.indnullsnotdistinct::text,LEFT(pg_catalog.pg_get_indexdef(i.indexrelid),8192)::text,encode(sha256(convert_to(pg_catalog.pg_get_indexdef(i.indexrelid),'UTF8')),'hex')::text,LEFT(pg_catalog.pg_get_expr(i.indpred,i.indrelid),8192)::text,((length(pg_catalog.pg_get_indexdef(i.indexrelid))>8192) OR COALESCE(length(pg_catalog.pg_get_expr(i.indpred,i.indrelid)),0)>8192)::text,k.ord::text,a.attname::text,CASE WHEN i.indkey[k.ord-1]=0 THEN LEFT(pg_catalog.pg_get_indexdef(i.indexrelid,k.ord,true),8192) END::text,LEFT(pg_catalog.pg_get_indexdef(i.indexrelid,k.ord,true),8192)::text,(k.ord>i.indnkeyatts)::text,CASE WHEN k.ord>i.indnkeyatts OR am.amname<>'btree' THEN NULL WHEN (i.indoption[k.ord-1] & 1)=1 THEN 'DESC' ELSE 'ASC' END::text,NULL,encode(sha256(convert_to(CASE WHEN i.indkey[k.ord-1]=0 THEN pg_catalog.pg_get_indexdef(i.indexrelid,k.ord,true) END,'UTF8')),'hex')::text FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid=i.indrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace JOIN pg_catalog.pg_class ic ON ic.oid=i.indexrelid JOIN pg_catalog.pg_am am ON am.oid=ic.relam CROSS JOIN LATERAL generate_series(1,i.indnatts) AS k(ord) LEFT JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid AND a.attnum=i.indkey[k.ord-1] WHERE n.nspname=$1 AND c.relname=$2 ORDER BY ic.relname,k.ord LIMIT 513",
            "SELECT CAST(INDEX_NAME AS CHAR),CAST(NON_UNIQUE=0 AS CHAR),CAST(INDEX_NAME='PRIMARY' AS CHAR),CAST(INDEX_TYPE AS CHAR),NULL,NULL,CAST(IS_VISIBLE AS CHAR),NULL,NULL,NULL,NULL,CAST(COALESCE(CHAR_LENGTH(EXPRESSION),0)>8192 AS CHAR),CAST(SEQ_IN_INDEX AS CHAR),CAST(COLUMN_NAME AS CHAR),LEFT(CAST(EXPRESSION AS CHAR),8192),NULL,'0',CASE COLLATION WHEN 'A' THEN 'ASC' WHEN 'D' THEN 'DESC' ELSE NULL END,CAST(SUB_PART AS CHAR),SHA2(EXPRESSION,256) FROM information_schema.STATISTICS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY INDEX_NAME,SEQ_IN_INDEX LIMIT 513",
            &[schema,table],&["name","unique","primary","method","valid","ready","visible","nulls_not_distinct","definition","definition_hash","predicate","text_truncated","ordinal","column","expression","key_definition","included","direction","prefix_length","expression_hash"]).await?;
        let mut diagnostics = vec![];
        if index_rows.len() > 512 {
            diagnostics.push("INDEX_KEY_LIMIT".into());
        }
        if index_rows.iter().any(|r| yes(&r["text_truncated"])) {
            diagnostics.push("INDEX_TEXT_TRUNCATED".into());
        }
        if index_rows
            .iter()
            .any(|r| r["column"].is_null() && available_definition(&r["expression"]).is_none())
        {
            diagnostics.push("INDEX_KEY_DEFINITION_UNAVAILABLE".into());
        }
        let indexes = group_indexes(index_rows.into_iter().take(512).collect());
        let mut constraints=self.rows(
            "SELECT con.conname::text,CASE con.contype WHEN 'p' THEN 'PRIMARY KEY' WHEN 'u' THEN 'UNIQUE' WHEN 'f' THEN 'FOREIGN KEY' WHEN 'c' THEN 'CHECK' WHEN 'x' THEN 'EXCLUSION' ELSE con.contype::text END,con.convalidated::text,'true',con.condeferrable::text,con.condeferred::text,LEFT(pg_catalog.pg_get_constraintdef(con.oid,true),8192)::text,encode(sha256(convert_to(pg_catalog.pg_get_constraintdef(con.oid,true),'UTF8')),'hex')::text,(length(pg_catalog.pg_get_constraintdef(con.oid,true))>8192)::text,con.connoinherit::text FROM pg_catalog.pg_constraint con JOIN pg_catalog.pg_class c ON c.oid=con.conrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 ORDER BY con.conname LIMIT 257",
            "SELECT CAST(t.CONSTRAINT_NAME AS CHAR),CAST(t.CONSTRAINT_TYPE AS CHAR),NULL,CAST(t.ENFORCED AS CHAR),'false','false',LEFT(CAST(c.CHECK_CLAUSE AS CHAR),8192),SHA2(c.CHECK_CLAUSE,256),CAST(COALESCE(CHAR_LENGTH(c.CHECK_CLAUSE),0)>8192 AS CHAR),NULL FROM information_schema.TABLE_CONSTRAINTS t LEFT JOIN information_schema.CHECK_CONSTRAINTS c ON c.CONSTRAINT_SCHEMA=t.CONSTRAINT_SCHEMA AND c.CONSTRAINT_NAME=t.CONSTRAINT_NAME AND t.CONSTRAINT_TYPE='CHECK' WHERE t.TABLE_SCHEMA=? AND t.TABLE_NAME=? ORDER BY t.CONSTRAINT_NAME LIMIT 257",
            &[schema,table],&["name","kind","validated","enforced","deferrable","initially_deferred","definition","definition_hash","text_truncated","no_inherit"]).await?;
        if constraints.len() > 256 {
            diagnostics.push("CONSTRAINT_LIMIT".into());
        }
        if constraints.iter().any(|r| yes(&r["text_truncated"])) {
            diagnostics.push("CONSTRAINT_TEXT_TRUNCATED".into());
        }
        let keys=self.rows(
            "SELECT con.conname::text,k.ord::text,a.attname::text FROM pg_catalog.pg_constraint con JOIN pg_catalog.pg_class c ON c.oid=con.conrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace CROSS JOIN LATERAL unnest(con.conkey) WITH ORDINALITY AS k(attnum,ord) JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid AND a.attnum=k.attnum WHERE n.nspname=$1 AND c.relname=$2 ORDER BY con.conname,k.ord LIMIT 513",
            "SELECT CAST(CONSTRAINT_NAME AS CHAR),CAST(ORDINAL_POSITION AS CHAR),CAST(COLUMN_NAME AS CHAR) FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY CONSTRAINT_NAME,ORDINAL_POSITION LIMIT 513",
            &[schema,table],&["constraint","ordinal","column"]).await?;
        if keys.len() > 512 {
            diagnostics.push("CONSTRAINT_COLUMN_LIMIT".into());
        }
        for c in &mut constraints {
            for field in [
                "validated",
                "enforced",
                "deferrable",
                "initially_deferred",
                "no_inherit",
                "text_truncated",
            ] {
                c[field] = null_or_flag(&c[field]);
            }
            c["columns"] = json!(keys
                .iter()
                .take(512)
                .filter(|k| k["constraint"] == c["name"])
                .map(|k| json!({"ordinal":k["ordinal"],"column":k["column"]}))
                .collect::<Vec<_>>());
            if c["kind"] == "CHECK" && available_definition(&c["definition"]).is_none() {
                c["definition"] = Value::Null;
                c["definition_hash"] = Value::Null;
                diagnostics.push("CHECK_DEFINITION_UNAVAILABLE".into());
            }
        }
        constraints.truncate(256);
        let view = if matches!(kind, "VIEW" | "MATERIALIZED VIEW") {
            let definitions=self.rows(
                "SELECT LEFT(pg_catalog.pg_get_viewdef(c.oid,true),16384)::text,encode(sha256(convert_to(pg_catalog.pg_get_viewdef(c.oid,true),'UTF8')),'hex')::text,(length(pg_catalog.pg_get_viewdef(c.oid,true))>16384)::text,c.reloptions::text,NULL,NULL FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 AND c.relkind IN ('v','m') LIMIT 1",
                "SELECT LEFT(CAST(VIEW_DEFINITION AS CHAR),16384),SHA2(VIEW_DEFINITION,256),CAST(COALESCE(CHAR_LENGTH(VIEW_DEFINITION),0)>16384 AS CHAR),CAST(CHECK_OPTION AS CHAR),CAST(SECURITY_TYPE AS CHAR),CAST(IS_UPDATABLE AS CHAR) FROM information_schema.VIEWS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? LIMIT 1",
                &[schema,table],&["definition","hash","truncated","check_or_reloptions","security_type","updatable"]).await?;
            let d = definitions.first().cloned().unwrap_or_else(|| json!({}));
            let deps=self.rows(
                "SELECT DISTINCT tn.nspname::text,tc.relname::text,CASE tc.relkind WHEN 'v' THEN 'VIEW' WHEN 'm' THEN 'MATERIALIZED VIEW' WHEN 'f' THEN 'FOREIGN TABLE' ELSE 'BASE TABLE' END FROM pg_catalog.pg_depend d JOIN pg_catalog.pg_rewrite r ON d.classid='pg_catalog.pg_rewrite'::regclass AND d.objid=r.oid JOIN pg_catalog.pg_class c ON c.oid=r.ev_class JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace JOIN pg_catalog.pg_class tc ON d.refclassid='pg_catalog.pg_class'::regclass AND tc.oid=d.refobjid JOIN pg_catalog.pg_namespace tn ON tn.oid=tc.relnamespace WHERE n.nspname=$1 AND c.relname=$2 AND tc.oid<>c.oid AND tc.relkind IN ('r','p','v','m','f') ORDER BY 1,2,3 LIMIT 257",
                "SELECT DISTINCT CAST(u.TABLE_SCHEMA AS CHAR),CAST(u.TABLE_NAME AS CHAR),CAST(t.TABLE_TYPE AS CHAR) FROM information_schema.VIEW_TABLE_USAGE u LEFT JOIN information_schema.TABLES t ON t.TABLE_SCHEMA=u.TABLE_SCHEMA AND t.TABLE_NAME=u.TABLE_NAME WHERE u.VIEW_SCHEMA=? AND u.VIEW_NAME=? ORDER BY 1,2,3 LIMIT 257",
                &[schema,table],&["target_schema","target_table","target_kind"]).await?;
            let complete = available_definition(&d["definition"]).is_some()
                && !yes(&d["truncated"])
                && deps.len() <= 256;
            if available_definition(&d["definition"]).is_none() {
                diagnostics.push("VIEW_DEFINITION_UNAVAILABLE".into());
            }
            if yes(&d["truncated"]) {
                diagnostics.push("VIEW_DEFINITION_TRUNCATED".into());
            }
            if deps.len() > 256 {
                diagnostics.push("VIEW_DEPENDENCY_LIMIT".into());
            }
            Some(ViewMetadata {
                definition: available_definition(&d["definition"]).map(str::to_owned),
                definition_hash: available_definition(&d["definition"])
                    .and_then(|_| d["hash"].as_str())
                    .map(str::to_owned),
                definition_truncated: yes(&d["truncated"]),
                options: json!({"check_or_reloptions":d["check_or_reloptions"],"security_type":d["security_type"],"updatable":d["updatable"]}),
                dependencies: deps.into_iter().take(256).collect(),
                complete,
            })
        } else {
            None
        };
        let types = self.types(schema, table).await?;
        diagnostics.extend(types.diagnostics.clone());
        diagnostics.sort();
        diagnostics.dedup();
        Ok(TableMetadata {
            types: Some(types),
            indexes,
            constraints,
            view,
            complete: diagnostics.is_empty(),
            diagnostics,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blank_hidden_view_and_check_definitions_are_not_available() {
        for v in [Value::Null, json!(""), json!(" \n")] {
            assert!(available_definition(&v).is_none());
        }
        assert_eq!(
            available_definition(&json!("SELECT '' AS value")),
            Some("SELECT '' AS value")
        );
    }
    #[test]
    fn composite_index_order_and_nullable_capabilities_are_preserved() {
        let rows = vec![
            json!({"name":"ix","unique":"true","primary":"false","ordinal":"1","column":"b","included":"false","direction":"DESC"}),
            json!({"name":"ix","unique":"true","primary":"false","ordinal":"2","column":"a","included":"true"}),
        ];
        let indexes = group_indexes(rows);
        assert_eq!(indexes[0]["keys"][0]["column"], "b");
        assert_eq!(indexes[0]["keys"][1]["included"], true);
        assert!(indexes[0]["visible"].is_null());
        assert_eq!(indexes[0]["unique"], true);
    }
}
