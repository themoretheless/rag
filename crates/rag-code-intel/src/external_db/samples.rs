//! Samples require both an owner allowlist and an explicit refresh flag.
use super::*;
fn quoted(name: &str, dialect: Dialect) -> String {
    match dialect {
        Dialect::Postgresql => format!("\"{}\"", name.replace('"', "\"\"")),
        Dialect::Mysql => format!("`{}`", name.replace('`', "``")),
    }
}
fn sensitive(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "password", "passwd", "secret", "token", "email", "phone", "address", "private", "api_key",
        "apikey", "ssn", "birth", "name",
    ]
    .iter()
    .any(|s| n.contains(s))
}
fn display(value: Option<String>) -> Value {
    match value {
        None => Value::Null,
        Some(s) => {
            let lower = s.to_ascii_lowercase();
            if s.contains('@')
                || lower.starts_with("bearer ")
                || s.starts_with("sk-")
                || s.starts_with("eyJ")
            {
                return json!({"redacted":true});
            }
            let truncated = s.chars().count() > 256;
            json!({"value":s.chars().take(256).collect::<String>(),"truncated":truncated})
        }
    }
}
impl Remote {
    pub(super) async fn sample(
        &mut self,
        t: &Target,
        table: &LiveTable,
        budget: &mut usize,
    ) -> Result<Value> {
        let Some(allowlist) = t
            .sample_columns
            .get(&table.schema)
            .and_then(|s| s.get(&table.name))
        else {
            return Ok(json!({"collected":false,"reason":"no_owner_column_allowlist"}));
        };
        if table.kind != "BASE TABLE" || !table.complete {
            return Ok(json!({"collected":false,"reason":"only_complete_base_tables_sampled"}));
        }
        let fields: Vec<_> = table
            .columns
            .iter()
            .filter_map(|c| {
                let name = c["name"].as_str()?;
                let resolved = &c["resolved_type"];
                let kind = resolved["kind"].as_str().unwrap_or("");
                let leaf = resolved["leaf_kind"].as_str().unwrap_or(kind);
                let known = if resolved.is_null() {
                    c["type_schema"].as_str().is_none_or(|s| s == "pg_catalog")
                } else {
                    resolved["complete"] == true
                        && (matches!(kind, "scalar" | "enum" | "set")
                            || kind == "array" && matches!(leaf, "scalar" | "enum"))
                };
                let data_type = if matches!(leaf, "enum" | "set") {
                    leaf.to_string()
                } else {
                    resolved["base_type"]
                        .as_str()
                        .or_else(|| c["data_type"].as_str())
                        .unwrap_or("")
                        .to_ascii_lowercase()
                };
                (known
                    && allowlist.iter().any(|a| a == name)
                    && !sensitive(name)
                    && !["binary", "blob", "bytea", "user-defined"]
                        .iter()
                        .any(|s| data_type.contains(s)))
                .then_some(name)
            })
            .take(16)
            .collect();
        if fields.is_empty() || *budget < 1024 {
            return Ok(
                json!({"collected":false,"reason":"no_permitted_columns_or_sample_budget_exhausted"}),
            );
        }
        let projection = fields
            .iter()
            .map(|n| match t.dialect {
                Dialect::Postgresql => format!("LEFT(CAST({} AS TEXT),257)", quoted(n, t.dialect)),
                Dialect::Mysql => format!("LEFT(CAST({} AS CHAR),257)", quoted(n, t.dialect)),
            })
            .collect::<Vec<_>>()
            .join(",");
        // Identifiers come exclusively from the captured metadata, are quoted independently,
        // and are never interpreted as SQL clauses. No caller predicate/order/expression exists.
        let sql = format!(
            "SELECT {projection} FROM {}.{} LIMIT 5",
            quoted(&table.schema, t.dialect),
            quoted(&table.name, t.dialect)
        );
        let rows: Vec<Value> = match self {
            Self::Pg(c) => {
                let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
                    .fetch_all(c)
                    .await
                    .map_err(|_| remote_error())?;
                rows.iter()
                    .map(|r| {
                        fields
                            .iter()
                            .enumerate()
                            .map(|(i, n)| {
                                Ok((
                                    (*n).to_string(),
                                    display(
                                        r.try_get::<Option<String>, _>(i)
                                            .map_err(|_| remote_error())?,
                                    ),
                                ))
                            })
                            .collect::<Result<serde_json::Map<_, _>>>()
                            .map(Value::Object)
                    })
                    .collect::<Result<_>>()?
            }
            Self::My(c) => {
                let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
                    .fetch_all(c)
                    .await
                    .map_err(|_| remote_error())?;
                rows.iter()
                    .map(|r| {
                        fields
                            .iter()
                            .enumerate()
                            .map(|(i, n)| {
                                Ok((
                                    (*n).to_string(),
                                    display(
                                        r.try_get::<Option<String>, _>(i)
                                            .map_err(|_| remote_error())?,
                                    ),
                                ))
                            })
                            .collect::<Result<serde_json::Map<_, _>>>()
                            .map(Value::Object)
                    })
                    .collect::<Result<_>>()?
            }
        };
        let sample = json!({"collected":true,"columns":fields,"rows":rows,"max_rows":5,"ordered":false,"complete_table":false,"representation":"display strings with explicit null, redaction and truncation","sensitivity_filter":"heuristic plus owner allowlist; not a guarantee of anonymity"});
        let bytes = serde_json::to_vec(&sample)
            .map_err(|_| remote_error())?
            .len();
        if bytes > 16384 || bytes > *budget {
            return Ok(json!({"collected":false,"reason":"encoded_sample_budget_exceeded"}));
        }
        *budget -= bytes;
        Ok(sample)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoting_and_redaction_preserve_null_and_empty() {
        assert_eq!(
            quoted("x\"; DROP TABLE t;--", Dialect::Postgresql),
            "\"x\"\"; DROP TABLE t;--\""
        );
        assert_eq!(
            quoted("x`; DROP TABLE t;--", Dialect::Mysql),
            "`x``; DROP TABLE t;--`"
        );
        assert!(sensitive("api_token"));
        assert!(sensitive("Email"));
        assert!(!sensitive("status"));
        assert_eq!(display(None), Value::Null);
        assert_eq!(display(Some("".into()))["value"], "");
        assert_eq!(
            display(Some("private@example.org".into()))["redacted"],
            true
        );
        assert_eq!(
            display(Some("я".repeat(257)))["value"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            256
        );
    }
}
