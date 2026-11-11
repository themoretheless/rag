use super::*;
/// Delimiters are checked before publishing fields; strings and :: do not split.
fn separators(value: &str, delimiter: u8) -> std::result::Result<Vec<usize>, &'static str> {
    let bytes = value.as_bytes();
    let mut stack = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    let mut found = Vec::new();
    for (i, c) in bytes.iter().copied().enumerate() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        if matches!(c, b'\'' | b'"') {
            quote = Some(c);
            continue;
        }
        match c {
            b'{' | b'[' | b'(' | b'<' => {
                stack.push(c);
                if stack.len() > 32 {
                    return Err("delimiter_depth_limit");
                }
            }
            b'}' | b']' | b')' | b'>' => {
                let expected = match c {
                    b'}' => b'{',
                    b']' => b'[',
                    b')' => b'(',
                    _ => b'<',
                };
                if stack.pop() != Some(expected) {
                    return Err("unbalanced_type_delimiters");
                }
            }
            _ => {}
        }
        if stack.is_empty()
            && c == delimiter
            && !(c == b':' && (bytes.get(i + 1) == Some(&b':') || i > 0 && bytes[i - 1] == b':'))
        {
            found.push(i);
        }
    }
    if quote.is_some() || !stack.is_empty() {
        return Err("unbalanced_type_delimiters");
    }
    Ok(found)
}
fn parameter_position(value: &str) -> std::result::Result<Option<usize>, &'static str> {
    let mut stack = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    for (i, c) in value.as_bytes().iter().copied().enumerate() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        if matches!(c, b'\'' | b'"') {
            quote = Some(c);
            continue;
        }
        if c == b'$' && stack.is_empty() && !value[..i].trim().is_empty() {
            return Ok(Some(i));
        }
        match c {
            b'{' | b'[' | b'(' | b'<' => {
                stack.push(c);
                if stack.len() > 32 {
                    return Err("delimiter_depth_limit");
                }
            }
            b'}' | b']' | b')' | b'>' => {
                let expected = match c {
                    b'}' => b'{',
                    b']' => b'[',
                    b')' => b'(',
                    _ => b'<',
                };
                if stack.pop() != Some(expected) {
                    return Err("unbalanced_type_delimiters");
                }
            }
            _ => {}
        }
    }
    if quote.is_some() || !stack.is_empty() {
        return Err("unbalanced_type_delimiters");
    }
    Ok(None)
}
fn trimmed(value: &str, base: usize) -> (&str, usize) {
    let left = value.len() - value.trim_start().len();
    (value.trim(), base + left)
}
fn key(raw: &str) -> std::result::Result<(Option<String>, String), &'static str> {
    let bytes = raw.as_bytes();
    if bytes.is_empty() {
        return Err("missing_field_key");
    }
    if matches!(bytes[0], b'\'' | b'"') {
        if bytes.last() != Some(&bytes[0]) || bytes.len() < 2 {
            return Err("invalid_quoted_key");
        }
        let value = &raw[1..raw.len() - 1];
        let mut result = String::new();
        let mut chars = value.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                let Some(next) = chars.next() else {
                    return Err("invalid_key_escape");
                };
                if next != '\\' && next != bytes[0] as char {
                    return Err("unsupported_key_escape");
                }
                result.push(next);
            } else if c == bytes[0] as char || c.is_control() {
                return Err("invalid_quoted_key");
            } else {
                result.push(c);
            }
        }
        return Ok((Some(result), "quoted_string".into()));
    }
    if raw
        .trim_start_matches('-')
        .bytes()
        .all(|b| b.is_ascii_digit())
        && raw != "-"
    {
        return Ok((Some(raw.into()), "integer_literal".into()));
    }
    if raw.contains("::")
        && raw
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '\\' | ':'))
    {
        return Ok((None, "constant_expression_unresolved".into()));
    }
    if raw
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && raw
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Ok((Some(raw.into()), "identifier".into()));
    }
    Err("unsupported_field_key_expression")
}
fn shape(
    value: &str,
    base: usize,
    mapped: &Mapped,
    depth: usize,
    budget: &mut usize,
) -> std::result::Result<Option<ShapeContract>, &'static str> {
    if depth > 16 {
        return Err("shape_depth_limit");
    }
    let Some(open) = value.find('{') else {
        return Ok(None);
    };
    let kind = value[..open].trim();
    if !matches!(
        kind,
        "array" | "list" | "object" | "non-empty-array" | "non-empty-list"
    ) {
        return Ok(None);
    }
    separators(value, b',')?;
    // A union/intersection or [] suffix is kept as an opaque full expression,
    // never misrepresented as an unconditional object contract.
    let mut level = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut close = None;
    for (i, c) in value.as_bytes().iter().copied().enumerate().skip(open) {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        if matches!(c, b'\'' | b'"') {
            quote = Some(c);
            continue;
        }
        if c == b'{' {
            level += 1;
        } else if c == b'}' {
            level -= 1;
            if level == 0 {
                close = Some(i);
                break;
            }
        }
    }
    let Some(close) = close else {
        return Err("unbalanced_shape");
    };
    if !value[close + 1..].trim().is_empty() {
        return Ok(None);
    }
    let inside = &value[open + 1..close];
    let mut cuts = separators(inside, b',')?;
    cuts.push(inside.len());
    let mut start = 0;
    let mut fields = Vec::new();
    let mut seen = BTreeSet::new();
    let mut sealed = true;
    let mut extra = None;
    let mut next_index = 0i128;
    for end in cuts {
        let (part, part_base) = trimmed(&inside[start..end], base + open + 1 + start);
        start = end + 1;
        if part.is_empty() {
            if end != inside.len() {
                return Err("empty_shape_field");
            }
            continue;
        }
        if let Some(rest) = part.strip_prefix("...") {
            if end != inside.len() && !inside[end + 1..].trim().is_empty() {
                return Err("rest_must_be_last");
            }
            if kind == "object" {
                return Err("object_rest_unverified");
            }
            let rest = rest.trim();
            if !rest.is_empty() && !(rest.starts_with('<') && rest.ends_with('>')) {
                return Err("unsupported_rest_type");
            }
            sealed = false;
            extra = if rest.is_empty() {
                None
            } else {
                Some(rest[1..rest.len() - 1].to_string())
            };
            continue;
        }
        if !sealed {
            return Err("field_after_rest");
        }
        if fields.len() >= 128 || *budget == 0 {
            return Err("shape_field_limit");
        }
        *budget -= 1;
        let colon = separators(part, b':')?.first().copied();
        let (raw_key, optional, typ, type_base, key_value, key_kind) = if let Some(colon) = colon {
            let raw = part[..colon].trim();
            let optional = raw.ends_with('?');
            let raw = raw.strip_suffix('?').unwrap_or(raw).trim();
            let (key, kind) = key(raw)?;
            let (typ, type_base) = trimmed(&part[colon + 1..], part_base + colon + 1);
            (raw.to_string(), optional, typ, type_base, key, kind)
        } else {
            if kind == "object" {
                return Err("object_field_needs_key");
            }
            let key = next_index.to_string();
            (
                key.clone(),
                false,
                part,
                part_base,
                Some(key),
                "implicit_integer".into(),
            )
        };
        if typ.is_empty() {
            return Err("missing_field_type");
        }
        let key_identity = key_value
            .as_ref()
            .map(|k| format!("literal:{k}"))
            .unwrap_or_else(|| format!("expression:{raw_key}"));
        if !seen.insert(key_identity) {
            return Err("duplicate_shape_key");
        }
        if let Some(n) = key_value.as_ref().and_then(|v| v.parse::<i128>().ok()) {
            next_index = next_index.max(n.saturating_add(1));
        }
        let nested = shape(typ, type_base, mapped, depth + 1, budget)?.map(Box::new);
        fields.push(ShapeField {
            key: key_value,
            key_expression: raw_key,
            key_kind,
            required: !optional,
            type_expression: typ.to_string(),
            nested_shape: nested,
            source: mapped.span(part_base, part_base + part.len()),
        });
    }
    Ok(Some(ShapeContract {
        kind: kind.into(),
        sealed,
        extra_types: extra,
        fields,
    }))
}
pub(super) fn parameter(
    raw: &str,
    base: usize,
    mapped: &Mapped,
    budget: &mut usize,
) -> ParameterContract {
    let mut result = ParameterContract {
        name: None,
        tag: String::new(),
        type_expression: None,
        type_hash: None,
        type_truncated: false,
        by_reference: false,
        variadic: false,
        declaration_match: "unavailable".into(),
        status: "unparsed".into(),
        diagnostic: None,
        shape: None,
        source: mapped.span(base, base + raw.len()),
    };
    let pos = match parameter_position(raw) {
        Ok(Some(pos)) => pos,
        Ok(None) => {
            result.diagnostic = Some("parameter_name_or_type_missing".into());
            return result;
        }
        Err(e) => {
            result.diagnostic = Some(e.into());
            return result;
        }
    };
    let rest = &raw[pos + 1..];
    let length = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    let name = &rest[..length];
    if name.is_empty() || name.len() > 256 || name.chars().next().is_some_and(char::is_numeric) {
        result.diagnostic = Some("invalid_parameter_name".into());
        return result;
    }
    result.name = Some(name.into());
    let (mut typ, type_base) = trimmed(&raw[..pos], base);
    if let Some(t) = typ.strip_suffix("...") {
        result.variadic = true;
        typ = t.trim_end();
    }
    if let Some(t) = typ.strip_suffix('&') {
        result.by_reference = true;
        typ = t.trim_end();
    }
    result.type_hash = Some(content_hash(typ));
    if typ.len() > MAX_TYPE {
        result.status = "limited".into();
        result.type_truncated = true;
        result.diagnostic = Some("type_expression_byte_limit".into());
        return result;
    }
    if typ.is_empty() {
        result.diagnostic = Some("parameter_type_missing".into());
        return result;
    }
    result.type_expression = Some(typ.into());
    match shape(typ, type_base, mapped, 0, budget) {
        Ok(Some(s)) => {
            result.shape = Some(s);
            result.status = "shape".into();
        }
        Ok(None) => {
            result.status = "type_only".into();
            result.diagnostic =
                Some("type_expression_retained_without_shape_or_compiler_resolution".into());
        }
        Err(e) => {
            result.diagnostic = Some(e.into());
            result.status = if e.ends_with("limit") {
                "limited"
            } else {
                "unparsed"
            }
            .into();
        }
    }
    result
}
