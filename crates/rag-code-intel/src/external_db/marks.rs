//! Conservative column traits. A composite/partial/expression unique index is not
//! evidence that each participating column is individually unique.
use super::*;
pub(super) fn summary(table: &LiveTable, dialect: &str) -> Value {
    let metadata = table.metadata.as_ref();
    let complete = table.complete && metadata.is_some_and(|m| m.complete);
    let mut unique_keys: Vec<Vec<String>> = vec![];
    let mut primary: Vec<String> = vec![];
    if complete {
        for index in &metadata.unwrap().indexes {
            if index["unique"] != true
                || index["predicate"].is_string()
                || index["valid"] == false
                || index["ready"] == false
            {
                continue;
            }
            if dialect == "postgresql" && (index["valid"] != true || index["ready"] != true) {
                continue;
            }
            let Some(keys) = index["keys"].as_array() else {
                continue;
            };
            let keys = keys
                .iter()
                .filter(|k| k["included"] != true)
                .collect::<Vec<_>>();
            if keys.is_empty()
                || keys.iter().any(|k| {
                    !k["column"].is_string()
                        || k["expression"].is_string()
                        || !k["prefix_length"].is_null()
                        || k["included"].is_null()
                })
            {
                continue;
            }
            let names = keys
                .iter()
                .map(|k| k["column"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>();
            if index["primary"] == true {
                primary = names.clone();
            }
            unique_keys.push(names);
        }
    }
    unique_keys.sort();
    unique_keys.dedup();
    let individually_unique: Vec<_> = unique_keys
        .iter()
        .filter(|k| k.len() == 1)
        .map(|k| k[0].clone())
        .collect();
    let columns:Vec<_>=table.columns.iter().map(|c|{
        let name=c["name"].as_str().unwrap_or("");
        let generation=c["generation_metadata"].as_str().unwrap_or("").to_ascii_lowercase();
        let generated=if dialect=="mysql"{generation.contains("virtual generated")||generation.contains("stored generated")}else{generation=="always"};
        json!({"column":name,"primary_key_member":if complete{Some(primary.iter().any(|n|n==name))}else{None},"single_column_unique_key":if complete{Some(individually_unique.iter().any(|n|n==name))}else{None},"generated_expression":generated,"generation_kind":c["generation_kind"],"identity_kind":c["identity_kind"],"identity":if dialect=="postgresql"{Some(c["identity"]=="YES")}else{None},"auto_increment":if dialect=="mysql"{Some(generation.contains("auto_increment"))}else{None},"source":"captured_column_and_index_metadata"})
    }).collect();
    json!({"complete":complete,"primary_key":if complete{Some(primary)}else{None},"unconditional_named_unique_keys":if complete{Some(unique_keys)}else{None},"individually_unique_columns":if complete{Some(individually_unique)}else{None},"columns":columns,"scope":"captured structural declarations, not row validation or permission; partial/expression/prefix/invalid indexes are excluded from derived uniqueness"})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn composite_partial_expression_and_default_generated_do_not_fabricate_traits() {
        let key = |name: &str| json!({"column":name,"included":false});
        let index = |keys: Vec<Value>, predicate: Value| json!({"unique":true,"valid":true,"ready":true,"predicate":predicate,"keys":keys});
        let table = LiveTable {
            schema: "app".into(),
            name: "t".into(),
            kind: "BASE TABLE".into(),
            columns: vec![
                json!({"name":"a","generation_metadata":"DEFAULT_GENERATED"}),
                json!({"name":"b","generation_metadata":"STORED GENERATED"}),
                json!({"name":"id","generation_metadata":"auto_increment"}),
            ],
            foreign_keys: vec![],
            complete: true,
            sample: Value::Null,
            metadata: Some(metadata::TableMetadata {
                types: None,
                indexes: vec![
                    index(vec![key("a"), key("b")], Value::Null),
                    index(vec![key("a")], json!("a>0")),
                    index(
                        vec![json!({"expression":"lower(a)","included":false})],
                        Value::Null,
                    ),
                    index(vec![key("id")], Value::Null),
                ],
                constraints: vec![],
                view: None,
                complete: true,
                diagnostics: vec![],
            }),
        };
        let s = summary(&table, "mysql");
        assert_eq!(s["individually_unique_columns"], json!(["id"]));
        assert_eq!(s["columns"][0]["generated_expression"], false);
        assert_eq!(s["columns"][1]["generated_expression"], true);
        assert_eq!(s["columns"][2]["auto_increment"], true);
        let mut partial = table;
        partial.complete = false;
        assert!(summary(&partial, "mysql")["individually_unique_columns"].is_null());
    }
}
