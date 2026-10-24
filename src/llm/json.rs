//! Reading structured JSON out of free-form model output.
//!
//! Models wrap JSON in Markdown fences or surround it with prose. Every
//! LLM-backed workflow (compile, consolidate, organize, maintenance plan) uses
//! these helpers so the tolerance rules and error excerpts stay identical.

use serde::de::DeserializeOwned;

/// Characters of raw model output quoted in a parse error.
pub const ERROR_EXCERPT_CHARS: usize = 400;

/// Remove one surrounding ```` ``` ```` / ```` ```json ```` fence, if present.
pub fn strip_json_fences(raw: &str) -> &str {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest
            .strip_prefix("json")
            .or_else(|| rest.strip_prefix("JSON"))
            .unwrap_or(rest);
        let rest = rest.trim_start();
        rest.strip_suffix("```").unwrap_or(rest).trim()
    } else {
        trimmed
    }
}

/// Parse `T` from the whole unfenced text, else from the outermost
/// `open … close` slice (e.g. `{…}` or `[…]` embedded in prose).
pub fn parse_embedded<T: DeserializeOwned>(unfenced: &str, open: char, close: char) -> Option<T> {
    if let Ok(value) = serde_json::from_str::<T>(unfenced) {
        return Some(value);
    }
    let (start, end) = (unfenced.find(open)?, unfenced.rfind(close)?);
    if end <= start {
        return None;
    }
    serde_json::from_str::<T>(&unfenced[start..=end]).ok()
}

/// Fence-tolerant parse of one JSON object from model output.
pub fn parse_model_object<T: DeserializeOwned>(raw: &str) -> Option<T> {
    parse_embedded(strip_json_fences(raw), '{', '}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct Probe {
        a: u8,
    }

    #[test]
    fn fenced_and_embedded_objects_parse_the_same() {
        let expected = Some(Probe { a: 1 });
        assert_eq!(parse_model_object::<Probe>("{\"a\":1}"), expected);
        assert_eq!(
            parse_model_object::<Probe>("```json\n{\"a\":1}\n```"),
            expected
        );
        assert_eq!(
            parse_model_object::<Probe>("```JSON {\"a\":1}```"),
            expected
        );
        assert_eq!(
            parse_model_object::<Probe>("Sure! Here it is: {\"a\":1} — done."),
            expected
        );
        assert_eq!(parse_model_object::<Probe>("no json"), None);
    }
}
