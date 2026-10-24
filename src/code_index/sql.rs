//! Offline SQL schema replay and table-use evidence. Never executes SQL.
use super::*;
use rmcp::schemars;
use sqlparser::ast::{self as ast, Spanned, Visit, Visitor};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser as SqlParser;
use std::collections::BTreeMap;
use std::ops::ControlFlow;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Constraint {
    pub name: Option<String>,
    pub kind: String,
    pub definition: String,
    pub not_valid: bool,
}
fn table_constraint(value: &ast::TableConstraint, not_valid: bool) -> Constraint {
    let (kind, name) = match value {
        ast::TableConstraint::Check(c) => ("check", c.name.as_ref()),
        ast::TableConstraint::Unique(c) => ("unique", c.name.as_ref()),
        ast::TableConstraint::PrimaryKey(c) => ("primary_key", c.name.as_ref()),
        ast::TableConstraint::ForeignKey(c) => ("foreign_key", c.name.as_ref()),
        _ => ("other", None),
    };
    Constraint {
        name: name.map(ident),
        kind: kind.into(),
        definition: value.to_string(),
        not_valid,
    }
}
fn sort_constraints(values: &mut [Constraint]) {
    values.sort_by_key(|c| serde_json::to_string(c).unwrap());
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub has_default: bool,
    pub generated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_expression: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_expression: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_clause: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<Constraint>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Change {
    Create {
        columns: Vec<Column>,
        #[serde(default)]
        constraints: Vec<Constraint>,
        conditional: bool,
    },
    Add {
        column: Column,
        conditional: bool,
    },
    DropColumn {
        column: String,
        conditional: bool,
    },
    RenameColumn {
        from: String,
        to: String,
    },
    RenameTable {
        to: String,
    },
    Drop {
        conditional: bool,
    },
    AlterColumn {
        column: String,
        nullable: Option<bool>,
        default: Option<bool>,
        #[serde(default)]
        default_expression: Option<String>,
        data_type: Option<String>,
    },
    AddConstraint {
        constraint: Constraint,
    },
    DropConstraint {
        name: String,
        conditional: bool,
    },
    Unknown {
        reason: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Event {
    table: String,
    change: Change,
    line: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Use {
    table: String,
    mode: String,
    operation: String,
    line: usize,
    symbol: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct SqlDocument {
    events: Vec<Event>,
    uses: Vec<Use>,
    diagnostics: Vec<Diagnostic>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub line: usize,
    pub code: String,
    pub schema_relevant: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    pub document_id: String,
    pub path: String,
    pub line: usize,
    pub revision: i64,
    pub content_hash: String,
    pub project: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Table {
    pub name: String,
    pub project: Option<String>,
    pub columns: BTreeMap<String, Column>,
    pub complete: bool,
    pub constraints: Vec<Constraint>,
    pub evidence: Vec<Evidence>,
}
#[derive(Debug, Serialize)]
pub struct TableUse {
    pub table: String,
    pub mode: String,
    pub operation: String,
    pub symbol: Option<String>,
    pub evidence: Evidence,
}
#[derive(Debug, Default, Serialize)]
pub struct SqlSnapshot {
    pub tables: Vec<Table>,
    pub usages: Vec<TableUse>,
    pub diagnostics: Vec<serde_json::Value>,
    pub schema_sources: Vec<Evidence>,
}
fn ident(id: &ast::Ident) -> String {
    let simple = id
        .value
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && id
            .value
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '$'));
    if id.quote_style.is_some() && (id.value != id.value.to_lowercase() || !simple) {
        format!("\"{}\"", id.value.replace('"', "\"\""))
    } else {
        id.value.to_lowercase()
    }
}
fn name(n: &ast::ObjectName) -> String {
    n.0.iter()
        .map(|p| {
            p.as_ident()
                .map(ident)
                .unwrap_or_else(|| "?dynamic_identifier".into())
        })
        .collect::<Vec<_>>()
        .join(".")
}
fn column(c: &ast::ColumnDef) -> Column {
    let mut out = Column {
        name: ident(&c.name),
        data_type: c.data_type.to_string(),
        nullable: true,
        has_default: false,
        generated: false,
        default_expression: None,
        generated_expression: None,
        generation_clause: None,
        constraints: vec![],
    };
    for option in &c.options {
        let kind = match &option.option {
            ast::ColumnOption::Check(_) => Some("check"),
            ast::ColumnOption::Unique(_) => Some("unique"),
            ast::ColumnOption::PrimaryKey(_) => Some("primary_key"),
            ast::ColumnOption::ForeignKey(_) => Some("foreign_key"),
            _ => None,
        };
        if let Some(kind) = kind {
            out.constraints.push(Constraint {
                name: option.name.as_ref().map(ident),
                kind: kind.into(),
                definition: option.to_string(),
                not_valid: false,
            });
        }
        match &option.option {
            ast::ColumnOption::NotNull => out.nullable = false,
            ast::ColumnOption::Null => out.nullable = true,
            ast::ColumnOption::Default(expr) => {
                out.has_default = true;
                out.default_expression = Some(expr.to_string());
            }
            ast::ColumnOption::Generated {
                generation_expr, ..
            } => {
                out.generated_expression = generation_expr.as_ref().map(ToString::to_string);
                out.generation_clause = Some(option.option.to_string());
                out.generated = true;
                out.has_default = true;
            }
            ast::ColumnOption::DialectSpecific(tokens)
                if tokens.iter().any(|t| {
                    matches!(
                        t.to_string().to_uppercase().as_str(),
                        "AUTOINCREMENT" | "AUTO_INCREMENT"
                    )
                }) =>
            {
                out.has_default = true
            }
            _ => {}
        }
    }
    if out.data_type.to_uppercase().contains("SERIAL") {
        out.has_default = true;
    }
    sort_constraints(&mut out.constraints);
    out
}
fn migration(path: &str) -> bool {
    let path = path.replace('\\', "/");
    is_sql(&path)
        || path
            .split('/')
            .any(|p| matches!(p, "migrations" | "migration"))
        || path.rsplit('/').next().is_some_and(|p| {
            matches!(
                p,
                "schema.rs"
                    | "schema.py"
                    | "schema.ts"
                    | "schema.js"
                    | "migrations.rs"
                    | "migrate.rs"
            )
        })
}
fn ignored_down(path: &str) -> bool {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    path.ends_with(".down.sql")
        || path.ends_with(".rollback.sql")
        || path.contains("/down/")
        || path.contains("/rollback/")
}
fn sqlish(s: &str) -> bool {
    let s = s.trim_start();
    let keyword = s
        .split(|c: char| !c.is_ascii_alphabetic())
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    matches!(
        keyword.as_str(),
        "SELECT"
            | "WITH"
            | "INSERT"
            | "UPDATE"
            | "DELETE"
            | "CREATE"
            | "ALTER"
            | "DROP"
            | "TRUNCATE"
            | "MERGE"
    ) || s.starts_with("--")
        || s.starts_with("/*")
}
fn diag(out: &mut IndexedDocument, line: usize, code: &str, relevant: bool) {
    out.sql.diagnostics.push(Diagnostic {
        line,
        code: code.into(),
        schema_relevant: relevant,
    });
}
fn decode(node: Node, source: &str) -> Option<(String, usize)> {
    let raw = text(node, source);
    if cpp_raw(node, source) {
        let quote = raw.find('"')?;
        let open = raw[quote + 1..].find('(')? + quote + 1;
        let delimiter = &raw[quote + 1..open];
        if delimiter.len() > 16 || delimiter.contains([' ', '\t', '\n', '\r', '\\', ')']) {
            return None;
        }
        let suffix = format!("){delimiter}\"");
        let end = raw.len().checked_sub(suffix.len())?;
        if !raw.ends_with(&suffix) || end < open + 1 {
            return None;
        }
        return Some((
            raw[open + 1..end].into(),
            raw[..open + 1].bytes().filter(|b| *b == b'\n').count(),
        ));
    }
    let start = raw.find(['\'', '"', '`'])?;
    let quote = raw.as_bytes()[start] as char;
    let end = raw.rfind(quote)?;
    if end <= start || raw[..start].contains(['f', 'b']) {
        return None;
    }
    let mut a = start + 1;
    let mut b = end;
    if raw[start..].starts_with("\"\"\"") || raw[start..].starts_with("''' ".trim_end()) {
        a = start + 3;
        b = end.saturating_sub(2);
    }
    if a > b {
        return None;
    }
    let body = &raw[a..b];
    if quote == '`' && body.contains("${") {
        return None;
    }
    if body.contains('\\') && !raw[..start].starts_with('r') {
        if quote == '"' && start == 0 && a == 1 {
            return serde_json::from_str::<String>(raw).ok().map(|s| (s, 0));
        }
        return None;
    }
    Some((
        body.into(),
        raw[..a].bytes().filter(|b| *b == b'\n').count(),
    ))
}
fn cpp_raw(node: Node, source: &str) -> bool {
    if node.kind() != "raw_string_literal" {
        return false;
    }
    let raw = text(node, source);
    raw.find('"')
        .is_some_and(|quote| matches!(&raw[..quote], "R" | "u8R" | "uR" | "UR" | "LR"))
}
pub(super) fn collect(node: Node, source: &str, out: &mut IndexedDocument, owner: Option<&str>) {
    if out.shell.is_some() {
        if matches!(node.kind(), "string" | "raw_string") {
            if node.parent().is_some_and(|p| p.kind() == "concatenation") {
                diag(
                    out,
                    node.start_position().row + 1,
                    "SHELL_SQL_VALUE_UNVERIFIED",
                    migration(&out.path),
                );
                return;
            }
            if let Some(sql) = super::shell::literal(node, source) {
                if sqlish(&sql) {
                    parse_unit(&sql, node.start_position().row + 1, out, owner, true);
                }
            } else if [
                "SELECT ", "INSERT ", "UPDATE ", "DELETE ", "CREATE ", "ALTER ", "DROP ",
            ]
            .iter()
            .any(|prefix| text(node, source).to_uppercase().contains(prefix))
            {
                diag(
                    out,
                    node.start_position().row + 1,
                    "SHELL_SQL_VALUE_UNVERIFIED",
                    migration(&out.path),
                );
            }
        }
        return;
    }
    if !matches!(
        node.kind(),
        "string" | "string_literal" | "raw_string_literal" | "template_string"
    ) {
        return;
    }
    // Java text blocks apply indentation/escape processing beyond ordinary
    // quoted strings. Do not publish table names from an unmodeled value.
    if out.java.is_some() && text(node, source).starts_with("\"\"\"") {
        let raw = text(node, source).to_ascii_uppercase();
        if [
            "SELECT ", "INSERT ", "UPDATE ", "DELETE ", "CREATE ", "ALTER ", "DROP ",
        ]
        .iter()
        .any(|prefix| raw.contains(prefix))
        {
            diag(
                out,
                node.start_position().row + 1,
                "JAVA_SQL_TEXT_BLOCK_VALUE_UNVERIFIED",
                migration(&out.path),
            );
        }
        return;
    }
    // A containing string node owns fragments; interpolation is never evaluated.
    if node.parent().is_some_and(|n| {
        matches!(
            n.kind(),
            "string" | "string_literal" | "raw_string_literal" | "template_string"
        )
    }) {
        return;
    }
    if let Some((sql, offset)) = decode(node, source) {
        if sqlish(&sql) {
            let physical_lines = !text(node, source).contains('\\')
                || text(node, source).starts_with('r')
                || cpp_raw(node, source);
            parse_unit(
                &sql,
                node.start_position().row + 1 + offset,
                out,
                owner,
                physical_lines,
            );
        }
    } else if [
        "SELECT ", "INSERT ", "UPDATE ", "DELETE ", "CREATE ", "ALTER ", "DROP ",
    ]
    .iter()
    .any(|prefix| text(node, source).to_uppercase().contains(prefix))
    {
        diag(
            out,
            node.start_position().row + 1,
            "DYNAMIC_OR_UNSUPPORTED_SQL_LITERAL",
            migration(&out.path),
        );
    }
}
pub(super) fn parse_raw(source: &str, out: &mut IndexedDocument) {
    let mut forward = String::new();
    for line in source.lines() {
        let marker = line.trim().to_ascii_lowercase();
        if matches!(
            marker.as_str(),
            "-- migrate:down" | "-- +goose down" | "-- +migrate down"
        ) {
            break;
        }
        forward.push_str(line);
        forward.push('\n');
    }
    parse_unit(&forward, 1, out, None, true);
}
pub(super) fn finish(source: &str, out: &mut IndexedDocument) {
    if migration(&out.path) && !is_sql(&out.path) {
        if !out.sql.events.is_empty() {
            diag(out, 1, "CODE_MIGRATION_EXECUTION_ORDER_UNVERIFIED", true);
        }
        if [
            "create_table(",
            "drop_table(",
            "add_column(",
            "alter_column(",
            "createTable(",
            "alterTable(",
            "dropTable(",
        ]
        .iter()
        .any(|p| source.contains(p))
        {
            diag(out, 1, "MIGRATION_DSL_NOT_SUPPORTED", true);
        }
    }
}
struct Reads {
    scopes: Vec<HashSet<String>>,
    uses: Vec<(String, usize)>,
    cte_uncertain: bool,
}
impl Visitor for Reads {
    type Break = ();
    fn pre_visit_query(&mut self, q: &ast::Query) -> ControlFlow<()> {
        let aliases = q
            .with
            .as_ref()
            .map(|w| w.cte_tables.iter().map(|c| ident(&c.alias.name)).collect())
            .unwrap_or_default();
        self.scopes.push(aliases);
        ControlFlow::Continue(())
    }
    fn post_visit_query(&mut self, _: &ast::Query) -> ControlFlow<()> {
        self.scopes.pop();
        ControlFlow::Continue(())
    }
    fn pre_visit_table_factor(&mut self, t: &ast::TableFactor) -> ControlFlow<()> {
        if let ast::TableFactor::Table {
            name: n,
            args: None,
            ..
        } = t
        {
            let table = name(n);
            if self.scopes.iter().any(|scope| scope.contains(&table)) {
                self.cte_uncertain = true;
            } else {
                self.uses.push((table, n.span().start.line as usize));
            }
        }
        ControlFlow::Continue(())
    }
}
fn factor(f: &ast::TableFactor) -> Option<String> {
    if let ast::TableFactor::Table { name: n, .. } = f {
        Some(name(n))
    } else {
        None
    }
}
fn parse_unit(
    sql: &str,
    base: usize,
    out: &mut IndexedDocument,
    owner: Option<&str>,
    physical_lines: bool,
) {
    let rollback = owner
        .and_then(|id| out.symbols.iter().find(|s| s.id == id))
        .is_some_and(|s| {
            s.qualified_name
                .split("::")
                .any(|p| matches!(p, "down" | "downgrade"))
        });
    let relevant = migration(&out.path) && !ignored_down(&out.path) && !rollback;
    let statements = match SqlParser::parse_sql(&GenericDialect {}, sql) {
        Ok(s) => s,
        Err(_) => {
            diag(out, base, "SQL_PARSE_ERROR", relevant);
            return;
        }
    };
    for statement in &statements {
        let line = base
            + if physical_lines {
                statement.span().start.line.saturating_sub(1) as usize
            } else {
                0
            };
        let mut add = |table: String, change: Change| {
            if relevant {
                out.sql.events.push(Event {
                    table,
                    change,
                    line,
                });
            }
        };
        match statement {
            ast::Statement::CreateTable(create) => {
                if create.query.is_some() || create.columns.is_empty() {
                    add(
                        name(&create.name),
                        Change::Unknown {
                            reason: "CREATE_TABLE_WITHOUT_EXPLICIT_COLUMNS".into(),
                        },
                    );
                } else {
                    add(
                        name(&create.name),
                        Change::Create {
                            columns: create.columns.iter().map(column).collect(),
                            constraints: {
                                let mut values: Vec<_> = create
                                    .constraints
                                    .iter()
                                    .map(|c| table_constraint(c, false))
                                    .collect();
                                sort_constraints(&mut values);
                                values
                            },
                            conditional: create.if_not_exists,
                        },
                    );
                }
            }
            ast::Statement::AlterTable(alter) => {
                for op in &alter.operations {
                    let change = match op {
                        ast::AlterTableOperation::AddConstraint {
                            constraint,
                            not_valid,
                        } => Change::AddConstraint {
                            constraint: table_constraint(constraint, *not_valid),
                        },
                        ast::AlterTableOperation::DropConstraint {
                            name,
                            if_exists,
                            drop_behavior,
                        } => {
                            if matches!(drop_behavior, Some(ast::DropBehavior::Cascade)) {
                                Change::Unknown {
                                    reason: "CONSTRAINT_CASCADE_EFFECTS_UNVERIFIED".into(),
                                }
                            } else {
                                Change::DropConstraint {
                                    name: ident(name),
                                    conditional: *if_exists,
                                }
                            }
                        }
                        ast::AlterTableOperation::AddColumn {
                            column_def,
                            if_not_exists,
                            ..
                        } => Change::Add {
                            column: column(column_def),
                            conditional: *if_not_exists,
                        },
                        ast::AlterTableOperation::DropColumn {
                            column_names,
                            if_exists,
                            ..
                        } => {
                            for c in column_names {
                                add(
                                    name(&alter.name),
                                    Change::DropColumn {
                                        column: ident(c),
                                        conditional: *if_exists,
                                    },
                                );
                            }
                            continue;
                        }
                        ast::AlterTableOperation::RenameColumn {
                            old_column_name,
                            new_column_name,
                        } => Change::RenameColumn {
                            from: ident(old_column_name),
                            to: ident(new_column_name),
                        },
                        ast::AlterTableOperation::RenameTable { table_name } => {
                            Change::RenameTable {
                                to: match table_name {
                                    ast::RenameTableNameKind::As(n)
                                    | ast::RenameTableNameKind::To(n) => name(n),
                                },
                            }
                        }
                        ast::AlterTableOperation::AlterColumn { column_name, op } => {
                            let (nullable, default, data_type) = match op {
                                ast::AlterColumnOperation::SetNotNull => (Some(false), None, None),
                                ast::AlterColumnOperation::DropNotNull => (Some(true), None, None),
                                ast::AlterColumnOperation::SetDefault { .. } => {
                                    (None, Some(true), None)
                                }
                                ast::AlterColumnOperation::DropDefault => (None, Some(false), None),
                                ast::AlterColumnOperation::SetDataType { data_type, .. } => {
                                    (None, None, Some(data_type.to_string()))
                                }
                                _ => {
                                    add(
                                        name(&alter.name),
                                        Change::Unknown {
                                            reason: "UNSUPPORTED_ALTER_COLUMN".into(),
                                        },
                                    );
                                    continue;
                                }
                            };
                            Change::AlterColumn {
                                column: ident(column_name),
                                nullable,
                                default,
                                default_expression: match op {
                                    ast::AlterColumnOperation::SetDefault { value } => {
                                        Some(value.to_string())
                                    }
                                    _ => None,
                                },
                                data_type,
                            }
                        }
                        _ => Change::Unknown {
                            reason: "UNSUPPORTED_ALTER_TABLE".into(),
                        },
                    };
                    add(name(&alter.name), change);
                }
            }
            ast::Statement::Drop {
                object_type: ast::ObjectType::Table,
                names,
                if_exists,
                ..
            } => {
                for n in names {
                    add(
                        name(n),
                        Change::Drop {
                            conditional: *if_exists,
                        },
                    );
                }
            }
            _ => {}
        }
        let operation = match statement {
            ast::Statement::Insert(_) => "insert",
            ast::Statement::Update(_) => "update",
            ast::Statement::Delete(_) => "delete",
            ast::Statement::Query(_) => "select",
            _ => "other",
        };
        let mut writes = Vec::new();
        match statement {
            ast::Statement::Insert(insert) => {
                if let ast::TableObject::TableName(n) = &insert.table {
                    writes.push(name(n));
                }
            }
            ast::Statement::Update(update) => {
                if let Some(n) = factor(&update.table.relation) {
                    writes.push(n);
                }
            }
            ast::Statement::Delete(delete) => {
                if !delete.tables.is_empty() {
                    diag(out, line, "MULTI_TABLE_DELETE_TARGETS_UNRESOLVED", false);
                } else {
                    let (ast::FromTable::WithFromKeyword(tables)
                    | ast::FromTable::WithoutKeyword(tables)) = &delete.from;
                    for t in tables {
                        if let Some(n) = factor(&t.relation) {
                            writes.push(n);
                        }
                    }
                }
            }
            _ => {}
        }
        let mut reads = Reads {
            scopes: vec![],
            uses: vec![],
            cte_uncertain: false,
        };
        let _ = statement.visit(&mut reads);
        if reads.cte_uncertain {
            diag(out, line, "CTE_REFERENCES_EXCLUDED_SCOPE_LIMITATION", false);
        }
        for (table, sql_line) in reads.uses {
            out.sql.uses.push(Use {
                table,
                mode: "read".into(),
                operation: operation.into(),
                line: base
                    + if physical_lines {
                        sql_line.saturating_sub(1)
                    } else {
                        0
                    },
                symbol: owner.map(str::to_string),
            });
        }
        for table in writes {
            out.sql.uses.push(Use {
                table,
                mode: "write".into(),
                operation: operation.into(),
                line,
                symbol: owner.map(str::to_string),
            });
        }
        if matches!(
            statement,
            ast::Statement::Merge { .. } | ast::Statement::Truncate(_)
        ) {
            diag(out, line, "UNSUPPORTED_WRITE_OPERATION", false);
        }
    }
}
fn qualify_rename(old: &str, new: &str) -> String {
    let dialect = GenericDialect {};
    let parse = |s: &str| {
        SqlParser::new(&dialect)
            .try_with_sql(s)
            .ok()
            .and_then(|mut p| p.parse_object_name(false).ok())
    };
    if let (Some(mut old), Some(new)) = (parse(old), parse(new)) {
        if new.0.len() == 1 && old.0.len() > 1 {
            old.0.pop();
            old.0.extend(new.0);
            return name(&old);
        }
    }
    new.into()
}
fn evidence(id: &str, doc: &IndexedDocument, line: usize) -> Evidence {
    Evidence {
        document_id: id.into(),
        path: doc.path.clone(),
        line,
        revision: doc.revision,
        content_hash: doc.hash.clone(),
        project: doc.project.clone(),
    }
}
pub(super) fn snapshot(docs: &HashMap<String, IndexedDocument>) -> SqlSnapshot {
    let mut ordered: Vec<_> = docs.iter().collect();
    ordered.sort_by(|a, b| (&a.1.project, &a.1.path, a.0).cmp(&(&b.1.project, &b.1.path, b.0)));
    let mut out = SqlSnapshot::default();
    let mut tables: BTreeMap<(Option<String>, String), Table> = BTreeMap::new();
    for (id, doc) in &ordered {
        for diagnostic in &doc.sql.diagnostics {
            out.diagnostics.push(serde_json::json!({"code":diagnostic.code,"schema_relevant":diagnostic.schema_relevant,"evidence":evidence(id,doc,diagnostic.line)}));
        }
        for usage in &doc.sql.uses {
            out.usages.push(TableUse {
                table: usage.table.clone(),
                mode: usage.mode.clone(),
                operation: usage.operation.clone(),
                symbol: usage.symbol.clone(),
                evidence: evidence(id, doc, usage.line),
            });
        }
        for event in &doc.sql.events {
            let key = (doc.project.clone(), event.table.clone());
            let proof = evidence(id, doc, event.line);
            out.schema_sources.push(proof.clone());
            if matches!(
                &event.change,
                Change::RenameColumn { .. }
                    | Change::RenameTable { .. }
                    | Change::DropColumn { .. }
                    | Change::Drop { .. }
            ) && tables.values().any(|t| {
                t.project == doc.project
                    && (!t.constraints.is_empty()
                        || t.columns
                            .values()
                            .any(|c| !c.constraints.is_empty() || c.generated_expression.is_some()))
            }) {
                out.diagnostics.push(serde_json::json!({"code":"SCHEMA_DEPENDENCY_REWRITE_UNVERIFIED","schema_relevant":true,"evidence":proof.clone()}));
            }
            match &event.change {
                Change::Create {
                    columns,
                    constraints,
                    conditional,
                } => {
                    let duplicate_columns = columns
                        .iter()
                        .map(|c| &c.name)
                        .collect::<HashSet<_>>()
                        .len()
                        != columns.len();
                    let constraint_names: Vec<_> = constraints
                        .iter()
                        .chain(columns.iter().flat_map(|c| c.constraints.iter()))
                        .filter_map(|c| c.name.as_ref())
                        .collect();
                    let duplicate_constraints =
                        constraint_names.iter().collect::<HashSet<_>>().len()
                            != constraint_names.len();
                    if duplicate_constraints {
                        out.diagnostics.push(serde_json::json!({"code":"DUPLICATE_CONSTRAINT_NAME","schema_relevant":true,"evidence":proof.clone()}));
                    }
                    if duplicate_columns {
                        out.diagnostics.push(serde_json::json!({"code":"DUPLICATE_COLUMN_DEFINITION","schema_relevant":true,"evidence":proof}));
                    }
                    if let Some(existing) = tables.get_mut(&key) {
                        if !conditional {
                            existing.complete = false;
                            out.diagnostics.push(serde_json::json!({"code":"DUPLICATE_CREATE_TABLE","schema_relevant":true,"evidence":proof}));
                        }
                        continue;
                    }
                    tables.insert(
                        key,
                        Table {
                            name: event.table.clone(),
                            project: doc.project.clone(),
                            columns: columns
                                .iter()
                                .cloned()
                                .map(|c| (c.name.clone(), c))
                                .collect(),
                            complete: !duplicate_columns && !duplicate_constraints,
                            constraints: constraints.clone(),
                            evidence: vec![proof],
                        },
                    );
                }
                Change::Drop { conditional } => {
                    if tables.remove(&key).is_none() && !conditional {
                        out.diagnostics.push(serde_json::json!({"code":"DROP_WITHOUT_KNOWN_TABLE","schema_relevant":true,"evidence":proof}));
                    }
                }
                Change::RenameTable { to } => {
                    let to = qualify_rename(&event.table, to);
                    if let Some(mut t) = tables.remove(&key) {
                        t.name = to.clone();
                        t.evidence.push(proof);
                        if tables.contains_key(&(doc.project.clone(), to.clone())) {
                            t.complete = false;
                        }
                        tables.insert((doc.project.clone(), to.clone()), t);
                    } else {
                        out.diagnostics.push(serde_json::json!({"code":"ALTER_WITHOUT_KNOWN_TABLE","schema_relevant":true,"evidence":proof}));
                    }
                }
                change => {
                    if !tables.contains_key(&key) {
                        out.diagnostics.push(serde_json::json!({"code":"ALTER_WITHOUT_KNOWN_TABLE","schema_relevant":true,"evidence":proof}));
                    }
                    let t = tables.entry(key).or_insert_with(|| Table {
                        name: event.table.clone(),
                        project: doc.project.clone(),
                        columns: BTreeMap::new(),
                        complete: false,
                        constraints: vec![],
                        evidence: vec![],
                    });
                    t.evidence.push(proof.clone());
                    match change {
                        Change::Add {
                            column,
                            conditional,
                        } => {
                            if t.columns.contains_key(&column.name) {
                                if !conditional {
                                    t.complete = false;
                                }
                            } else {
                                t.columns.insert(column.name.clone(), column.clone());
                            }
                        }
                        Change::DropColumn {
                            column,
                            conditional,
                        } => {
                            if t.columns.remove(column).is_none() && !conditional {
                                t.complete = false;
                            }
                        }
                        Change::RenameColumn { from, to } => {
                            if let Some(mut c) = t.columns.remove(from) {
                                c.name = to.clone();
                                if t.columns.insert(to.clone(), c).is_some() {
                                    t.complete = false;
                                }
                            } else {
                                t.complete = false;
                            }
                        }
                        Change::AlterColumn {
                            column,
                            nullable,
                            default,
                            default_expression,
                            data_type,
                        } => {
                            if let Some(c) = t.columns.get_mut(column) {
                                if let Some(n) = nullable {
                                    c.nullable = *n;
                                }
                                if let Some(d) = default {
                                    c.has_default = *d;
                                    c.default_expression =
                                        if *d { default_expression.clone() } else { None };
                                }
                                if let Some(dt) = data_type {
                                    c.data_type = dt.clone();
                                }
                            } else {
                                t.complete = false;
                            }
                        }
                        Change::AddConstraint { constraint } => {
                            if constraint.name.as_ref().is_some_and(|name| {
                                t.constraints
                                    .iter()
                                    .chain(t.columns.values().flat_map(|c| c.constraints.iter()))
                                    .any(|c| c.name.as_ref() == Some(name))
                            }) {
                                t.complete = false;
                                out.diagnostics.push(serde_json::json!({"code":"DUPLICATE_CONSTRAINT_NAME","schema_relevant":true,"evidence":proof}));
                            }
                            t.constraints.push(constraint.clone());
                            sort_constraints(&mut t.constraints);
                        }
                        Change::DropConstraint { name, conditional } => {
                            let mut removed = 0;
                            let before = t.constraints.len();
                            t.constraints.retain(|c| c.name.as_ref() != Some(name));
                            removed += before - t.constraints.len();
                            for col in t.columns.values_mut() {
                                let before = col.constraints.len();
                                col.constraints.retain(|c| c.name.as_ref() != Some(name));
                                removed += before - col.constraints.len();
                            }
                            if removed > 1
                                || (removed == 0
                                    && (!conditional
                                        || t.constraints
                                            .iter()
                                            .chain(
                                                t.columns
                                                    .values()
                                                    .flat_map(|c| c.constraints.iter()),
                                            )
                                            .any(|c| c.name.is_none())))
                            {
                                t.complete = false;
                                out.diagnostics.push(serde_json::json!({"code":"CONSTRAINT_DROP_TARGET_UNVERIFIED","schema_relevant":true,"evidence":proof}));
                            }
                        }
                        Change::Unknown { reason } => {
                            t.complete = false;
                            out.diagnostics.push(serde_json::json!({"code":reason,"schema_relevant":true,"evidence":proof}));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    for table in tables.values() {
        let names: Vec<_> = table
            .constraints
            .iter()
            .chain(table.columns.values().flat_map(|c| c.constraints.iter()))
            .filter_map(|c| c.name.as_ref())
            .collect();
        if names.iter().collect::<HashSet<_>>().len() != names.len() {
            out.diagnostics.push(serde_json::json!({"code":"DUPLICATE_CONSTRAINT_NAME","schema_relevant":true,"evidence":table.evidence.last()}));
        }
    }
    for table in tables.values_mut() {
        if out.diagnostics.iter().any(|d| {
            d["schema_relevant"] == true
                && d["evidence"]["project"] == serde_json::json!(table.project)
        }) {
            table.complete = false;
        }
    }
    out.tables = tables.into_values().collect();
    out.usages.sort_by(|a, b| {
        (&a.evidence.path, a.evidence.line, &a.table, &a.mode).cmp(&(
            &b.evidence.path,
            b.evidence.line,
            &b.table,
            &b.mode,
        ))
    });
    out.usages.dedup_by(|a, b| {
        a.table == b.table
            && a.mode == b.mode
            && a.evidence.document_id == b.evidence.document_id
            && a.evidence.line == b.evidence.line
    });
    out
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct SqlQuery {
    /// Exact stored project/wing; omitted means all projects.
    pub project: Option<String>,
    /// Qualified or unqualified SQL table identifier, with SQL quoting when needed.
    pub table: Option<String>,
    /// table_usage: all (default), read or write.
    pub mode: Option<String>,
    pub limit: Option<usize>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteCheck {
    pub table: String,
    pub project: Option<String>,
    /// insert (default), update or delete. This tool never executes a write.
    pub operation: Option<String>,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub default_columns: Vec<String>,
    /// Optional literal row values: keys become assigned columns and explicit NULLs are checked.
    pub values: Option<BTreeMap<String, serde_json::Value>>,
}
fn canonical(value: &str, single: bool) -> Result<String> {
    if value.len() > 512 {
        return Err(AppError::config("SQL identifier exceeds 512 bytes"));
    }
    let dialect = GenericDialect {};
    let mut parser = SqlParser::new(&dialect)
        .try_with_sql(value)
        .map_err(|_| AppError::config("invalid SQL identifier"))?;
    let object = parser
        .parse_object_name(false)
        .map_err(|_| AppError::config("invalid SQL identifier"))?;
    if parser.peek_token().token != sqlparser::tokenizer::Token::EOF
        || object.0.is_empty()
        || (single && object.0.len() != 1)
        || object.0.iter().any(|p| p.as_ident().is_none())
    {
        return Err(AppError::config(
            "expected a single SQL identifier without trailing SQL",
        ));
    }
    Ok(name(&object))
}
fn table_matches(table: &str, query: &str) -> bool {
    if table == query {
        return true;
    }
    let d = GenericDialect {};
    let Ok(mut p) = SqlParser::new(&d).try_with_sql(query) else {
        return false;
    };
    let Ok(q) = p.parse_object_name(false) else {
        return false;
    };
    if q.0.len() != 1 {
        return false;
    }
    let Ok(mut p) = SqlParser::new(&d).try_with_sql(table) else {
        return false;
    };
    p.parse_object_name(false)
        .ok()
        .and_then(|n| n.0.last().and_then(|p| p.as_ident()).map(ident))
        .is_some_and(|leaf| leaf == query)
}
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<_> = b.chars().collect();
    let mut prev: Vec<_> = (0..=b.len()).collect();
    for (i, x) in a.chars().enumerate() {
        let mut row = vec![i + 1];
        for (j, y) in b.iter().enumerate() {
            row.push(
                (prev[j + 1] + 1)
                    .min(row[j] + 1)
                    .min(prev[j] + usize::from(x != *y)),
            );
        }
        prev = row;
    }
    prev[b.len()]
}
#[derive(Default)]
struct RowChecks {
    defaults: Vec<String>,
    checks: Vec<serde_json::Value>,
    issues: Vec<serde_json::Value>,
    unverified: Vec<serde_json::Value>,
}
fn compatible_scalar_default(column: &Column, value: &serde_json::Value) -> bool {
    if value.is_null() {
        return true;
    }
    match column.data_type.to_ascii_uppercase().as_str() {
        "TEXT" => value.is_string(),
        "BOOLEAN" | "BOOL" => value.is_boolean(),
        "SMALLINT" => value.as_i64().is_some_and(|n| i16::try_from(n).is_ok()),
        "INT" | "INTEGER" => value.as_i64().is_some_and(|n| i32::try_from(n).is_ok()),
        "BIGINT" => value.as_i64().is_some(),
        _ => false,
    }
}
fn check_literal_row(
    table: &Table,
    values: &BTreeMap<String, &serde_json::Value>,
    assigned: &std::collections::BTreeSet<String>,
    operation: &str,
    defaults: &std::collections::BTreeSet<String>,
) -> RowChecks {
    use crate::external_db::check_expr::{self, Outcome};
    let mut out = RowChecks::default();
    let mut env = BTreeMap::new();
    let mut budget = 4096;
    let mut bytes = 65536;
    for (name, column) in &table.columns {
        if budget == 0 {
            out.unverified
                .push(serde_json::json!({"code":"ROW_CONTEXT_BUDGET_EXHAUSTED"}));
            break;
        }

        let value = if let Some(value) = values.get(name) {
            Some((*value).clone())
        } else if defaults.contains(name)
            || (operation == "insert" && !assigned.contains(name) && !column.generated)
        {
            if column.generated {
                out.unverified
                    .push(serde_json::json!({"code":"GENERATED_DEFAULT_UNVERIFIED","column":name}));
                None
            } else if let Some(expression) = &column.default_expression {
                match check_expr::scalar_default(expression, &mut budget) {
                    Some(value) if compatible_scalar_default(column, &value) => {
                        out.defaults.push(name.clone());
                        if value.is_null() && !column.nullable {
                            out.issues.push(serde_json::json!({"code":"NULL_NOT_ALLOWED","column":name,"source":"literal_default"}));
                        }
                        Some(value)
                    }
                    _ => {
                        out.unverified.push(serde_json::json!({"code":"DEFAULT_EXPRESSION_OR_TYPE_UNVERIFIED","column":name}));
                        None
                    }
                }
            } else {
                if defaults.contains(name) && !column.has_default {
                    if !column.nullable {
                        out.unverified.push(serde_json::json!({"code":"UNDECLARED_NOT_NULL_DEFAULT_UNVERIFIED","column":name}));
                        None
                    } else {
                        out.defaults.push(name.clone());
                        Some(serde_json::Value::Null)
                    }
                } else {
                    if defaults.contains(name) {
                        out.unverified.push(
                            serde_json::json!({"code":"IMPLICIT_DEFAULT_UNVERIFIED","column":name}),
                        );
                    }
                    (column.nullable && !column.has_default).then_some(serde_json::Value::Null)
                }
            }
        } else {
            None
        };
        if let Some(value) = value {
            if !crate::external_db::write_check::within_budget(&value, 0, &mut budget, &mut bytes) {
                out.unverified
                    .push(serde_json::json!({"code":"ROW_CONTEXT_BUDGET_EXHAUSTED"}));
                break;
            }

            if let Ok(mut parser) = SqlParser::new(&GenericDialect {}).try_with_sql(name) {
                if let Ok(id) = parser.parse_identifier() {
                    if parser.peek_token().token == sqlparser::tokenizer::Token::EOF {
                        env.insert(id.value, value);
                    }
                }
            }
        }
    }
    for (i, (constraint, column)) in table
        .constraints
        .iter()
        .filter(|c| c.kind == "check")
        .map(|c| (c, None))
        .chain(table.columns.iter().flat_map(|(name, c)| {
            c.constraints
                .iter()
                .filter(|c| c.kind == "check")
                .map(move |c| (c, Some(name)))
        }))
        .enumerate()
    {
        if i >= 512 {
            out.unverified
                .push(serde_json::json!({"code":"CHECK_BUDGET_EXHAUSTED"}));
            break;
        }
        let (result, enforced) = check_expr::declaration(&constraint.definition, &env, &mut budget);
        let scope = serde_json::json!({"constraint":constraint.name,"column":column,"not_valid":constraint.not_valid});
        match result {
            Outcome::Pass=>out.checks.push(serde_json::json!({"scope":scope,"result":if enforced{"passed"}else{"skipped_not_enforced"}})),
            Outcome::Fail=>out.issues.push(serde_json::json!({"code":"CHECK_CONSTRAINT_VIOLATION","scope":scope})),
            Outcome::Unknown=>out.unverified.push(serde_json::json!({"code":"CHECK_EXPRESSION_OR_ROW_CONTEXT_UNVERIFIED","scope":scope})),
        }
    }
    out
}
impl CodeSnapshot {
    pub fn sql_query(&self, operation: &str, q: &SqlQuery) -> Result<serde_json::Value> {
        let table = q
            .table
            .as_deref()
            .map(|t| canonical(t, false))
            .transpose()?;
        let limit = q.limit.unwrap_or(50).clamp(1, 500);
        let mode = q.mode.as_deref().unwrap_or("all");
        if !matches!(mode, "all" | "read" | "write") {
            return Err(AppError::config("mode must be all, read or write"));
        }
        let items: Vec<_> = if operation == "schema" {
            self.sql
                .tables
                .iter()
                .filter(|t| {
                    q.project
                        .as_ref()
                        .is_none_or(|p| t.project.as_ref() == Some(p))
                        && table.as_ref().is_none_or(|n| table_matches(&t.name, n))
                })
                .map(|t| serde_json::to_value(t).unwrap())
                .collect()
        } else if operation == "table_usage" {
            self.sql
                .usages
                .iter()
                .filter(|u| {
                    q.project
                        .as_ref()
                        .is_none_or(|p| u.evidence.project.as_ref() == Some(p))
                        && table.as_ref().is_none_or(|n| table_matches(&u.table, n))
                        && (mode == "all" || mode == u.mode)
                })
                .map(|u| serde_json::to_value(u).unwrap())
                .collect()
        } else {
            return Err(AppError::config("unknown SQL query"));
        };
        let diagnostics: Vec<_> = self
            .sql
            .diagnostics
            .iter()
            .filter(|d| {
                q.project
                    .as_ref()
                    .is_none_or(|p| d["evidence"]["project"] == *p)
            })
            .collect();
        Ok(
            serde_json::json!({"index_rev":self.index_revision,"items":items.iter().take(limit).collect::<Vec<_>>(),"total":items.len(),"truncated":items.len()>limit,"diagnostics":diagnostics,
            "analysis_incomplete":!diagnostics.is_empty()||!self.stale_documents.is_empty()||!self.unindexed_documents.is_empty()||!self.parse_errors.is_empty(),
            "migration_order":"source_path_lexicographic_then_statement_order","source":"derived_from_ingested_sources","stale_documents":self.stale_documents,"unindexed_documents":self.unindexed_documents,"parse_errors":self.parse_errors}),
        )
    }
    pub fn write_check(&self, q: &WriteCheck) -> Result<serde_json::Value> {
        if q.columns.len() + q.default_columns.len() + q.values.as_ref().map_or(0, |v| v.len())
            > 256
        {
            return Err(AppError::config(
                "write_check supports at most 256 columns/values",
            ));
        }
        let requested = canonical(&q.table, false)?;
        let op = q.operation.as_deref().unwrap_or("insert");
        if !matches!(op, "insert" | "update" | "delete") {
            return Err(AppError::config(
                "operation must be insert, update or delete",
            ));
        }
        let tables: Vec<_> = self
            .sql
            .tables
            .iter()
            .filter(|t| {
                q.project
                    .as_ref()
                    .is_none_or(|p| t.project.as_ref() == Some(p))
                    && table_matches(&t.name, &requested)
            })
            .collect();
        let mut constraint_checks = Vec::new();
        let mut inferred_defaults = Vec::new();
        let mut unverified = Vec::new();
        let mut row_nodes = 4096;
        let mut row_bytes = 65536;
        for (name, value) in q.values.iter().flat_map(|m| m.iter()) {
            if name.len() > row_bytes {
                return Err(AppError::config("literal row exceeds byte budget"));
            }
            row_bytes -= name.len();
            if !crate::external_db::write_check::within_budget(
                value,
                0,
                &mut row_nodes,
                &mut row_bytes,
            ) {
                return Err(AppError::config(
                    "literal row exceeds 64 KiB, 4096 nodes or depth 32",
                ));
            }
        }
        let mut issues = Vec::new();
        let mut uncertain = !self.stale_documents.is_empty()
            || !self.unindexed_documents.is_empty()
            || !self.parse_errors.is_empty();
        let mut names = std::collections::BTreeSet::new();
        let mut duplicate = Vec::new();
        for col in &q.columns {
            let n = canonical(col, true)?;
            if !names.insert(n.clone()) {
                duplicate.push(n);
            }
        }
        let mut values = BTreeMap::new();
        if let Some(v) = &q.values {
            for (k, v) in v {
                let n = canonical(k, true)?;
                names.insert(n.clone());
                if values.insert(n, v).is_some() {
                    return Err(AppError::config("duplicate normalized value keys"));
                }
            }
        }
        let mut defaults = std::collections::BTreeSet::new();
        for column in &q.default_columns {
            let name = canonical(column, true)?;
            if values.contains_key(&name) {
                return Err(AppError::config(
                    "a column cannot have both a literal value and DEFAULT",
                ));
            }
            if !defaults.insert(name.clone()) {
                issues.push(serde_json::json!({"code":"DUPLICATE_DEFAULT_COLUMN","column":name}));
            }
            names.insert(name);
        }
        if !duplicate.is_empty() {
            issues.push(serde_json::json!({"code":"DUPLICATE_COLUMNS","columns":duplicate}));
        }
        let mut evidence: Vec<_> = self
            .sql
            .schema_sources
            .iter()
            .filter(|e| {
                q.project
                    .as_ref()
                    .is_none_or(|p| e.project.as_ref() == Some(p))
            })
            .take(500)
            .cloned()
            .collect();
        if tables.len() != 1 {
            let known = self.sql.schema_sources.iter().any(|e| {
                q.project
                    .as_ref()
                    .is_none_or(|p| e.project.as_ref() == Some(p))
            });
            let partial = self.sql.tables.iter().any(|t| {
                q.project
                    .as_ref()
                    .is_none_or(|p| t.project.as_ref() == Some(p))
                    && !t.complete
            });
            let unknown = tables.len() > 1 || !known || partial;
            uncertain |= unknown;
            let mut suggestions: Vec<_> = self
                .sql
                .tables
                .iter()
                .filter(|t| {
                    q.project
                        .as_ref()
                        .is_none_or(|p| t.project.as_ref() == Some(p))
                })
                .map(|t| (distance(&requested, &t.name), &t.name))
                .collect();
            suggestions.sort();
            issues.push(serde_json::json!({"code":if tables.len()>1 {"TABLE_AMBIGUOUS"}else if unknown{"TABLE_SCHEMA_UNKNOWN"}else{"TABLE_NOT_IN_DERIVED_SCHEMA"},
                "candidates":tables.iter().map(|t|serde_json::json!({"name":t.name,"project":t.project})).collect::<Vec<_>>(),"suggestions":suggestions.into_iter().filter(|s|s.0<=3).take(3).map(|s|s.1).collect::<Vec<_>>()}));
        } else {
            let t = tables[0];
            uncertain |= !t.complete;
            evidence = t.evidence.clone();
            let missing: Vec<_> = names
                .iter()
                .filter(|n| !t.columns.contains_key(n.as_str()))
                .cloned()
                .collect();
            for n in missing {
                let mut suggestions: Vec<_> =
                    t.columns.keys().map(|c| (distance(&n, c), c)).collect();
                suggestions.sort();
                issues.push(serde_json::json!({"code":"UNKNOWN_COLUMN","column":n,"suggestions":suggestions.into_iter().filter(|s|s.0<=3).take(3).map(|s|s.1).collect::<Vec<_>>() }));
            }
            if op == "insert" {
                let required: Vec<_> = t
                    .columns
                    .values()
                    .filter(|c| {
                        !c.nullable && !c.has_default && !c.generated && !names.contains(&c.name)
                    })
                    .map(|c| &c.name)
                    .collect();
                if !required.is_empty() {
                    issues.push(
                        serde_json::json!({"code":"MISSING_REQUIRED_COLUMNS","columns":required}),
                    );
                }
            }
            if op == "update" && names.is_empty() {
                issues.push(serde_json::json!({"code":"EMPTY_UPDATE"}));
            }
            if op == "delete" && !names.is_empty() {
                issues.push(serde_json::json!({"code":"DELETE_HAS_NO_ASSIGNED_COLUMNS"}));
            }
            for (n, v) in &values {
                if v.is_null() && t.columns.get(n).is_some_and(|c| !c.nullable) {
                    issues.push(serde_json::json!({"code":"NULL_NOT_ALLOWED","column":n}));
                }
            }
            if (q.values.is_some() || !defaults.is_empty()) && op != "delete" {
                let report = check_literal_row(t, &values, &names, op, &defaults);
                inferred_defaults = report.defaults;
                constraint_checks = report.checks;
                issues.extend(report.issues);
                unverified = report.unverified;
            }
        }
        let effective_project = if tables.len() == 1 {
            Some(&tables[0].project)
        } else {
            None
        };
        let diagnostics: Vec<_> = self
            .sql
            .diagnostics
            .iter()
            .filter(|d| {
                d["schema_relevant"] == true
                    && effective_project.map_or_else(
                        || {
                            q.project
                                .as_ref()
                                .is_none_or(|p| d["evidence"]["project"] == *p)
                        },
                        |p| d["evidence"]["project"] == serde_json::json!(p),
                    )
            })
            .collect();
        uncertain |= !diagnostics.is_empty();
        let schema_complete = !uncertain;
        let validation_uncertain = uncertain || !unverified.is_empty();
        let status = if validation_uncertain {
            "indeterminate"
        } else if issues.is_empty() {
            "valid"
        } else {
            "invalid"
        };
        Ok(
            serde_json::json!({"index_rev":self.index_revision,"status":status,"valid":if validation_uncertain{None}else{Some(issues.is_empty())},"table":requested,"operation":op,"issues":issues,"schema_evidence":evidence,"diagnostics":diagnostics,
            "explicit_default_columns":defaults,"inferred_default_columns":inferred_defaults,"constraint_checks":constraint_checks,"unverified":unverified,"check_evaluation":"bounded_literal_or_default_context","literal_row_requested":q.values.is_some(),"check_evaluator_version":3,"checked_columns":names,"schema_complete":schema_complete,"validation_scope":"derived_table_columns_explicit_not_null_defaults_and_supported_offline_checks","not_checked":["live_database_state","sql_type_coercions","foreign_keys","unique_constraints","check_constraints_outside_supported_offline_subset","permissions","triggers","dialect_specific_identity_rules"],
            "executed":false,"stale_documents":self.stale_documents,"unindexed_documents":self.unindexed_documents,"parse_errors":self.parse_errors}),
        )
    }
}

#[cfg(test)]
mod tests {
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
        let create =
            "CREATE TABLE calc(a INT DEFAULT 1, b INT GENERATED ALWAYS AS (a + 1) STORED);";
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
        let snap=snap(&[
            ("/p/001.sql","CREATE TABLE public.users(name TEXT); ALTER TABLE public.users RENAME TO accounts;",None),
            ("/p/query.rs", "fn fetch() { let q = \"SELECT\\n* FROM public.accounts\"; }",None),
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
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::source_sync::{SourceSyncCommand, SourceSyncService};
    #[tokio::test]
    async fn sql_sync_cache_reuse_stale_guard_and_schema_refresh() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("migrations")).unwrap();
        let migration = root.path().join("migrations/001.sql");
        std::fs::write(&migration, "CREATE TABLE users(name TEXT NOT NULL);").unwrap();
        std::fs::write(
            root.path().join("service.py"),
            "def fetch():\n    return query('SELECT * FROM users')\n",
        )
        .unwrap();
        let config = crate::Config {
            db_path: db.path().join("sql.duckdb"),
            ingest_roots: vec![root.path().to_path_buf()],
            embedding_dims: 16,
            ..crate::Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let embedder: std::sync::Arc<dyn crate::embeddings::EmbeddingProvider> =
            std::sync::Arc::new(crate::embeddings::MockEmbedder::new(16));
        let service = SourceSyncService::new(&store, &embedder, &config);
        let command = SourceSyncCommand::new(root.path().to_path_buf());
        let report = service.sync(command.clone()).await.unwrap();
        assert!(report.errors.is_empty(), "{report:?}");
        assert_eq!(report.code_index.unwrap().indexed, 2);
        let check = WriteCheck {
            table: "users".into(),
            project: None,
            operation: None,
            columns: vec!["name".into()],
            values: None,
            default_columns: vec![],
        };
        assert_eq!(
            store.code_snapshot().unwrap().write_check(&check).unwrap()["status"],
            "valid"
        );
        assert_eq!(
            service
                .sync(command.clone())
                .await
                .unwrap()
                .code_index
                .unwrap()
                .unchanged,
            2
        );
        let mut doc = store
            .list_documents()
            .unwrap()
            .into_iter()
            .find(|d| {
                d.source_file
                    .as_ref()
                    .is_some_and(|p| p.ends_with("001.sql"))
            })
            .unwrap();
        doc.content = "CREATE TABLE users(name TEXT NOT NULL, age INT NOT NULL);".into();
        store.upsert_document(&doc).unwrap();
        assert_eq!(
            store.code_snapshot().unwrap().write_check(&check).unwrap()["status"],
            "indeterminate"
        );
        std::fs::write(&migration, &doc.content).unwrap();
        let report = service.sync(command).await.unwrap();
        assert!(report.errors.is_empty(), "{report:?}");
        let snap = store.code_snapshot().unwrap();
        assert!(snap.stale_documents.is_empty());
        assert_eq!(snap.write_check(&check).unwrap()["status"], "invalid");
        let mut complete_check = check;
        complete_check.columns.push("age".into());
        assert_eq!(
            snap.write_check(&complete_check).unwrap()["status"],
            "valid"
        );
        assert!(snap
            .sql
            .usages
            .iter()
            .any(|u| u.table == "users" && u.symbol.is_some()));
        drop(service);
        drop(store);
        let store = Store::open(&config.db_path).unwrap();
        assert_eq!(
            store
                .code_snapshot()
                .unwrap()
                .write_check(&complete_check)
                .unwrap()["status"],
            "valid"
        );
    }
}
