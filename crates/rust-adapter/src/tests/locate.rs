use trcode_test_protocol::Location;

use crate::locate::{Locator, panic_location};

fn at(path: &str, line: u32, column: u32) -> Option<Location> {
    Some(Location {
        path: path.to_owned(),
        line,
        column,
    })
}

#[test]
fn a_panic_names_its_place() {
    let new =
        "thread 'tests::a' (4242) panicked at src/lib.rs:12:5:\nassertion `left == right` failed";
    assert_eq!(panic_location(new), at("src/lib.rs", 12, 5));
    let old = "thread 'tests::a' panicked at 'boom', crates/x/src/lib.rs:3:9\nnote: run with RUST_BACKTRACE";
    assert_eq!(panic_location(old), at("crates/x/src/lib.rs", 3, 9));
    // Windows writes `\`.
    let windows = "thread 'a' panicked at src\\tests\\a.rs:7:1:\nboom";
    assert_eq!(panic_location(windows), at("src/tests/a.rs", 7, 1));
}

#[test]
fn a_panic_outside_the_project_is_no_place_to_go_to() {
    for outside in [
        "thread 'a' panicked at /rustc/abc/library/core/src/option.rs:1:2:\nx",
        "thread 'a' panicked at C:\\Users\\x\\.cargo\\registry\\a.rs:1:2:\nx",
        "thread 'a' panicked at ../other/src/lib.rs:1:2:\nx",
    ] {
        assert_eq!(panic_location(outside), None, "{outside}");
    }
}

#[test]
fn no_panic_line_no_place() {
    assert_eq!(panic_location("the test printed this and failed"), None);
    assert_eq!(
        panic_location("thread 'a' panicked at src/lib.rs:oops"),
        None
    );
    assert_eq!(panic_location(""), None);
}

fn found(id: &str, output: &str) -> Option<Location> {
    let mut locator = Locator::new(id);
    // As it comes: cut in the middle of a line.
    let (first, second) = output.split_at(output.len() / 2);
    locator.feed(first);
    locator.feed(second);
    locator.finish()
}

#[test]
fn the_only_match_is_the_place() {
    let output = "./src/a.rs:12:    fn parses() {\n";

    assert_eq!(found("fx tests::parses", output), at("src/a.rs", 12, 5));
}

#[test]
fn of_several_matches_the_one_in_the_modules_of_the_test_wins() {
    let output = "./src/other.rs:3:fn works() {}\n\
                  ./crates/pty/src/tests/flood_tests.rs:40:fn works() {\n\
                  ./src/tests/mod.rs:9:fn works() {}\n";

    assert_eq!(
        found("trcode-pty tests::flood_tests::works", output),
        at("crates/pty/src/tests/flood_tests.rs", 40, 1)
    );
}

#[test]
fn a_file_named_mod_counts_as_its_folder() {
    let output = "./src/a.rs:1:fn t() {}\n./src/tests/mod.rs:2:fn t() {}\n";

    assert_eq!(found("fx tests::t", output), at("src/tests/mod.rs", 2, 1));
}

#[test]
fn with_nothing_to_choose_by_the_first_match_stays() {
    let output = "./src/a.rs:1:fn t() {}\n./src/b.rs:2:fn t() {}\n";

    assert_eq!(found("fx t", output), at("src/a.rs", 1, 1));
}

#[test]
fn lines_that_are_not_matches_are_left_alone() {
    let output = "grep: ./x: Permission denied\nBinary file ./y matches\n./src/a.rs:5:fn t() {}\n";

    assert_eq!(found("fx t", output), at("src/a.rs", 5, 1));
    assert_eq!(found("fx t", ""), None);
}

#[test]
fn a_test_of_the_library_is_not_looked_for_among_the_integration_tests_and_the_reverse() {
    let output = "./src/lib.rs:8:    fn parses_empty() {\n./tests/it.rs:4:fn parses_empty() {\n";

    // The library's `tests` module is inside `src/lib.rs`, whatever a `tests/` folder holds.
    assert_eq!(
        found("fx tests::parses_empty", output),
        at("src/lib.rs", 8, 5)
    );
    // The binary `fx::it` is the file `tests/it.rs`.
    assert_eq!(
        found("fx::it parses_empty", output),
        at("tests/it.rs", 4, 1)
    );
    // A binary of the project and an example are not integration tests.
    assert_eq!(
        found("fx::bin/fx tests::parses_empty", output),
        at("src/lib.rs", 8, 5)
    );
}

#[test]
fn the_unit_tests_in_a_src_tests_folder_are_not_integration_tests() {
    let output = "./crates/a/tests/it.rs:1:fn t() {}\n./crates/a/src/tests/m.rs:2:fn t() {}\n";

    assert_eq!(
        found("a tests::m::t", output),
        at("crates/a/src/tests/m.rs", 2, 1)
    );
    assert_eq!(found("a::it t", output), at("crates/a/tests/it.rs", 1, 1));
}
