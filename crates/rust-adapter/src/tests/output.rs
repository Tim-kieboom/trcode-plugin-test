use trcode_test_protocol::Status;

use super::fixture;
use crate::output::{Outcome, RunParser};

fn parse(pieces: impl IntoIterator<Item = String>) -> Vec<Outcome> {
    let mut parser = RunParser::default();
    let mut ended = Vec::new();
    for piece in pieces {
        ended.extend(parser.feed(&piece));
    }
    ended.extend(parser.finish());
    ended
}

fn find<'a>(ended: &'a [Outcome], binary: &str, name: &str) -> &'a Outcome {
    ended
        .iter()
        .find(|outcome| outcome.binary_id == binary && outcome.name == name)
        .unwrap_or_else(|| panic!("no result for {binary} {name}: {ended:?}"))
}

#[test]
fn the_real_run_gives_each_test_once_with_how_it_ended() {
    let ended = parse([fixture("run.txt")]);

    assert_eq!(ended.len(), 5, "{ended:?}");
    for (binary, name) in [
        ("fx::bin/fx", "tests::parses_empty"),
        ("fx::it", "parses_empty"),
        ("fx", "tests::parses_empty"),
        ("fx", "tests::panics_ok"),
    ] {
        let outcome = find(&ended, binary, name);
        assert_eq!(outcome.status, Status::Passed, "{binary} {name}");
        assert_eq!(outcome.message, "");
    }
    assert_eq!(find(&ended, "fx", "tests::parses_empty").duration_ms, 3);
}

#[test]
fn a_failed_test_has_what_it_wrote_from_where_it_panicked_and_is_not_told_twice() {
    let ended = parse([fixture("run.txt")]);

    let failed: Vec<&Outcome> = ended
        .iter()
        .filter(|outcome| outcome.status == Status::Failed)
        .collect();
    assert_eq!(
        failed.len(),
        1,
        "the list of failures after the summary is not new"
    );
    let message = &failed[0].message;
    assert!(
        message.starts_with("thread 'tests::fails_here'"),
        "{message}"
    );
    assert!(message.contains("panicked at src/lib.rs:14:9"), "{message}");
    assert!(message.contains("left: 2"), "{message}");
    assert!(!message.contains("stderr ───"), "{message}");
    assert!(!message.starts_with(' '), "no indent");
}

#[test]
fn the_results_are_the_same_wherever_the_output_is_cut() {
    let text = fixture("run.txt");
    let whole = parse([text.clone()]);

    for cut in (0..text.len()).filter(|at| text.is_char_boundary(*at)) {
        let pieces = [text[..cut].to_owned(), text[cut..].to_owned()];
        assert_eq!(parse(pieces), whole, "cut at {cut}");
    }
}

#[test]
fn a_test_that_passed_is_told_when_its_line_has_its_end_not_before() {
    let mut parser = RunParser::default();

    assert!(
        parser
            .feed("        PASS [   0.003s] (1/2) fx a")
            .is_empty()
    );
    let ended = parser.feed("\n");

    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].name, "a");
}

#[test]
fn a_failed_test_is_told_when_the_next_result_or_a_line_of_dashes_ends_its_output() {
    let mut parser = RunParser::default();
    assert!(
        parser
            .feed("        FAIL [   0.004s] (1/3) fx t1\n  stderr ───\n\n    boom\n")
            .is_empty()
    );

    let ended = parser.feed("        PASS [   0.004s] (2/3) fx t2\n");

    assert_eq!(ended.len(), 2);
    assert_eq!(ended[0].name, "t1");
    assert_eq!(ended[0].message, "boom");
    assert_eq!(ended[1].name, "t2");
}

#[test]
fn a_last_line_without_an_end_and_a_failure_still_open_are_told_when_the_output_ends() {
    let mut parser = RunParser::default();
    parser.feed("        FAIL [   0.004s] (1/2) fx t1\n    only stdout\n");
    parser.feed("        PASS [   0.004s] (2/2) fx t2");

    let ended = parser.finish();

    assert_eq!(ended.len(), 2);
    assert_eq!(ended[0].message, "only stdout");
    assert_eq!(ended[1].status, Status::Passed);
}

#[test]
fn what_a_failed_test_wrote_to_stdout_is_the_message_when_it_wrote_nothing_to_stderr() {
    let text = "        FAIL [   0.004s] (1/1) fx t1\n  stdout ───\n\n    running 1 test\n    bad thing\n\n  stderr ───\n\n────────────\n";

    let ended = parse([text.to_owned()]);

    assert_eq!(ended[0].message, "running 1 test\nbad thing");
}

#[test]
fn skipped_timed_out_and_killed_tests_have_their_states_and_the_rest_is_ignored() {
    let text = "\n        SKIP [         ] (───) fx ignored_one
     TIMEOUT [  60.000s] ( 2/9) fx slow
     SIGSEGV [   0.001s] ( 3/9) fx crashes
        SLOW [> 30.000s] ( 4/9) fx still_going
   LEAK-FAIL [   0.001s] ( 5/9) fx leaks
      Starting 5 tests across 1 binary
";

    let ended = parse([text.to_owned()]);

    let states: Vec<(&str, Status)> = ended
        .iter()
        .map(|outcome| (outcome.name.as_str(), outcome.status))
        .collect();
    assert_eq!(
        states,
        [
            ("ignored_one", Status::Skipped),
            ("slow", Status::Failed),
            ("crashes", Status::Failed),
            ("leaks", Status::Failed),
        ]
    );
    assert_eq!(ended[1].duration_ms, 60_000);
    assert_eq!(ended[0].duration_ms, 0);
}

#[test]
fn a_line_a_test_wrote_that_looks_like_a_result_is_not_one() {
    let text = "        FAIL [   0.004s] (1/1) fx t1\n  stdout ───\n\n    PASS [   0.001s] (1/1) fx pretend\n";

    let ended = parse([text.to_owned()]);

    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].name, "t1");
    assert!(ended[0].message.contains("PASS [   0.001s]"));
}

#[test]
fn a_long_message_is_cut_and_says_so() {
    let long = "x".repeat(10_000);
    let text = format!("        FAIL [   0.004s] (1/1) fx t1\n  stderr ───\n\n    {long}\n");

    let ended = parse([text]);

    let message = &ended[0].message;
    assert!(message.ends_with('…'));
    assert_eq!(message.chars().count(), 4_001);
}

#[test]
fn text_that_is_not_the_output_gives_nothing_and_carriage_returns_are_ignored() {
    assert!(parse(["hello\nworld\n".to_owned()]).is_empty());
    let ended = parse(["        PASS [   0.003s] (1/1) fx a\r\n".to_owned()]);
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].name, "a");
}

#[test]
fn compile_errors_are_not_results() {
    let text =
        "error[E0425]: cannot find value `x`\n --> src/lib.rs:1:1\nerror: could not compile `fx`\n";

    assert!(parse([text.to_owned()]).is_empty());
}
