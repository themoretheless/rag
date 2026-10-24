//! Assignment-plan checking against immutable external metadata; never opens a remote connection.
use super::check_expr::{self, Outcome};
use super::*;
use rmcp::schemars;
use std::collections::BTreeSet;
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum Values {
    Row(BTreeMap<String, Value>),
    Count(usize),
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub table: String,
    pub project: Option<String>,
    pub operation: Option<String>,
    #[serde(default)]
    pub columns: Vec<String>,
    /// Literal row map, or a RAGRAF-compatible VALUES expression count.
    pub values: Option<Values>,
    /// Explicit owner alias selects a captured external schema; omission retains migration mode.
    pub connection: Option<String>,
    pub snapshot_id: Option<String>,
    /// External mode uses exact catalog schema/table/column names, not SQL fragments.
    pub schema: Option<String>,
    pub value_count: Option<usize>,
    /// Assigned columns whose SQL expression is DEFAULT, not a JSON null literal.
    #[serde(default)]
    pub default_columns: Vec<String>,
    #[serde(default)]
    pub overriding_system_value: bool,
}
impl Request {
    fn row(&self) -> Option<&BTreeMap<String, Value>> {
        match &self.values {
            Some(Values::Row(v)) => Some(v),
            _ => None,
        }
    }
    fn count(&self) -> Result<Option<usize>> {
        let alias = match self.values {
            Some(Values::Count(n)) => Some(n),
            _ => None,
        };
        if alias.is_some() && self.value_count.is_some() && alias != self.value_count {
            return Err(AppError::config("values count and value_count disagree"));
        }
        Ok(self.value_count.or(alias))
    }
    fn validate(&self) -> Result<()> {
        if self.columns.len() + self.default_columns.len() + self.row().map_or(0, |v| v.len()) > 256
            || self.count()?.is_some_and(|n| n > 256)
        {
            return Err(AppError::config(
                "write_check supports at most 256 column/value entries",
            ));
        }
        for name in self
            .columns
            .iter()
            .chain(self.default_columns.iter())
            .chain(self.row().into_iter().flat_map(|r| r.keys()))
            .chain(std::iter::once(&self.table))
            .chain(self.schema.iter())
        {
            if name.is_empty() || name.len() > 512 || name.contains('\0') {
                return Err(AppError::config(
                    "invalid or oversized write_check identifier",
                ));
            }
        }
        if self
            .connection
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 64)
            || self.snapshot_id.as_ref().is_some_and(|s| s.len() > 128)
        {
            return Err(AppError::config("invalid external schema selector"));
        }
        let mut nodes = 4096;
        let mut bytes = 65536;
        if let Some(row) = self.row() {
            for (k, v) in row {
                if k.len() > bytes {
                    return Err(AppError::config("literal row exceeds byte budget"));
                }
                bytes -= k.len();
                if !within_budget(v, 0, &mut nodes, &mut bytes) {
                    return Err(AppError::config(
                        "literal row exceeds 64 KiB, 4096 nodes or depth 32",
                    ));
                }
            }
        }
        Ok(())
    }
}
pub(crate) fn within_budget(v: &Value, depth: usize, nodes: &mut usize, bytes: &mut usize) -> bool {
    if *nodes == 0 || depth > 32 {
        return false;
    }
    *nodes -= 1;
    match v {
        Value::String(s) => {
            if s.len() > *bytes {
                return false;
            }
            *bytes -= s.len();
        }
        Value::Array(a) => {
            for v in a {
                if !within_budget(v, depth + 1, nodes, bytes) {
                    return false;
                }
            }
        }
        Value::Object(m) => {
            for (k, v) in m {
                if k.len() > *bytes {
                    return false;
                }
                *bytes -= k.len();
                if !within_budget(v, depth + 1, nodes, bytes) {
                    return false;
                }
            }
        }
        _ => {
            if *bytes < 16 {
                return false;
            }
            *bytes -= 16;
        }
    }
    true
}
fn normalize<'a>(name: &str, table: &'a LiveTable, dialect: &str) -> Option<&'a str> {
    if let Some(c) = table.columns.iter().find(|c| c["name"] == name) {
        return c["name"].as_str();
    }
    if dialect == "mysql" {
        let mut matches = table
            .columns
            .iter()
            .filter_map(|c| c["name"].as_str())
            .filter(|n| n.eq_ignore_ascii_case(name));
        let first = matches.next();
        if matches.next().is_none() {
            return first;
        }
    }
    None
}
fn near(name: &str, names: impl Iterator<Item = String>) -> Vec<String> {
    let a = name.chars().collect::<Vec<_>>();
    if a.len() > 68 {
        return vec![];
    }
    let mut matches = vec![];
    for n in names {
        let b = n.chars().collect::<Vec<_>>();
        if a.len().abs_diff(b.len()) > 3 || b.len() > 68 {
            continue;
        }
        let mut prev = (0..=b.len()).collect::<Vec<_>>();
        for (i, x) in a.iter().enumerate() {
            let mut next = vec![i + 1];
            for (j, y) in b.iter().enumerate() {
                next.push(
                    (prev[j + 1] + 1)
                        .min(next[j] + 1)
                        .min(prev[j] + usize::from(x != y)),
                );
            }
            prev = next;
        }
        if prev[b.len()] <= 3 {
            matches.push((prev[b.len()], n));
        }
    }
    matches.sort();
    matches.into_iter().take(3).map(|(_, n)| n).collect()
}
fn generated(c: &Value, dialect: &str) -> bool {
    let marker = c["generation_metadata"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    if dialect == "mysql" {
        marker.contains("stored generated") || marker.contains("virtual generated")
    } else {
        marker == "always"
    }
}
fn auto(c: &Value, dialect: &str) -> bool {
    dialect == "mysql"
        && c["generation_metadata"]
            .as_str()
            .unwrap_or("")
            .to_ascii_lowercase()
            .contains("auto_increment")
}
fn identity(c: &Value) -> bool {
    c["identity"] == "YES"
}
fn has_default(c: &Value) -> bool {
    !c["default_sql"].is_null() || !c["effective_default_sql"].is_null()
}
fn default_literal(c: &Value, dialect: &str) -> Option<Value> {
    let raw = c["effective_default_sql"]
        .as_str()
        .or_else(|| c["default_sql"].as_str())?;
    if dialect == "mysql" {
        if c["generation_metadata"]
            .as_str()
            .unwrap_or("")
            .contains("DEFAULT_GENERATED")
        {
            return None;
        }
        let base = c["data_type"].as_str().unwrap_or("");
        if base.contains("int") {
            if let Ok(n) = raw.parse::<i64>() {
                return Some(json!(n));
            }
            return raw.parse::<u64>().ok().map(|n| json!(n));
        }
        return Some(json!(raw));
    }
    // Only constant defaults are materialized. Casts, sequences and functions remain unknown.
    let mut p = sqlparser::parser::Parser::new(&sqlparser::dialect::GenericDialect {})
        .try_with_sql(raw)
        .ok()?;
    let expr = p.parse_expr().ok()?;
    if p.peek_token().token != sqlparser::tokenizer::Token::EOF {
        return None;
    }
    use sqlparser::ast::{Expr, UnaryOperator, Value as V};
    match expr {
        Expr::Value(v) => match v.value {
            V::Null => Some(Value::Null),
            V::Boolean(b) => Some(json!(b)),
            V::Number(n, _) => n
                .parse::<i64>()
                .ok()
                .map(|n| json!(n))
                .or_else(|| n.parse::<u64>().ok().map(|n| json!(n))),
            V::SingleQuotedString(s) => Some(json!(s)),
            _ => None,
        },
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr,
        } => {
            if let Expr::Value(v) = *expr {
                if let V::Number(n, _) = v.value {
                    format!("-{n}").parse::<i64>().ok().map(|n| json!(n))
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    }
}
#[derive(Default)]
struct Report {
    issues: Vec<Value>,
    unverified: Vec<Value>,
    checks: Vec<Value>,
}
impl Report {
    fn issue(&mut self, code: &str, column: Option<&str>) {
        self.issues.push(json!({"code":code,"column":column}));
    }
    fn unknown(&mut self, code: &str, column: Option<&str>) {
        if !self
            .unverified
            .iter()
            .any(|v| v["code"] == code && v["column"] == json!(column))
        {
            self.unverified.push(json!({"code":code,"column":column}));
        }
    }
}
fn enum_members<'a>(c: &Value, table: &'a LiveTable) -> Option<&'a Vec<Value>> {
    let types = table.metadata.as_ref()?.types.as_ref()?;
    if c["resolved_type"]["complete"] != true {
        return None;
    }
    let r = &c["resolved_type"];
    if r["kind"] == "enum" && r["base_schema"].is_string() {
        return types.definitions.iter().find(|d| {
            d["schema"] == r["base_schema"] && d["name"] == r["base_name"] && d["complete"] == true
        })?["enum_members"]
            .as_array();
    }
    if matches!(r["kind"].as_str(), Some("enum" | "set")) {
        return types
            .definitions
            .iter()
            .find(|d| d["name"] == c["name"] && d["complete"] == true)?["members"]
            .as_array();
    }
    None
}
fn validate_literal(
    c: &Value,
    value: &Value,
    table: &LiveTable,
    dialect: &str,
    report: &mut Report,
) {
    if value.is_null() {
        return;
    }
    let name = c["name"].as_str().unwrap_or("");
    let r = &c["resolved_type"];
    if r["kind"] == "enum" || r["kind"] == "set" {
        let Some(members) = enum_members(c, table) else {
            report.unknown("ENUM_DEFINITION_UNAVAILABLE", Some(name));
            return;
        };
        let Some(s) = value.as_str() else {
            report.unknown("ENUM_INPUT_REQUIRES_COERCION", Some(name));
            return;
        };
        let ok = if r["kind"] == "set" {
            s.is_empty() || s.split(',').all(|v| members.iter().any(|m| m == v))
        } else {
            members.iter().any(|m| m == s)
        };
        if !ok {
            if dialect == "mysql" {
                report.unknown(
                    "ENUM_OR_SET_MEMBER_NOT_DECLARED_SQL_MODE_OR_COLLATION_UNVERIFIED",
                    Some(name),
                );
            } else {
                report.issue("ENUM_MEMBER_NOT_DECLARED", Some(name));
            }
        }
        return;
    }
    if r["kind"] == "array" {
        report.unknown("ARRAY_LITERAL_VALIDATION_NOT_SUPPORTED", Some(name));
        return;
    }
    let raw = r["base_type"]
        .as_str()
        .or_else(|| c["data_type"].as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let base = raw.split(['(', ' ']).next().unwrap_or("");
    let bits = match base {
        "tinyint" => Some(8),
        "smallint" | "int2" => Some(16),
        "mediumint" => Some(24),
        "int" | "integer" | "int4" => Some(32),
        "bigint" | "int8" => Some(64),
        _ => None,
    };
    if let Some(bits) = bits {
        if dialect == "mysql" && raw.starts_with("tinyint(1)") && value.is_boolean() {
            return;
        }
        let n = value
            .as_i64()
            .map(i128::from)
            .or_else(|| value.as_u64().map(i128::from));
        if let Some(n) = n {
            let unsigned = raw.contains("unsigned");
            let (min, max) = if unsigned {
                (0, (1i128 << bits) - 1)
            } else {
                (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
            };
            if n < min || n > max {
                report.issue("INTEGER_OUT_OF_RANGE", Some(name));
            }
        } else {
            report.unknown("INTEGER_LITERAL_REQUIRES_COERCION", Some(name));
        }
        return;
    }
    if matches!(base, "boolean" | "bool") {
        if !value.is_boolean() {
            report.unknown("BOOLEAN_LITERAL_REQUIRES_COERCION", Some(name));
        }
        return;
    }
    if base == "uuid" {
        if !value
            .as_str()
            .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok())
        {
            report.issue("INVALID_UUID_LITERAL", Some(name));
        }
        return;
    }
    if matches!(
        base,
        "varchar" | "char" | "character" | "text" | "tinytext" | "mediumtext" | "longtext"
    ) {
        let Some(s) = value.as_str() else {
            report.unknown("TEXT_LITERAL_REQUIRES_COERCION", Some(name));
            return;
        };
        if let Some(max) = c["character_maximum_length"]
            .as_str()
            .and_then(|s| s.parse::<usize>().ok())
        {
            if s.chars().count() > max {
                if dialect == "postgresql" && s.trim_end_matches(' ').chars().count() > max {
                    report.issue("TEXT_EXCEEDS_DECLARED_LENGTH", Some(name));
                } else {
                    report.unknown("TEXT_LENGTH_REQUIRES_COERCION", Some(name));
                }
            }
        }
        return;
    }
    if matches!(base, "timestamp" | "datetime" | "date") {
        let valid = value.as_str().is_some_and(|s| {
            if base == "date" {
                chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
            } else {
                chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f").is_ok()
            }
        });
        if !valid {
            report.unknown("TEMPORAL_LITERAL_REQUIRES_COERCION", Some(name));
        }
        return;
    }
    report.unknown("LITERAL_TYPE_VALIDATION_UNSUPPORTED", Some(name));
}
fn evaluate_constraints(
    table: &LiveTable,
    env: &BTreeMap<String, Value>,
    row_requested: bool,
    dialect: &str,
    relevant: &BTreeSet<String>,
    report: &mut Report,
) {
    if !row_requested {
        return;
    }
    let mut nodes = 4096;
    let mut checks = 0usize;
    let mut apply = |constraint: &Value,
                     scope: Value,
                     domain: Option<&Value>,
                     report: &mut Report| {
        if checks >= 512 {
            report.unknown("CHECK_BUDGET_EXHAUSTED", None);
            return;
        }
        checks += 1;
        if constraint["enforced"] == false {
            return;
        }
        let name = constraint["name"].as_str().unwrap_or("unnamed");
        let result = if constraint["text_truncated"] == true || constraint["truncated"] == true {
            Outcome::Unknown
        } else {
            constraint["definition"]
                .as_str()
                .map(|s| check_expr::check(s, env, domain, dialect, &mut nodes))
                .unwrap_or(Outcome::Unknown)
        };
        match result{Outcome::Pass=>report.checks.push(json!({"constraint":name,"scope":scope,"result":"passed"})),Outcome::Fail=>report.issues.push(json!({"code":"CHECK_CONSTRAINT_VIOLATION","constraint":name,"scope":scope})),Outcome::Unknown=>report.unverified.push(json!({"code":"CHECK_EXPRESSION_OR_ROW_CONTEXT_UNVERIFIED","constraint":name,"scope":scope}))}
    };
    if let Some(meta) = &table.metadata {
        for c in meta.constraints.iter().filter(|c| c["kind"] == "CHECK") {
            apply(
                c,
                json!({"kind":"table","schema":table.schema,"table":table.name}),
                None,
                report,
            );
        }
        if let Some(types) = &meta.types {
            for column in &table.columns {
                let Some(name) = column["name"].as_str() else {
                    continue;
                };
                if !relevant.contains(name) {
                    continue;
                }
                let value = env.get(name);
                if let Some(chain) = column["resolved_type"]["domain_chain"].as_array() {
                    for link in chain.iter().filter(|link| link["applies_to"] == "value") {
                        if let Some(d) = types.definitions.iter().find(|d| {
                            d["schema"] == link["type"]["schema"]
                                && d["name"] == link["type"]["name"]
                        }) {
                            if let Some(constraints) = d["constraints"].as_array() {
                                if value.is_none() && !constraints.is_empty() {
                                    report
                                        .unknown("DOMAIN_VALUE_OR_DEFAULT_UNVERIFIED", Some(name));
                                    continue;
                                }
                                for c in constraints {
                                    apply(
                                        c,
                                        json!({"kind":"domain","schema":d["schema"],"type":d["name"],"column":name}),
                                        value,
                                        report,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
impl Store {
    pub fn check_write_plan(&self, q: Request) -> Result<Value> {
        q.validate()?;
        let count = q.count()?;
        if q.connection.is_none() {
            if q.snapshot_id.is_some() || q.schema.is_some() || q.overriding_system_value {
                return Err(AppError::config(
                    "external schema/identity selectors require connection",
                ));
            }
            let row = q.row().cloned();
            let implicit = count.is_some_and(|n| n > 0)
                && q.columns.is_empty()
                && q.default_columns.is_empty()
                && row.is_none();
            let mut result =
                self.code_snapshot()?
                    .write_check(&crate::code_index::sql::WriteCheck {
                        table: q.table,
                        project: q.project,
                        operation: q.operation,
                        columns: q.columns,
                        values: row,
                        default_columns: q.default_columns,
                    })?;
            let assigned = result["checked_columns"].as_array().map_or(0, Vec::len);
            result["value_count"] = json!(count);
            if implicit {
                if let Some(issues) = result["issues"].as_array_mut() {
                    issues.retain(|i| i["code"] != "MISSING_REQUIRED_COLUMNS");
                }
                result["status"] = json!("indeterminate");
                result["valid"] = Value::Null;
                result["unverified"] =
                    json!([{"code":"TARGET_COLUMNS_FOR_VALUE_COUNT_NOT_PROVIDED"}]);
            } else if count.is_some_and(|n| n != assigned) {
                result["issues"].as_array_mut().unwrap().push(json!({"code":"VALUES_COUNT_MISMATCH","target_columns":assigned,"value_count":count}));
                result["status"] = json!("invalid");
                result["valid"] = json!(false);
            }
            return Ok(result);
        }
        let connection = q.connection.as_ref().unwrap();
        let snapshot = match self.external_snapshot(&Query {
            connection: connection.clone(),
            snapshot_id: q.snapshot_id.clone(),
            ..Default::default()
        }) {
            Ok(s) => s,
            Err(AppError::NotFound(_)) => {
                return Ok(
                    json!({"status":"indeterminate","valid":null,"source":"captured_external_metadata","issues":[],"unverified":[{"code":"EXTERNAL_SCHEMA_NOT_CAPTURED"}],"executed":false,"live_query":false}),
                )
            }
            Err(e) => return Err(e),
        };
        if q.project.as_ref().is_some_and(|p| p != &snapshot.project) {
            return Err(AppError::config(
                "requested project does not match captured database project",
            ));
        }
        let op = q.operation.as_deref().unwrap_or("insert");
        if !matches!(op, "insert" | "update" | "delete") {
            return Err(AppError::config(
                "operation must be insert, update or delete",
            ));
        }
        if q.overriding_system_value && (snapshot.dialect != "postgresql" || op != "insert") {
            return Err(AppError::config(
                "overriding_system_value only applies to PostgreSQL inserts",
            ));
        }
        let tables = snapshot
            .tables
            .iter()
            .filter(|t| t.name == q.table && q.schema.as_ref().is_none_or(|v| v == &t.schema))
            .collect::<Vec<_>>();
        let mut report = Report::default();
        let mut assigned = BTreeSet::new();
        let mut defaults = BTreeSet::new();
        let mut literals = BTreeMap::new();
        let mut required = vec![];
        let mut primary = Value::Null;
        let implicit = count.is_some_and(|n| n > 0)
            && q.columns.is_empty()
            && q.row().is_none()
            && q.default_columns.is_empty();
        if tables.len() != 1 {
            report.unknown(
                if tables.is_empty() {
                    "TABLE_NOT_OBSERVED_IN_CAPTURE"
                } else {
                    "TABLE_AMBIGUOUS"
                },
                None,
            );
        } else {
            let table = tables[0];
            if !table.complete || table.metadata.is_none() {
                report.unknown("CAPTURED_TABLE_METADATA_INCOMPLETE", None);
            }
            if table.kind != "BASE TABLE" {
                report.unknown("NON_BASE_TABLE_WRITE_RULES_UNVERIFIED", None);
            }
            primary = marks::summary(table, &snapshot.dialect)["primary_key"].clone();
            let mut insert_name = |raw: &str, duplicate: bool, report: &mut Report| {
                let name = normalize(raw, table, &snapshot.dialect)
                    .unwrap_or(raw)
                    .to_owned();
                if normalize(raw, table, &snapshot.dialect).is_none() {
                    report.issues.push(json!({"code":"UNKNOWN_COLUMN","column":raw,"suggestions":near(raw,table.columns.iter().filter_map(|c|c["name"].as_str()).map(str::to_owned))}));
                }
                if !assigned.insert(name.clone()) && duplicate {
                    report.issue("DUPLICATE_COLUMNS", Some(&name));
                }
                name
            };
            for raw in &q.columns {
                insert_name(raw, true, &mut report);
            }
            for raw in &q.default_columns {
                let name = insert_name(raw, false, &mut report);
                if !defaults.insert(name.clone()) {
                    report.issue("DUPLICATE_DEFAULT_COLUMNS", Some(&name));
                }
            }
            if let Some(row) = q.row() {
                for (raw, value) in row {
                    let name = insert_name(raw, false, &mut report);
                    if literals.insert(name.clone(), value.clone()).is_some() {
                        report.issue("DUPLICATE_NORMALIZED_LITERAL_KEYS", Some(&name));
                    }
                    if defaults.contains(&name) {
                        report.issue("COLUMN_HAS_LITERAL_AND_DEFAULT", Some(&name));
                    }
                }
            }
            if implicit {
                report.unknown("TARGET_COLUMNS_FOR_VALUE_COUNT_NOT_PROVIDED", None);
            } else if count.is_some_and(|n| n != assigned.len()) {
                report.issues.push(json!({"code":"VALUES_COUNT_MISMATCH","target_columns":assigned.len(),"value_count":count}));
            }
            if op == "delete" && !assigned.is_empty() {
                report.issue("DELETE_HAS_NO_ASSIGNED_COLUMNS", None);
            }
            if op == "update" && assigned.is_empty() {
                report.issue("EMPTY_UPDATE", None);
            }
            let mut env = literals.clone();
            if op != "delete" {
                for c in &table.columns {
                    let Some(name) = c["name"].as_str() else {
                        continue;
                    };
                    let is_generated = generated(c, &snapshot.dialect);
                    let is_auto = auto(c, &snapshot.dialect);
                    let is_identity = identity(c);
                    if op == "insert"
                        && c["nullable"] == "NO"
                        && !has_default(c)
                        && !is_generated
                        && !is_auto
                        && !is_identity
                    {
                        required.push(name.to_owned());
                        if !implicit && !assigned.contains(name) {
                            report.issue("MISSING_REQUIRED_COLUMN", Some(name));
                        }
                    }
                    if assigned.contains(name) && !defaults.contains(name) {
                        if is_generated {
                            report.issue("GENERATED_COLUMN_ASSIGNMENT", Some(name));
                        }
                        if is_identity
                            && c["identity_kind"] == "ALWAYS"
                            && !(op == "insert" && q.overriding_system_value)
                        {
                            report.issue("IDENTITY_ALWAYS_EXPLICIT_ASSIGNMENT", Some(name));
                        }
                        if is_identity && c["identity_kind"].is_null() {
                            report.unknown("IDENTITY_MODE_NOT_CAPTURED", Some(name));
                        }
                        if let Some(value) = literals.get(name) {
                            if value.is_null()
                                && c["nullable"] == "NO"
                                && !(snapshot.dialect == "mysql" && is_auto && op == "insert")
                            {
                                report.issue("NULL_NOT_ALLOWED", Some(name));
                            }
                            validate_literal(c, value, table, &snapshot.dialect, &mut report);
                        } else if q.row().is_some() {
                            report.unknown("ASSIGNED_LITERAL_VALUE_NOT_PROVIDED", Some(name));
                        }
                    }
                    if (defaults.contains(name) || (op == "insert" && !assigned.contains(name)))
                        && !is_generated
                        && !is_identity
                        && !is_auto
                    {
                        if let Some(value) = default_literal(c, &snapshot.dialect) {
                            if value.is_null() && c["nullable"] == "NO" {
                                report.issue("NULL_DEFAULT_NOT_ALLOWED", Some(name));
                            }
                            if q.row().is_some() {
                                validate_literal(c, &value, table, &snapshot.dialect, &mut report);
                            }
                            env.insert(name.into(), value);
                        } else if !has_default(c) && c["nullable"] == "YES" {
                            env.insert(name.into(), Value::Null);
                        } else if defaults.contains(name) && !has_default(c) {
                            if snapshot.dialect == "postgresql" {
                                report.issue("DEFAULT_FOR_REQUIRED_COLUMN_IS_NULL", Some(name));
                            } else {
                                report.unknown("MYSQL_IMPLICIT_DEFAULT_UNVERIFIED", Some(name));
                            }
                        }
                    }
                }
                evaluate_constraints(
                    table,
                    &env,
                    q.row().is_some(),
                    &snapshot.dialect,
                    &if op == "insert" {
                        table
                            .columns
                            .iter()
                            .filter_map(|c| c["name"].as_str().map(str::to_owned))
                            .collect()
                    } else {
                        assigned.clone()
                    },
                    &mut report,
                );
            }
        }
        let status = if !report.issues.is_empty() {
            "invalid"
        } else if !report.unverified.is_empty() {
            "indeterminate"
        } else {
            "valid"
        };
        Ok(
            json!({"status":status,"valid":match status{"valid"=>Some(true),"invalid"=>Some(false),_=>None},"source":"captured_external_metadata","snapshot_id":snapshot.id,"capture_format":snapshot.capture_format,"captured_ms":snapshot.captured_ms,"connection":snapshot.connection,"project":snapshot.project,"schema":if tables.len()==1{Some(&tables[0].schema)}else{q.schema.as_ref()},"table":q.table,"table_found":tables.len()==1,"operation":op,"checked_columns":assigned,"default_columns":defaults,"value_count":count,"literal_checks_requested":q.row().is_some(),"issues":report.issues,"unverified":report.unverified,"checked_predicates":report.checks,"all_required_insert_columns":required,"primary_key":primary,"schema_complete":tables.len()==1&&tables[0].complete,"validation_scope":"captured_declarations_and_supported_literal_assignment_plan","not_checked":["generated_and_default_expression_results","live_database_state","actual_unique_or_foreign_key_conflicts","exclusion_constraints","permissions","triggers","row_selection_and_placeholder_order","arbitrary_sql_functions","server_type_coercions_sql_mode_and_character_encoding","temporal_range_and_timezone","collation_equivalent_enum_spellings"],"live_write_success":null,"executed":false,"live_query":false}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("check.duckdb")).unwrap();
        let payload = json!({"capture_format":2,"id":"frozen","connection":"offline","project":"p","dialect":"postgresql","schemas":["public"],"captured_ms":1,"code_index_revision":null,"target_fingerprint":"h","complete_within_visible_scope":true,"diagnostics":[],"tables":[{"schema":"public","name":"t","kind":"BASE TABLE","complete":true,"sample":null,"foreign_keys":[],"columns":[
            {"name":"id","data_type":"integer","nullable":"NO","default_sql":null},
            {"name":"computed","data_type":"integer","nullable":"YES","generation_metadata":"ALWAYS"},
            {"name":"serial","data_type":"integer","nullable":"NO","identity":"YES","identity_kind":"ALWAYS"},
            {"name":"optional","data_type":"text","nullable":"YES"}
        ],"metadata":{"indexes":[],"constraints":[{"name":"positive","kind":"CHECK","definition":"CHECK (id > 0)"}],"view":null,"complete":true,"diagnostics":[]}}]});
        store.lock().unwrap().execute("INSERT INTO external_db_snapshots (id,connection_alias,captured_ms,payload) VALUES (?,?,?,?)",duckdb::params!["frozen","offline",1,payload.to_string()]).unwrap();
        (dir, store)
    }
    fn run(store: &Store, fields: Value) -> Value {
        let mut q = json!({"connection":"offline","table":"t"});
        q.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        store
            .check_write_plan(serde_json::from_value(q).unwrap())
            .unwrap()
    }
    fn code(report: &Value, expected: &str) -> bool {
        report["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["code"] == expected)
    }
    #[test]
    fn frozen_assignment_plan_checks_required_generated_identity_and_count_together() {
        let (_dir, store) = store();
        assert_eq!(run(&store, json!({"values":{"id":1}}))["status"], "valid");
        assert!(code(
            &run(&store, json!({"values":{}})),
            "MISSING_REQUIRED_COLUMN"
        ));
        let r = run(&store, json!({"columns":["id","computed"],"values":1}));
        assert!(code(&r, "GENERATED_COLUMN_ASSIGNMENT") && code(&r, "VALUES_COUNT_MISMATCH"));
        assert!(code(
            &run(&store, json!({"values":{"id":1,"serial":2}})),
            "IDENTITY_ALWAYS_EXPLICIT_ASSIGNMENT"
        ));
        assert_eq!(
            run(
                &store,
                json!({"values":{"id":1,"serial":2},"overriding_system_value":true})
            )["status"],
            "valid"
        );
        assert_eq!(
            run(
                &store,
                json!({"values":{"id":1},"default_columns":["computed","serial"]})
            )["status"],
            "valid"
        );
        assert!(code(
            &run(&store, json!({"values":{"id":null}})),
            "NULL_NOT_ALLOWED"
        ));
        assert!(code(
            &run(&store, json!({"values":{"id":0}})),
            "CHECK_CONSTRAINT_VIOLATION"
        ));
        assert_eq!(
            run(
                &store,
                json!({"operation":"update","values":{"optional":"PRIVATE_LITERAL_MARKER"}})
            )["status"],
            "indeterminate"
        );
        assert!(!run(
            &store,
            json!({"values":{"id":1,"optional":"PRIVATE_LITERAL_MARKER"}})
        )
        .to_string()
        .contains("PRIVATE_LITERAL_MARKER"));
        assert_eq!(run(&store, json!({"values":2}))["status"], "indeterminate");
        assert_eq!(
            run(&store, json!({"connection":"missing"}))["status"],
            "indeterminate"
        );
        assert_eq!(
            run(&store, json!({"table":"t; DROP TABLE t"}))["status"],
            "indeterminate"
        );
    }
    #[test]
    fn request_bounds_and_count_aliases_fail_without_panicking() {
        let q: Request =
            serde_json::from_value(json!({"table":"t","values":{"k":"x".repeat(65536)}})).unwrap();
        assert!(q.validate().is_err());
        let q: Request =
            serde_json::from_value(json!({"table":"t","values":2,"value_count":3})).unwrap();
        assert!(q.validate().is_err());
        assert!(serde_json::from_value::<Request>(json!({"table":"t","url":"secret"})).is_err());
        let q: Request =
            serde_json::from_value(json!({"table":"t","values":2,"value_count":2})).unwrap();
        assert_eq!(q.count().unwrap(), Some(2));
    }
    #[test]
    fn explicit_or_omitted_null_default_cannot_satisfy_not_null() {
        let (_dir, store) = store();
        let mut payload: Value = serde_json::from_str(
            &store
                .lock()
                .unwrap()
                .query_row(
                    "SELECT payload FROM external_db_snapshots WHERE id='frozen'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .unwrap(),
        )
        .unwrap();
        payload["tables"][0]["columns"][0]["default_sql"] = json!("NULL");
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE external_db_snapshots SET payload=? WHERE id='frozen'",
                duckdb::params![payload.to_string()],
            )
            .unwrap();
        for q in [json!({"values":{}}), json!({"default_columns":["id"]})] {
            assert!(code(&run(&store, q), "NULL_DEFAULT_NOT_ALLOWED"));
        }
    }
}
