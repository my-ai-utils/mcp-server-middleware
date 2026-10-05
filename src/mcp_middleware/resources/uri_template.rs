use std::collections::HashMap;

/// A URI template as RFC 6570 defines it, limited to level 1: literal text
/// plus simple `{name}` expressions, e.g. `docs://rust-extensions/{topic}`.
///
/// Matching goes the other way round from expansion — it takes a concrete
/// URI and extracts the variables:
///
/// - a variable stands for exactly one non-empty piece of a path segment:
///   its raw value never contains `/`, `?` or `#`, because a simple
///   expansion percent-encodes those;
/// - the value is percent-decoded, and a value which decodes to something
///   containing `/`, or to a dot-segment (`.` / `..`), is not a match —
///   a variable can never be used to walk to another path;
/// - when a URI can be split in more than one way, the shortest value wins
///   for the leftmost variable.
pub(crate) struct UriTemplate {
    parts: Vec<UriTemplatePart>,
    literal_len: usize,
}

enum UriTemplatePart {
    Literal(String),
    Variable(String),
}

/// The reserved RFC 6570 operators - all of them are level 2 and higher.
const OPERATORS: &str = "+#./;?&=,!@|";

impl UriTemplate {
    pub fn parse(src: &str) -> Result<Self, String> {
        if src.is_empty() {
            return Err("the template is empty".to_string());
        }

        let mut parts: Vec<UriTemplatePart> = Vec::new();
        let mut literal = String::new();
        let mut chars = src.chars();

        while let Some(c) = chars.next() {
            match c {
                '{' => {
                    let mut name = String::new();
                    loop {
                        match chars.next() {
                            Some('}') => break,
                            Some('{') => return Err("`{` inside an expression".to_string()),
                            Some(c) => name.push(c),
                            None => {
                                return Err("an expression is not closed with `}`".to_string());
                            }
                        }
                    }

                    check_variable_name(name.as_str())?;

                    if !literal.is_empty() {
                        parts.push(UriTemplatePart::Literal(std::mem::take(&mut literal)));
                    } else if let Some(UriTemplatePart::Variable(previous)) = parts.last() {
                        // `{a}{b}` can be split in any place - there is no
                        // telling where one value ends and the next starts.
                        return Err(format!(
                            "the variables `{}` and `{}` are not separated by literal text",
                            previous, name
                        ));
                    }

                    let duplicate = parts.iter().any(|part| match part {
                        UriTemplatePart::Variable(existing) => existing == &name,
                        UriTemplatePart::Literal(_) => false,
                    });

                    if duplicate {
                        return Err(format!("the variable `{}` is used twice", name));
                    }

                    parts.push(UriTemplatePart::Variable(name));
                }
                '}' => return Err("`}` without a matching `{`".to_string()),
                c => literal.push(c),
            }
        }

        if !literal.is_empty() {
            parts.push(UriTemplatePart::Literal(literal));
        }

        let literal_len = parts
            .iter()
            .map(|part| match part {
                UriTemplatePart::Literal(literal) => literal.len(),
                UriTemplatePart::Variable(_) => 0,
            })
            .sum();

        Ok(Self { parts, literal_len })
    }

    /// How much literal text the template has. Of two templates matching
    /// the same URI, the one with more literal text is the more specific one.
    pub fn literal_len(&self) -> usize {
        self.literal_len
    }

    /// The variables of `uri`, or `None` when the URI does not match.
    pub fn match_uri(&self, uri: &str) -> Option<HashMap<String, String>> {
        let mut variables = HashMap::new();

        if match_parts(&self.parts, uri, &mut variables) {
            return Some(variables);
        }

        None
    }
}

fn check_variable_name(name: &str) -> Result<(), String> {
    let Some(first) = name.chars().next() else {
        return Err("an empty expression `{}`".to_string());
    };

    if OPERATORS.contains(first) {
        return Err(format!(
            "the `{}` operator in `{{{}}}` is not supported - only simple `{{name}}` expressions are",
            first, name
        ));
    }

    // Covers the `*` and `:` modifiers and `{a,b}` lists as well.
    if let Some(c) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '_'))
    {
        return Err(format!(
            "`{}` in `{{{}}}` is not supported - a variable name is letters, digits and `_`",
            c, name
        ));
    }

    Ok(())
}

