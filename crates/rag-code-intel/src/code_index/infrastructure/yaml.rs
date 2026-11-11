//! Bounded YAML AST values retaining original byte anchors. No alias expansion.
use super::*;
#[derive(Clone)]
pub(super) enum Data {
    Scalar(String),
    Map(BTreeMap<String, Yaml>),
    Seq(Vec<Yaml>),
    Block(String),
    Empty,
}
#[derive(Clone)]
pub(super) struct Yaml {
    pub data: Data,
    pub start: usize,
    pub end: usize,
    pub line: usize,
}
impl Yaml {
    pub fn get(&self, key: &str) -> Option<&Self> {
        if let Data::Map(m) = &self.data {
            m.get(key)
        } else {
            None
        }
    }
    pub fn scalar(&self) -> Option<&str> {
        if let Data::Scalar(s) = &self.data {
            Some(s)
        } else {
            None
        }
    }
    pub fn map(&self) -> Option<&BTreeMap<String, Self>> {
        if let Data::Map(m) = &self.data {
            Some(m)
        } else {
            None
        }
    }
    pub fn seq(&self) -> Option<&[Self]> {
        if let Data::Seq(s) = &self.data {
            Some(s)
        } else {
            None
        }
    }
    pub fn at(&self, keys: &[&str]) -> Option<&Self> {
        let mut n = self;
        for key in keys {
            n = n.get(key)?;
        }
        Some(n)
    }
}
fn children(n: Node) -> Vec<Node> {
    let mut c = n.walk();
    n.named_children(&mut c)
        .filter(|n| n.kind() != "comment")
        .collect()
}
fn value(n: Node, s: &str, depth: usize, work: &mut usize) -> Result<Yaml> {
    *work += 1;
    if depth > 64 || *work > 100000 {
        return Err(AppError::Invalid("YAML depth/work limit".into()));
    }
    let raw = text(n, s);
    let mut out = Yaml {
        data: Data::Empty,
        start: n.start_byte(),
        end: n.end_byte(),
        line: n.start_position().row + 1,
    };
    out.data = match n.kind() {
        "document" | "block_node" | "flow_node" | "block_sequence_item" | "plain_scalar" => {
            let ch = children(n);
            if ch.len() != 1 {
                return Err(AppError::Invalid(
                    "unsupported YAML wrapper/tag/anchor".into(),
                ));
            }
            return value(ch[0], s, depth + 1, work);
        }
        "block_mapping" | "flow_mapping" => {
            let mut map = BTreeMap::new();
            for pair in children(n) {
                if !matches!(pair.kind(), "block_mapping_pair" | "flow_pair") {
                    return Err(AppError::Invalid("unsupported YAML mapping".into()));
                }
                let key = pair
                    .child_by_field_name("key")
                    .ok_or_else(|| AppError::Invalid("YAML key missing".into()))?;
                let key = value(key, s, depth + 1, work)?
                    .scalar()
                    .ok_or_else(|| AppError::Invalid("YAML key must be string".into()))?
                    .to_string();
                if key == "<<" {
                    return Err(AppError::Invalid("YAML merge unsupported".into()));
                }
                let item = if let Some(v) = pair.child_by_field_name("value") {
                    value(v, s, depth + 1, work)?
                } else {
                    Yaml {
                        data: Data::Empty,
                        start: pair.end_byte(),
                        end: pair.end_byte(),
                        line: pair.end_position().row + 1,
                    }
                };
                if map.insert(key, item).is_some() {
                    return Err(AppError::Invalid("duplicate YAML key".into()));
                }
            }
            Data::Map(map)
        }
        "block_sequence" | "flow_sequence" => Data::Seq(
            children(n)
                .into_iter()
                .map(|c| value(c, s, depth + 1, work))
                .collect::<Result<Vec<_>>>()?,
        ),
        "string_scalar" => Data::Scalar(raw.into()),
        "single_quote_scalar" => Data::Scalar(raw[1..raw.len() - 1].replace("''", "'")),
        "double_quote_scalar" => Data::Scalar(
            serde_json::from_str::<String>(raw)
                .map_err(|_| AppError::Invalid("unsupported YAML quoted escape".into()))?,
        ),
        "block_scalar" => Data::Block(raw.into()),
        "null_scalar" | "boolean_scalar" | "integer_scalar" | "float_scalar"
        | "timestamp_scalar" => Data::Empty,
        _ => return Err(AppError::Invalid("unsupported YAML form".into())),
    };
    Ok(out)
}
pub(super) fn documents(source: &str) -> Result<Vec<Yaml>> {
    let mut p = Parser::new();
    p.set_language(&tree_sitter_yaml::LANGUAGE.into())
        .map_err(|e| AppError::Invalid(e.to_string()))?;
    let tree = p
        .parse(source, None)
        .ok_or_else(|| AppError::Invalid("YAML parser unavailable".into()))?;
    if tree.root_node().has_error() {
        return Err(AppError::Invalid("YAML syntax error".into()));
    }
    let mut work = 0;
    children(tree.root_node())
        .into_iter()
        .map(|n| value(n, source, 0, &mut work))
        .collect()
}
