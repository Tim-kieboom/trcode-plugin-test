//! The filter that makes nextest run some tests and not others (`-E`, a "filterset").
//! A test is known by its binary id and its name, so a test is
//! `binary_id(=the binary) & test(=the name)`, and the tests asked for are those joined
//! with `|`, a binary at a time. A name that has anything but letters, digits and `_:/-.`
//! is written as a regular expression that matches it exactly, since nextest reads a
//! quote after `=` as part of the name.

use std::collections::BTreeMap;

/// The filter for the tests with these ids (a binary id, a space and the name), or nothing
/// when none of the ids is one.
pub(crate) fn expression(ids: &[String]) -> Option<String> {
    let mut binaries: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for id in ids {
        if let Some((binary, name)) = id.split_once(' ') {
            binaries.entry(binary).or_default().push(name);
        }
    }
    let parts: Vec<String> = binaries
        .iter()
        .map(|(binary, names)| {
            let tests: Vec<String> = names
                .iter()
                .map(|name| format!("test({})", matcher(name)))
                .collect();
            match tests.as_slice() {
                [only] => format!("(binary_id({}) & {only})", matcher(binary)),
                several => format!(
                    "(binary_id({}) & ({}))",
                    matcher(binary),
                    several.join(" | ")
                ),
            }
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(" | "))
}

/// What goes inside the parentheses of a predicate to match exactly `text`.
fn matcher(text: &str) -> String {
    let plain = text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_:/-.".contains(c));
    match plain {
        true => format!("={text}"),
        false => format!("/^{}$/", escaped(text)),
    }
}

/// `text` with what a regular expression would read as more than itself written with a
/// `\` before it (and the `/` that ends the expression too).
fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if c.is_ascii_punctuation() && c != '_' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
