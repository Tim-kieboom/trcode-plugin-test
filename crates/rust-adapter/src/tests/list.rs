use trcode_test_protocol::TestItem;

use super::fixture;
use crate::list::ListScanner;

fn scan(pieces: impl IntoIterator<Item = String>) -> Result<Vec<TestItem>, String> {
    let mut scanner = ListScanner::default();
    for piece in pieces {
        scanner.feed(&piece);
    }
    scanner.finish().map(|table| table.items(0, usize::MAX))
}

fn ids(tests: &[TestItem]) -> Vec<&str> {
    tests.iter().map(|test| test.id.as_str()).collect()
}

/// A document of suites, each with tests `(name, ignored)`, in the shape nextest writes.
fn document(suites: &[(&str, &[(&str, bool)])]) -> String {
    let suites: Vec<String> = suites
        .iter()
        .map(|(suite, tests)| {
            let tests: Vec<String> = tests
                .iter()
                .map(|(name, ignored)| {
                    format!(
                        r#""{name}":{{"kind":"test","ignored":{ignored},"filter-match":{{"status":"matches"}}}}"#
                    )
                })
                .collect();
            format!(
                r#""{suite}":{{"binary-id":"{suite}","kind":"lib","testcases":{{{}}}}}"#,
                tests.join(",")
            )
        })
        .collect();
    format!(
        r#"{{"rust-build-meta":{{"x":[1,2,{{"y":"z"}}]}},"test-count":1,"rust-suites":{{{}}}}}"#,
        suites.join(",")
    )
}

#[test]
fn the_real_listing_gives_every_test_that_is_not_ignored_with_its_binary() {
    let tests = scan([fixture("list.json")]).unwrap();

    let mut found = ids(&tests);
    found.sort_unstable();
    assert_eq!(
        found,
        [
            "fx tests::fails_here",
            "fx tests::panics_ok",
            "fx tests::parses_empty",
            "fx::bin/fx tests::parses_empty",
            "fx::it parses_empty",
        ]
    );
    let bin = tests
        .iter()
        .find(|test| test.group == "fx::bin/fx")
        .unwrap();
    assert_eq!(bin.name, "tests::parses_empty");
}

#[test]
fn the_same_tests_come_however_the_document_is_cut_into_pieces() {
    let text = fixture("list.json");
    let whole = scan([text.clone()]).unwrap();

    let by_char = scan(text.chars().map(String::from)).unwrap();
    let by_seven: Vec<String> = text
        .chars()
        .collect::<Vec<_>>()
        .chunks(7)
        .map(|piece| piece.iter().collect())
        .collect();

    assert_eq!(by_char, whole);
    assert_eq!(scan(by_seven).unwrap(), whole);
}

#[test]
fn ignored_tests_are_left_out_whichever_side_of_the_flag_the_other_keys_are() {
    let text = r#"{"rust-suites":{"a":{"testcases":{"x":{"ignored":true,"kind":"test"},"y":{"kind":"test","ignored":false},"z":{"kind":"test","ignored":true}}}}}"#;

    let tests = scan([text.to_owned()]).unwrap();

    assert_eq!(ids(&tests), ["a y"]);
}

#[test]
fn names_with_quotes_braces_colons_and_unicode_escapes_are_read_as_they_are() {
    let text = r#"{"rust-suites":{"a::b/c-d":{"testcases":{"t{1}:x,y":{"ignored":false},"q\"uote":{"ignored":false},"café":{"ignored":false},"smile😀":{"ignored":false}}}}}"#;

    let tests = scan([text.to_owned()]).unwrap();

    let names: Vec<&str> = tests.iter().map(|test| test.name.as_str()).collect();
    assert_eq!(names, ["t{1}:x,y", "q\"uote", "café", "smile😀"]);
    assert!(tests.iter().all(|test| test.group == "a::b/c-d"));
    assert_eq!(tests[0].id, "a::b/c-d t{1}:x,y");
}

#[test]
fn a_document_with_no_suites_or_no_tests_gives_none() {
    assert_eq!(scan(["{}".to_owned()]).unwrap(), []);
    assert_eq!(scan([r#"{"rust-suites":{}}"#.to_owned()]).unwrap(), []);
    assert_eq!(scan([document(&[("a", &[])])]).unwrap(), []);
}

#[test]
fn other_parts_of_the_document_with_the_same_key_names_are_not_tests() {
    let text = r#"{"testcases":{"loose":{"ignored":false}},"other":{"rust-suites":{"s":{"testcases":{"inner":{}}}}},"rust-suites":{}}"#;

    assert_eq!(scan([text.to_owned()]).unwrap(), []);
}

#[test]
fn a_document_that_stops_in_the_middle_is_an_error_not_a_short_list() {
    let text = document(&[("a", &[("x", false), ("y", false)])]);
    for cut in [text.len() / 2, text.len() - 1, 5] {
        assert!(scan([text[..cut].to_owned()]).is_err(), "cut at {cut}");
    }
    let mut string_open = String::from(r#"{"rust-suites":{"a"#);
    assert!(scan([std::mem::take(&mut string_open)]).is_err());
}

#[test]
fn twenty_thousand_tests_are_read_in_pieces_the_size_the_editor_sends() {
    let names: Vec<String> = (0..20_000).map(|n| format!("module::case_{n}")).collect();
    let tests: Vec<(&str, bool)> = names.iter().map(|name| (name.as_str(), false)).collect();
    let text = document(&[("big", &tests)]);
    let chars: Vec<char> = text.chars().collect();

    let found = scan(chars.chunks(64 * 1024).map(|piece| piece.iter().collect())).unwrap();

    assert_eq!(found.len(), 20_000);
    assert_eq!(found[19_999].id, "big module::case_19999");
}

#[test]
fn the_binaries_are_kept_with_where_they_are_and_where_they_run() {
    let mut scanner = ListScanner::default();
    scanner.feed(&fixture("list.json"));

    let (_, suites) = scanner.finish_with_binaries().unwrap();

    let fx = suites.iter().find(|suite| suite.binary_id == "fx").unwrap();
    assert!(fx.binary_path.ends_with("/debug/deps/fx-f32bc118f496b3b9"));
    assert!(fx.cwd.ends_with("tests/fixture-project"));
    assert_eq!(fx.package, "fx");
    assert!(suites.iter().any(|suite| suite.binary_id == "fx::it"));
}
