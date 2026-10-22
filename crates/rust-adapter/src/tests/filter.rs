use crate::filter::expression;

fn ids(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

#[test]
fn one_test_is_its_binary_and_its_name() {
    assert_eq!(
        expression(&ids(&["fx tests::parses_empty"])).as_deref(),
        Some("(binary_id(=fx) & test(=tests::parses_empty))")
    );
}

#[test]
fn tests_of_one_binary_are_one_group_and_binaries_are_joined() {
    let expression =
        expression(&ids(&["fx tests::a", "fx::bin/fx tests::a", "fx tests::b"])).unwrap();

    assert_eq!(
        expression,
        "(binary_id(=fx) & (test(=tests::a) | test(=tests::b))) | (binary_id(=fx::bin/fx) & test(=tests::a))"
    );
}

#[test]
fn a_name_with_more_than_plain_characters_is_matched_as_a_whole_by_a_regular_expression() {
    let expression = expression(&ids(&["fx a(1) b+c"])).unwrap();

    assert_eq!(expression, r"(binary_id(=fx) & test(/^a\(1\) b\+c$/))");
}

#[test]
fn a_binary_id_with_a_dash_and_dots_is_plain() {
    let expression = expression(&ids(&["my-crate::bin/foo.rs x"])).unwrap();

    assert_eq!(expression, "(binary_id(=my-crate::bin/foo.rs) & test(=x))");
}

#[test]
fn ids_that_are_not_ids_are_left_out_and_none_at_all_is_nothing() {
    assert_eq!(expression(&ids(&["nospace", ""])), None);
    assert_eq!(expression(&[]), None);
    assert_eq!(
        expression(&ids(&["nospace", "fx t"])).as_deref(),
        Some("(binary_id(=fx) & test(=t))")
    );
}
