use super::*;

#[test]
fn a_run_event_is_written_with_its_name_and_reads_back() {
    let event = RunEvent::Finished {
        run_id: 7,
        id: "t3".into(),
        status: Status::Failed,
        duration_ms: 12,
        message: "boom".into(),
    };

    let text = to_payload(&event);

    assert_eq!(
        text,
        r#"{"event":"finished","run_id":7,"id":"t3","status":"failed","duration_ms":12,"message":"boom"}"#
    );
    assert_eq!(from_payload::<RunEvent>(&text), Some(event));
}

#[test]
fn an_event_or_status_a_later_adapter_has_reads_as_unknown_not_as_an_error() {
    assert_eq!(
        from_payload::<RunEvent>(r#"{"event":"retrying","run_id":1}"#),
        Some(RunEvent::Unknown)
    );
    assert_eq!(
        from_payload::<RunEvent>(r#"{"event":"finished","run_id":1,"id":"a","status":"flaky"}"#),
        Some(RunEvent::Finished {
            run_id: 1,
            id: "a".into(),
            status: Status::Unknown,
            duration_ms: 0,
            message: String::new(),
        })
    );
    assert_eq!(RunEvent::Unknown.run_id(), None);
}

#[test]
fn a_discover_request_without_a_size_asks_for_a_whole_page() {
    let args: DiscoverArgs = from_payload(r#"{"offset":500}"#).unwrap();

    assert_eq!(args, DiscoverArgs::page_at(500));
}

#[test]
fn a_run_without_ids_runs_everything_and_nothing_else_is_assumed() {
    let args: RunArgs = from_payload(r#"{"run_id":2}"#).unwrap();

    assert_eq!(args.ids, None);
    assert!(
        from_payload::<RunArgs>(r#"{"ids":["a"]}"#).is_none(),
        "no run id"
    );
}

#[test]
fn a_scenario_says_only_what_differs_from_the_plain_adapter() {
    let scenario: Scenario = from_payload(r#"{"tests":20000,"same_names":true}"#).unwrap();

    assert_eq!(scenario.tests, 20000);
    assert!(scenario.same_names);
    assert!(!scenario.traps_on_advance && !scenario.talks_after_cancel);
    assert_eq!(from_payload::<Scenario>("{}"), Some(Scenario::default()));
}

#[test]
fn what_is_not_the_payload_asked_for_is_none() {
    assert_eq!(from_payload::<Page>("nonsense"), None);
}

#[test]
fn a_page_that_failed_says_why_and_a_good_page_does_not_mention_an_error() {
    let failed = Page::failed("it does not build");
    let text = to_payload(&failed);

    assert_eq!(
        text,
        r#"{"offset":0,"total":0,"tests":[],"error":"it does not build"}"#
    );
    assert_eq!(from_payload::<Page>(&text), Some(failed));
    let good = Page {
        offset: 0,
        total: 0,
        tests: Vec::new(),
        error: None,
    };
    assert_eq!(to_payload(&good), r#"{"offset":0,"total":0,"tests":[]}"#);
    assert_eq!(
        from_payload::<Page>(r#"{"offset":0,"total":0,"tests":[]}"#),
        Some(good)
    );
}

#[test]
fn an_error_event_belongs_to_its_run_like_the_others() {
    let event = RunEvent::Error {
        run_id: 4,
        message: "no build".into(),
    };

    assert_eq!(event.run_id(), Some(4));
    assert_eq!(
        to_payload(&event),
        r#"{"event":"error","run_id":4,"message":"no build"}"#
    );
}

// --- the table of tests ---

fn item(id: &str, name: &str, group: &str) -> TestItem {
    TestItem {
        id: id.into(),
        name: name.into(),
        group: group.into(),
    }
}

#[test]
fn a_table_gives_back_what_was_put_in_it_in_order() {
    let mut table = Table::default();

    assert_eq!(table.push(&item("a 1", "t1", "a")), 0);
    assert_eq!(table.push(&item("b 2", "t2", "b")), 1);
    assert_eq!(table.push(&item("a 3", "t3", "a")), 2);

    assert_eq!(table.len(), 3);
    assert_eq!(table.item(1), item("b 2", "t2", "b"));
    assert_eq!(
        (table.id(2), table.name(2), table.group(2)),
        ("a 3", "t3", "a")
    );
}

#[test]
fn an_id_is_found_and_one_that_is_not_there_is_not() {
    let mut table = Table::default();
    for n in 0..1_000 {
        table.push_parts(&format!("g t{n}"), &format!("t{n}"), "g");
    }

    assert_eq!(table.find("g t0"), Some(0));
    assert_eq!(table.find("g t999"), Some(999));
    assert_eq!(table.find("g t1000"), None);
    assert_eq!(table.find(""), None);
}

#[test]
fn two_tests_with_one_name_in_two_groups_are_two_tests_with_ids_of_their_own() {
    let mut table = Table::default();
    table.push_parts("x t", "t", "x");
    table.push_parts("y t", "t", "y");

    assert_eq!(table.find("x t"), Some(0));
    assert_eq!(table.find("y t"), Some(1));
    assert_eq!((table.group(0), table.group(1)), ("x", "y"));
}

#[test]
fn ids_that_hash_alike_are_told_apart_and_so_is_the_same_id_added_twice() {
    // The same id twice hashes the same: the second is kept at the next place, and the first
    // is the one that is found.
    let mut table = Table::default();
    table.push_parts("same", "a", "g");
    table.push_parts("same", "b", "g");
    table.push_parts("other", "c", "g");

    assert_eq!(table.find("same"), Some(0));
    assert_eq!(table.find("other"), Some(2));
    assert_eq!(table.len(), 3);
}

#[test]
fn a_group_is_kept_once_however_many_tests_are_in_it() {
    let mut table = Table::default();
    for n in 0..100 {
        table.push_parts(&format!("g{} {n}", n % 2), "t", &format!("g{}", n % 2));
    }

    assert_eq!(table.group(98), "g0");
    assert_eq!(table.group(99), "g1");
    assert_eq!(table.group_count(), 2);
}

#[test]
fn items_are_a_page_that_stops_at_the_end_and_is_empty_past_it() {
    let mut table = Table::default();
    for n in 0..5 {
        table.push_parts(&format!("id{n}"), &format!("n{n}"), "g");
    }

    let ids = |items: Vec<TestItem>| items.into_iter().map(|i| i.id).collect::<Vec<_>>();
    assert_eq!(ids(table.items(0, 2)), ["id0", "id1"]);
    assert_eq!(ids(table.items(3, 10)), ["id3", "id4"]);
    assert!(table.items(9, 10).is_empty());
    assert!(table.items(1, 0).is_empty());
}

#[test]
fn clearing_forgets_everything() {
    let mut table = Table::default();
    table.push_parts("a", "a", "g");

    table.clear();

    assert!(table.is_empty());
    assert_eq!(table.find("a"), None);
    assert_eq!(table.push_parts("b", "b", "h"), 0);
    assert_eq!(table.group(0), "h");
}

#[test]
fn text_with_non_ascii_characters_is_kept_whole() {
    let mut table = Table::default();
    table.push_parts("é ü", "😀", "gé");

    assert_eq!(table.item(0), item("é ü", "😀", "gé"));
    assert_eq!(table.find("é ü"), Some(0));
}
