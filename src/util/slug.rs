/// Identifier policies are deliberately named: similar-looking slugs serve
/// different compatibility contracts in the wiki index and graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlugPolicy {
    WikiPage,
    IndexLookup,
    LinkTarget,
}

pub fn slugify(input: &str, policy: SlugPolicy) -> String {
    let mut output = String::new();
    for character in input.chars() {
        let preserve =
            matches!(policy, SlugPolicy::LinkTarget) && matches!(character, '-' | '_' | '.');
        if character.is_ascii_alphanumeric() || preserve {
            output.push(character.to_ascii_lowercase());
            continue;
        }
        let separator = character.is_whitespace()
            || matches!(character, '-' | '_')
            || (character == '/'
                && matches!(policy, SlugPolicy::WikiPage | SlugPolicy::LinkTarget));
        if separator && !output.ends_with('-') && !output.is_empty() {
            output.push('-');
        }
    }
    let output = output.trim_matches('-').to_string();
    if output.is_empty() && matches!(policy, SlugPolicy::IndexLookup) {
        "page".into()
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policies_preserve_existing_identifier_contracts() {
        assert_eq!(slugify("A / B", SlugPolicy::WikiPage), "a-b");
        assert_eq!(slugify("A / B", SlugPolicy::IndexLookup), "a-b");
        assert_eq!(
            slugify("file_name.md", SlugPolicy::LinkTarget),
            "file_name.md"
        );
        assert_eq!(slugify("///", SlugPolicy::IndexLookup), "page");
        assert_eq!(slugify("///", SlugPolicy::WikiPage), "");
    }
}