fn match_parts(
    parts: &[UriTemplatePart],
    uri: &str,
    variables: &mut HashMap<String, String>,
) -> bool {
    let Some((part, rest)) = parts.split_first() else {
        return uri.is_empty();
    };

    match part {
        UriTemplatePart::Literal(literal) => match uri.strip_prefix(literal.as_str()) {
            Some(remaining) => match_parts(rest, remaining, variables),
            None => false,
        },
        UriTemplatePart::Variable(name) => {
            // A simple expansion percent-encodes the URI delimiters, so a
            // raw value can only end before the first of them.
            let max_len = uri.find(['/', '?', '#']).unwrap_or(uri.len());

            for end in (1..=max_len).filter(|end| uri.is_char_boundary(*end)) {
                let Some(value) = decode_value(&uri[..end]) else {
                    continue;
                };

                if match_parts(rest, &uri[end..], variables) {
                    variables.insert(name.clone(), value);
                    return true;
                }
            }

            false
        }
    }
}

/// Percent-decodes a raw value. `None` for broken percent-encoding, for
/// bytes which are not UTF-8, and for the values a variable must never
/// carry: anything with a `/` and the dot-segments `.` and `..`.
fn decode_value(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut pos = 0;

    while pos < bytes.len() {
        if bytes[pos] != b'%' {
            decoded.push(bytes[pos]);
            pos += 1;
            continue;
        }

        let high = hex_digit(*bytes.get(pos + 1)?)?;
        let low = hex_digit(*bytes.get(pos + 2)?)?;
        decoded.push(high * 16 + low);
        pos += 3;
    }

    let decoded = String::from_utf8(decoded).ok()?;

    if decoded.contains('/') || decoded == "." || decoded == ".." {
        return None;
    }

    Some(decoded)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matched(template: &str, uri: &str) -> Option<HashMap<String, String>> {
        UriTemplate::parse(template).unwrap().match_uri(uri)
    }

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn a_single_variable_takes_the_last_segment() {
        assert_eq!(
            matched("docs://lib/{topic}", "docs://lib/events-loop"),
            Some(vars(&[("topic", "events-loop")]))
        );
    }

    #[test]
    fn a_template_without_variables_matches_only_itself() {
        assert_eq!(
            matched("docs://lib/index", "docs://lib/index"),
            Some(vars(&[]))
        );
        assert_eq!(matched("docs://lib/index", "docs://lib/index2"), None);
    }

    #[test]
    fn literal_text_after_a_variable_has_to_be_there() {
        assert_eq!(
            matched("docs://lib/{topic}.md", "docs://lib/intro.md"),
            Some(vars(&[("topic", "intro")]))
        );
        assert_eq!(matched("docs://lib/{topic}.md", "docs://lib/intro"), None);
        assert_eq!(matched("docs://lib/{topic}.md", "docs://lib/.md"), None);
    }

    #[test]
    fn variables_in_several_segments() {
        assert_eq!(
            matched(
                "repo://{owner}/{name}/readme",
                "repo://my-ai-utils/mcp/readme"
            ),
            Some(vars(&[("owner", "my-ai-utils"), ("name", "mcp")]))
        );
    }

    #[test]
    fn on_an_ambiguous_split_the_leftmost_value_is_the_shortest() {
        assert_eq!(
            matched("x://{a}-{b}", "x://foo-bar-baz"),
            Some(vars(&[("a", "foo"), ("b", "bar-baz")]))
        );
    }

    #[test]
    fn the_scheme_and_the_literal_prefix_have_to_match() {
        assert_eq!(
            matched("docs://lib/{topic}", "file://lib/events-loop"),
            None
        );
        assert_eq!(
            matched("docs://lib/{topic}", "docs://other/events-loop"),
            None
        );
    }

    #[test]
    fn a_variable_is_never_empty() {
        assert_eq!(matched("docs://lib/{topic}", "docs://lib/"), None);
    }

    #[test]
    fn a_variable_does_not_cross_a_slash_raw_or_encoded() {
        assert_eq!(matched("docs://lib/{topic}", "docs://lib/a/b"), None);
        assert_eq!(matched("docs://lib/{topic}", "docs://lib/a%2Fb"), None);
        assert_eq!(matched("docs://lib/{topic}", "docs://lib/a%2fb"), None);
    }

    #[test]
    fn a_variable_is_never_a_dot_segment() {
        assert_eq!(matched("docs://lib/{topic}", "docs://lib/."), None);
        assert_eq!(matched("docs://lib/{topic}", "docs://lib/.."), None);
        assert_eq!(matched("docs://lib/{topic}", "docs://lib/%2E%2E"), None);
        // Dots inside a name are fine.
        assert_eq!(
            matched("docs://lib/{topic}", "docs://lib/..hidden"),
            Some(vars(&[("topic", "..hidden")]))
        );
    }

    #[test]
    fn a_query_or_a_fragment_the_template_does_not_have_is_not_a_match() {
        assert_eq!(matched("docs://lib/{topic}", "docs://lib/intro?x=1"), None);
        assert_eq!(matched("docs://lib/{topic}", "docs://lib/intro#top"), None);
    }

    #[test]
    fn a_variable_in_the_query() {
        assert_eq!(
            matched(
                "x://search?q={query}&page={page}",
                "x://search?q=a&b&page=2"
            ),
            Some(vars(&[("query", "a&b"), ("page", "2")]))
        );
    }

    #[test]
    fn the_value_is_percent_decoded() {
        assert_eq!(
            matched("docs://lib/{topic}", "docs://lib/hello%20world%3F"),
            Some(vars(&[("topic", "hello world?")]))
        );
        assert_eq!(
            matched(
                "docs://lib/{topic}",
                "docs://lib/%D0%BF%D1%80%D0%B8%D0%B2%D0%B5%D1%82"
            ),
            Some(vars(&[("topic", "привет")]))
        );
    }

    #[test]
    fn a_raw_utf8_value_is_taken_as_is() {
        assert_eq!(
            matched("docs://lib/{topic}.md", "docs://lib/привет.md"),
            Some(vars(&[("topic", "привет")]))
        );
    }

    #[test]
    fn broken_percent_encoding_is_not_a_match() {
        assert_eq!(matched("docs://lib/{topic}", "docs://lib/a%2"), None);
        assert_eq!(matched("docs://lib/{topic}", "docs://lib/a%G1"), None);
        assert_eq!(matched("docs://lib/{topic}", "docs://lib/a%+1"), None);
        // Not UTF-8 once decoded.
        assert_eq!(matched("docs://lib/{topic}", "docs://lib/%FF"), None);
    }

    #[test]
    fn literal_len_counts_only_the_literal_text() {
        assert_eq!(
            UriTemplate::parse("docs://lib/{topic}")
                .unwrap()
                .literal_len(),
            11
        );
        assert_eq!(
            UriTemplate::parse("docs://lib/{topic}.md")
                .unwrap()
                .literal_len(),
            14
        );
    }

    fn parse_error(template: &str) -> String {
        match UriTemplate::parse(template) {
            Ok(_) => panic!("`{}` must be refused", template),
            Err(err) => err,
        }
    }

    #[test]
    fn operators_are_refused() {
        for template in [
            "x://{+path}",
            "x://{#frag}",
            "x://a{.ext}",
            "x://{/segments}",
            "x://{;params}",
            "x://{?query}",
            "x://{&more}",
        ] {
            assert!(parse_error(template).contains("operator"), "{}", template);
        }
    }

    #[test]
    fn modifiers_and_lists_are_refused() {
        assert!(parse_error("x://{list*}").contains('*'));
        assert!(parse_error("x://{name:3}").contains(':'));
        assert!(parse_error("x://{a,b}").contains(','));
    }

    #[test]
    fn malformed_templates_are_refused() {
        assert!(parse_error("").contains("empty"));
        assert!(parse_error("x://{name").contains("not closed"));
        assert!(parse_error("x://name}").contains("without a matching"));
        assert!(parse_error("x://{}").contains("empty expression"));
        assert!(parse_error("x://{{name}}").contains("inside an expression"));
    }

    #[test]
    fn adjacent_and_duplicate_variables_are_refused() {
        assert!(parse_error("x://{a}{b}").contains("not separated"));
        assert!(parse_error("x://{a}-{a}").contains("used twice"));
    }
}
