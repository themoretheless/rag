//! Pure, bounded CHECK interpreter. Unknown SQL is never evaluated or executed.
use super::*;
use sqlparser::{
    ast::{
        BinaryOperator, Expr, FunctionArg, FunctionArgExpr, FunctionArguments, UnaryOperator,
        Value as SqlValue,
    },
    dialect::{GenericDialect, MySqlDialect},
    keywords::Keyword,
    parser::Parser,
    tokenizer::Token,
};
#[derive(Debug, PartialEq, Clone)]
enum Atom {
    Null,
    Int(i128),
    Bool(bool),
    Text(String),
    Unknown,
}
#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    Pass,
    Fail,
    Unknown,
}
fn literal(v: &Value) -> Atom {
    match v {
        Value::Null => Atom::Null,
        Value::Bool(b) => Atom::Bool(*b),
        Value::Number(n) => n
            .as_i64()
            .map(|v| Atom::Int(v.into()))
            .or_else(|| n.as_u64().map(|v| Atom::Int(v.into())))
            .unwrap_or(Atom::Unknown),
        Value::String(s) => Atom::Text(s.clone()),
        _ => Atom::Unknown,
    }
}
fn truth(v: Atom, mysql: bool) -> Atom {
    match v {
        Atom::Int(n) if mysql => Atom::Bool(n != 0),
        Atom::Bool(_) | Atom::Null => v,
        _ => Atom::Unknown,
    }
}
fn compare(a: Atom, b: Atom, op: &BinaryOperator) -> Atom {
    if matches!(a, Atom::Unknown) || matches!(b, Atom::Unknown) {
        return Atom::Unknown;
    }
    if matches!(a, Atom::Null) || matches!(b, Atom::Null) {
        return Atom::Null;
    }
    let order = match (&a, &b) {
        (Atom::Int(a), Atom::Int(b)) => Some(a.cmp(b)),
        (Atom::Bool(a), Atom::Bool(b)) => Some(a.cmp(b)),
        (Atom::Text(a), Atom::Text(b))
            if a == b && matches!(op, BinaryOperator::Eq | BinaryOperator::NotEq) =>
        {
            Some(std::cmp::Ordering::Equal)
        }
        _ => None,
    };
    let Some(o) = order else {
        return Atom::Unknown;
    };
    Atom::Bool(match op {
        BinaryOperator::Eq => o.is_eq(),
        BinaryOperator::NotEq => !o.is_eq(),
        BinaryOperator::Gt => o.is_gt(),
        BinaryOperator::GtEq => !o.is_lt(),
        BinaryOperator::Lt => o.is_lt(),
        BinaryOperator::LtEq => !o.is_gt(),
        _ => return Atom::Unknown,
    })
}
fn logical(a: Atom, b: Atom, and: bool, mysql: bool) -> Atom {
    match (truth(a, mysql), truth(b, mysql)) {
        (Atom::Unknown, _) | (_, Atom::Unknown) => Atom::Unknown,
        (Atom::Bool(a), Atom::Bool(b)) => Atom::Bool(if and { a && b } else { a || b }),
        (Atom::Bool(false), Atom::Null) | (Atom::Null, Atom::Bool(false)) if and => {
            Atom::Bool(false)
        }
        (Atom::Bool(true), Atom::Null) | (Atom::Null, Atom::Bool(true)) if !and => Atom::Bool(true),
        _ => Atom::Null,
    }
}
fn eval(
    expr: &Expr,
    values: &BTreeMap<String, Value>,
    domain: Option<&Value>,
    mysql: bool,
    budget: &mut usize,
    depth: usize,
) -> Atom {
    if *budget == 0 || depth > 32 {
        return Atom::Unknown;
    }
    *budget -= 1;
    let mut sub = |e: &Expr| eval(e, values, domain, mysql, budget, depth + 1);
    match expr {
        Expr::Identifier(id) => {
            if id.quote_style.is_none()
                && id.value.eq_ignore_ascii_case("value")
                && domain.is_some()
            {
                return literal(domain.unwrap());
            }
            let key = if !mysql && id.quote_style.is_none() {
                id.value.to_lowercase()
            } else {
                id.value.clone()
            };
            values
                .get(&key)
                .or_else(|| {
                    if mysql {
                        let mut found = values
                            .iter()
                            .filter(|(k, _)| k.eq_ignore_ascii_case(&id.value));
                        let first = found.next();
                        if found.next().is_none() {
                            first.map(|(_, v)| v)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .map(literal)
                .unwrap_or(Atom::Unknown)
        }
        Expr::Value(v) => match &v.value {
            SqlValue::Null => Atom::Null,
            SqlValue::Boolean(b) => Atom::Bool(*b),
            SqlValue::Number(n, _) => n.parse().map(Atom::Int).unwrap_or(Atom::Unknown),
            SqlValue::SingleQuotedString(s) => Atom::Text(s.clone()),
            _ => Atom::Unknown,
        },
        Expr::Nested(e) => sub(e),
        Expr::UnaryOp { op, expr } => match (op, sub(expr)) {
            (UnaryOperator::Not, Atom::Bool(b)) => Atom::Bool(!b),
            (UnaryOperator::Not, Atom::Null) => Atom::Null,
            (UnaryOperator::Minus, Atom::Int(n)) => {
                n.checked_neg().map(Atom::Int).unwrap_or(Atom::Unknown)
            }
            (UnaryOperator::Plus, Atom::Int(n)) => Atom::Int(n),
            _ => Atom::Unknown,
        },
        Expr::BinaryOp { left, op, right } => {
            let a = sub(left);
            let b = sub(right);
            match op {
                BinaryOperator::And => logical(a, b, true, mysql),
                BinaryOperator::Or => logical(a, b, false, mysql),
                _ => compare(a, b, op),
            }
        }
        Expr::IsNull(e) => match sub(e) {
            Atom::Unknown => Atom::Unknown,
            Atom::Null => Atom::Bool(true),
            _ => Atom::Bool(false),
        },
        Expr::IsNotNull(e) => match sub(e) {
            Atom::Unknown => Atom::Unknown,
            Atom::Null => Atom::Bool(false),
            _ => Atom::Bool(true),
        },
        Expr::IsTrue(e) => match truth(sub(e), mysql) {
            Atom::Bool(b) => Atom::Bool(b),
            Atom::Null => Atom::Bool(false),
            _ => Atom::Unknown,
        },
        Expr::IsFalse(e) => match truth(sub(e), mysql) {
            Atom::Bool(b) => Atom::Bool(!b),
            Atom::Null => Atom::Bool(false),
            _ => Atom::Unknown,
        },
        Expr::Between {
            expr,
            negated,
            low,
            high,
        } => {
            let value = sub(expr);
            let lo = sub(low);
            let hi = sub(high);
            let result = logical(
                compare(value.clone(), lo, &BinaryOperator::GtEq),
                compare(value, hi, &BinaryOperator::LtEq),
                true,
                mysql,
            );
            match (result, negated) {
                (Atom::Bool(b), true) => Atom::Bool(!b),
                (v, _) => v,
            }
        }
        Expr::InList {
            expr,
            list,
            negated,
        } => {
            let value = sub(expr);
            let mut result = Atom::Bool(false);
            for e in list {
                let other = sub(e);
                result = logical(
                    result,
                    compare(value.clone(), other, &BinaryOperator::Eq),
                    false,
                    mysql,
                );
            }
            match (result, negated) {
                (Atom::Bool(b), true) => Atom::Bool(!b),
                (v, _) => v,
            }
        }
        Expr::Cast {
            expr, data_type, ..
        } => {
            let value = sub(expr);
            let kind = data_type.to_string().to_ascii_lowercase();
            match (kind.as_str(), value) {
                ("text" | "integer" | "int" | "bigint" | "smallint", Atom::Null) => Atom::Null,
                ("text", Atom::Text(s)) => Atom::Text(s),
                ("integer" | "int" | "bigint" | "smallint", Atom::Int(n)) => {
                    let fits = match kind.as_str() {
                        "smallint" => n >= i16::MIN.into() && n <= i16::MAX.into(),
                        "bigint" => n >= i64::MIN.into() && n <= i64::MAX.into(),
                        _ => n >= i32::MIN.into() && n <= i32::MAX.into(),
                    };
                    if fits {
                        Atom::Int(n)
                    } else {
                        Atom::Unknown
                    }
                }
                ("integer" | "int" | "bigint" | "smallint", Atom::Text(s)) => s
                    .parse::<i128>()
                    .ok()
                    .filter(|n| match kind.as_str() {
                        "smallint" => *n >= i16::MIN.into() && *n <= i16::MAX.into(),
                        "bigint" => *n >= i64::MIN.into() && *n <= i64::MAX.into(),
                        _ => *n >= i32::MIN.into() && *n <= i32::MAX.into(),
                    })
                    .map(Atom::Int)
                    .unwrap_or(Atom::Unknown),
                _ => Atom::Unknown,
            }
        }
        Expr::Function(f) => {
            if f.uses_odbc_syntax
                || !matches!(f.parameters, FunctionArguments::None)
                || !f.within_group.is_empty()
                || f.filter.is_some()
                || f.null_treatment.is_some()
                || f.over.is_some()
            {
                return Atom::Unknown;
            }
            let name = f.name.to_string().to_ascii_lowercase();
            if !matches!(
                name.as_str(),
                "length"
                    | "char_length"
                    | "character_length"
                    | "pg_catalog.length"
                    | "pg_catalog.char_length"
                    | "pg_catalog.character_length"
            ) {
                return Atom::Unknown;
            }
            if mysql && name == "length" {
                return Atom::Unknown;
            } // MySQL LENGTH counts encoded bytes.
            let FunctionArguments::List(args) = &f.args else {
                return Atom::Unknown;
            };
            if args.args.len() != 1
                || args.duplicate_treatment.is_some()
                || !args.clauses.is_empty()
            {
                return Atom::Unknown;
            }
            let FunctionArg::Unnamed(FunctionArgExpr::Expr(e)) = &args.args[0] else {
                return Atom::Unknown;
            };
            match sub(e) {
                Atom::Text(s) => Atom::Int(s.chars().count() as i128),
                Atom::Null => Atom::Null,
                _ => Atom::Unknown,
            }
        }
        _ => Atom::Unknown,
    }
}
pub(crate) fn check(
    sql: &str,
    values: &BTreeMap<String, Value>,
    domain: Option<&Value>,
    dialect: &str,
    budget: &mut usize,
) -> Outcome {
    if sql.len() > 8192 || *budget == 0 {
        return Outcome::Unknown;
    }
    let mut sql = sql.trim();
    for _ in 0..2 {
        let upper = sql.to_ascii_uppercase();
        if upper.ends_with(" NOT VALID") {
            sql = sql[..sql.len() - 10].trim_end();
        } else if upper.ends_with(" NO INHERIT") {
            sql = sql[..sql.len() - 11].trim_end();
        } else {
            break;
        }
    }
    let dialect_impl: Box<dyn sqlparser::dialect::Dialect> = if dialect == "mysql" {
        Box::new(MySqlDialect {})
    } else {
        Box::new(GenericDialect {})
    };
    let Ok(mut p) = Parser::new(dialect_impl.as_ref()).try_with_sql(sql) else {
        return Outcome::Unknown;
    };
    let _ = p.parse_keyword(Keyword::CHECK);
    let Ok(expr) = p.parse_expr() else {
        return Outcome::Unknown;
    };
    if p.peek_token().token != Token::EOF {
        return Outcome::Unknown;
    }
    match truth(
        eval(&expr, values, domain, dialect == "mysql", budget, 0),
        dialect == "mysql",
    ) {
        Atom::Bool(false) => Outcome::Fail,
        Atom::Bool(true) | Atom::Null => Outcome::Pass,
        _ => Outcome::Unknown,
    }
}
/// Decode a scalar SQL literal only. No functions, casts or arithmetic are run.
pub(crate) fn scalar_default(sql: &str, budget: &mut usize) -> Option<Value> {
    fn read(expr: &Expr, budget: &mut usize, depth: usize) -> Option<Value> {
        if *budget == 0 || depth > 32 {
            return None;
        }
        *budget -= 1;
        match expr {
            Expr::Value(v) => match &v.value {
                SqlValue::Null => Some(Value::Null),
                SqlValue::Boolean(v) => Some(Value::Bool(*v)),
                SqlValue::SingleQuotedString(v) => Some(Value::String(v.clone())),
                SqlValue::Number(v, _) => v
                    .parse::<i64>()
                    .map(Value::from)
                    .ok()
                    .or_else(|| v.parse::<u64>().map(Value::from).ok()),
                _ => None,
            },
            Expr::Nested(e) => read(e, budget, depth + 1),
            Expr::UnaryOp {
                op: UnaryOperator::Plus,
                expr,
            } => read(expr, budget, depth + 1).filter(Value::is_number),
            Expr::UnaryOp {
                op: UnaryOperator::Minus,
                expr,
            } => {
                let value = read(expr, budget, depth + 1)?;
                let n = value
                    .as_i64()
                    .map(i128::from)
                    .or_else(|| value.as_u64().map(i128::from))?
                    .checked_neg()?;
                i64::try_from(n).ok().map(Value::from)
            }
            _ => None,
        }
    }
    if sql.len() > 8192 || *budget == 0 {
        return None;
    }
    let dialect = GenericDialect {};
    let mut parser = Parser::new(&dialect).try_with_sql(sql).ok()?;
    let expression = parser.parse_expr().ok()?;
    if parser.peek_token().token != Token::EOF {
        return None;
    }
    read(&expression, budget, 0)
}
/// Parse an already captured CHECK declaration, including a constraint name.
/// Returns false in the second component only for explicit NOT ENFORCED.
pub(crate) fn declaration(
    sql: &str,
    values: &BTreeMap<String, Value>,
    budget: &mut usize,
) -> (Outcome, bool) {
    if sql.len() > 8192 || *budget == 0 {
        return (Outcome::Unknown, true);
    }
    let wrapped = format!("CREATE TABLE __rag_check (__rag_column INT, {sql})");
    let Ok(statements) = Parser::parse_sql(&GenericDialect {}, &wrapped) else {
        return (Outcome::Unknown, true);
    };
    if statements.len() != 1 {
        return (Outcome::Unknown, true);
    }
    let sqlparser::ast::Statement::CreateTable(table) = &statements[0] else {
        return (Outcome::Unknown, true);
    };
    if table.columns.len() != 1 || table.constraints.len() != 1 {
        return (Outcome::Unknown, true);
    }
    let sqlparser::ast::TableConstraint::Check(constraint) = &table.constraints[0] else {
        return (Outcome::Unknown, true);
    };
    if constraint.enforced == Some(false) {
        return (Outcome::Pass, false);
    }
    (
        check(
            &format!("CHECK ({})", constraint.expr),
            values,
            None,
            "generic",
            budget,
        ),
        true,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scalar_defaults_are_literals_with_bounded_signs_not_sql_execution() {
        for (sql, expected) in [
            ("-9223372036854775808", json!(i64::MIN)),
            ("+42", json!(42)),
            ("((TRUE))", json!(true)),
            ("NULL", Value::Null),
            ("'it''s'", json!("it's")),
        ] {
            assert_eq!(scalar_default(sql, &mut 100), Some(expected));
        }
        for sql in ["1 + 1", "nextval('x')", "3::INT", "1; SELECT 1"] {
            assert_eq!(scalar_default(sql, &mut 100), None);
        }
        assert_eq!(scalar_default("1", &mut 0), None);
    }
    #[test]
    fn unquoted_postgres_names_fold_without_hiding_quoted_names() {
        let values = BTreeMap::from([("a".into(), json!(7)), ("A".into(), json!(-1))]);
        assert_eq!(
            check("CHECK(A>0)", &values, None, "postgresql", &mut 100),
            Outcome::Pass
        );
        assert_eq!(
            check(r#"CHECK("A">0)"#, &values, None, "postgresql", &mut 100),
            Outcome::Fail
        );
        assert_eq!(
            declaration("CONSTRAINT c CHECK(a>0) NOT ENFORCED", &values, &mut 100),
            (Outcome::Pass, false)
        );
    }
    #[test]
    fn domain_check_uses_sql_null_logic_and_never_calls_functions() {
        for (value, expected) in [
            (json!(7), Outcome::Pass),
            (json!(0), Outcome::Fail),
            (Value::Null, Outcome::Pass),
        ] {
            assert_eq!(
                check(
                    "CHECK (VALUE>0) NOT VALID",
                    &BTreeMap::new(),
                    Some(&value),
                    "postgresql",
                    &mut 100
                ),
                expected
            );
        }
        assert_eq!(
            check(
                "CHECK (dangerous_function(VALUE))",
                &BTreeMap::new(),
                Some(&json!(7)),
                "postgresql",
                &mut 100
            ),
            Outcome::Unknown
        );
        assert_eq!(
            check(
                "CHECK (VALUE > 1); SELECT 1",
                &BTreeMap::new(),
                Some(&json!(7)),
                "postgresql",
                &mut 100
            ),
            Outcome::Unknown
        );
        assert_eq!(
            check(
                "CHECK (VALUE>0)",
                &BTreeMap::new(),
                Some(&json!(7)),
                "postgresql",
                &mut 0
            ),
            Outcome::Unknown
        );
    }
    #[test]
    fn row_predicates_do_not_invent_unassigned_values_or_collations() {
        let values = BTreeMap::from([("a".into(), json!(3)), ("text".into(), json!("ж"))]);
        assert_eq!(
            check(
                "CHECK (a BETWEEN 1 AND 4 AND length(text)=1)",
                &values,
                None,
                "postgresql",
                &mut 100
            ),
            Outcome::Pass
        );
        assert_eq!(
            check("CHECK (a>b)", &values, None, "postgresql", &mut 100),
            Outcome::Unknown
        );
        assert_eq!(
            check("CHECK (text='Ж')", &values, None, "postgresql", &mut 100),
            Outcome::Unknown
        );
        assert_eq!(
            check(
                "CHECK (a >= '-1'::integer)",
                &values,
                None,
                "postgresql",
                &mut 100
            ),
            Outcome::Pass
        );
        assert_eq!(
            check(
                "CHECK (NULL OR FALSE)",
                &values,
                None,
                "postgresql",
                &mut 100
            ),
            Outcome::Pass
        );
        assert_eq!(
            check(
                "CHECK (NULL AND FALSE)",
                &values,
                None,
                "postgresql",
                &mut 100
            ),
            Outcome::Fail
        );
    }
    #[test]
    fn narrowing_and_custom_casts_and_function_modifiers_stay_unverified() {
        let env = BTreeMap::new();
        for sql in [
            "CHECK (40000::smallint > 0)",
            "CHECK (NULL::custom_domain IS NULL)",
            "CHECK (length('x') FILTER (WHERE false) = 1)",
        ] {
            assert_eq!(
                check(sql, &env, None, "postgresql", &mut 100),
                Outcome::Unknown,
                "{sql}"
            );
        }
    }
}
