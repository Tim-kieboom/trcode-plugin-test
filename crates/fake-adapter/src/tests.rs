use trcode_plugin_api::{FromPlugin, Plugin, ToPlugin};
use trcode_test_protocol::{
    AdvanceArgs, DiscoverArgs, PAGE, Page, RunArgs, RunEvent, Scenario, Status, from_payload,
    method, to_payload,
};

use super::FakeAdapter;

fn call(adapter: &mut FakeAdapter, request: u64, name: &str, payload: String) -> Vec<FromPlugin> {
    adapter.handle(ToPlugin::Call {
        from: "pane".into(),
        request,
        method: name.into(),
        payload,
    })
}

fn configure(adapter: &mut FakeAdapter, scenario: &Scenario) {
    call(adapter, 0, method::CONFIGURE, to_payload(scenario));
}

/// The payloads of the replies and whether each ends its call.
fn said(replies: &[FromPlugin]) -> Vec<(u64, String, bool)> {
    replies
        .iter()
        .map(|reply| match reply {
            FromPlugin::Reply {
                request,
                payload,
                last,
                ..
            } => (*request, payload.clone(), *last),
            other => panic!("not a reply: {other:?}"),
        })
        .collect()
}

fn page(adapter: &mut FakeAdapter, offset: usize) -> Page {
    let replies = call(
        adapter,
        1,
        method::DISCOVER,
        to_payload(&DiscoverArgs::page_at(offset)),
    );
    let said = said(&replies);
    assert_eq!(said.len(), 1);
    assert!(said[0].2, "a page ends its call");
    from_payload(&said[0].1).unwrap()
}

fn events(replies: &[FromPlugin]) -> Vec<RunEvent> {
    said(replies)
        .into_iter()
        .filter_map(|(_, payload, _)| from_payload::<RunEvent>(&payload))
        .collect()
}

fn ask_run(adapter: &mut FakeAdapter, request: u64, run_id: u64, ids: Option<Vec<&str>>) {
    let args = RunArgs {
        run_id,
        ids: ids.map(|ids| ids.into_iter().map(str::to_owned).collect()),
    };
    assert!(call(adapter, request, method::RUN, to_payload(&args)).is_empty());
}

fn advance(adapter: &mut FakeAdapter, n: usize) -> Vec<FromPlugin> {
    call(adapter, 99, method::ADVANCE, to_payload(&AdvanceArgs { n }))
}

#[test]
fn the_plain_adapter_has_six_tests_with_ids_of_their_own() {
    let mut adapter = FakeAdapter::default();

    let page = page(&mut adapter, 0);

    assert_eq!(page.total, 6);
    let ids: Vec<&str> = page.tests.iter().map(|test| test.id.as_str()).collect();
    assert_eq!(ids, ["t0", "t1", "t2", "t3", "t4", "t5"]);
}

#[test]
fn tests_can_share_a_name_and_not_an_id() {
    let mut adapter = FakeAdapter::default();
    configure(
        &mut adapter,
        &Scenario {
            tests: 4,
            same_names: true,
            ..Scenario::default()
        },
    );

    let tests = page(&mut adapter, 0).tests;

    assert_eq!(tests[0].name, tests[1].name);
    assert_ne!(tests[0].id, tests[1].id);
    assert_ne!(tests[0].group, tests[1].group);
}

#[test]
fn a_page_is_at_most_a_page_and_the_pages_add_up() {
    let mut adapter = FakeAdapter::default();
    configure(
        &mut adapter,
        &Scenario {
            tests: 5 * PAGE / 2,
            ..Scenario::default()
        },
    );

    let sizes: Vec<usize> = [0, PAGE, 2 * PAGE]
        .iter()
        .map(|offset| page(&mut adapter, *offset).tests.len())
        .collect();

    assert_eq!(sizes, vec![PAGE, PAGE, PAGE / 2]);
    assert!(page(&mut adapter, 5_000).tests.is_empty(), "past the end");
}

#[test]
fn a_run_says_nothing_until_the_clock_is_turned_and_then_each_test_starts_and_finishes() {
    let mut adapter = FakeAdapter::default();
    ask_run(&mut adapter, 7, 1, Some(vec!["t0", "t1"]));

    let first = advance(&mut adapter, 1);
    let events_first = events(&first[..first.len() - 1]);

    assert_eq!(events_first.len(), 2);
    assert!(matches!(&events_first[0], RunEvent::Started { run_id: 1, id } if id == "t0"));
    assert!(
        matches!(&events_first[1], RunEvent::Finished { run_id: 1, id, status: Status::Passed, .. } if id == "t0")
    );
    // The advance call itself is answered, last.
    assert_eq!(said(&first).last().unwrap(), &(99, "{}".to_owned(), true));
}

#[test]
fn a_run_ends_with_done_as_the_last_reply_and_is_forgotten() {
    let mut adapter = FakeAdapter::default();
    ask_run(&mut adapter, 7, 1, Some(vec!["t0"]));

    let replies = said(&advance(&mut adapter, 5));
    let to_the_run: Vec<_> = replies
        .iter()
        .filter(|(request, ..)| *request == 7)
        .collect();

    let (_, payload, last) = to_the_run.last().unwrap();
    assert_eq!(
        from_payload::<RunEvent>(payload),
        Some(RunEvent::Done { run_id: 1 })
    );
    assert!(last);
    assert!(
        to_the_run[..to_the_run.len() - 1]
            .iter()
            .all(|(.., last)| !last)
    );
    assert!(
        said(&advance(&mut adapter, 5))
            .iter()
            .all(|(request, ..)| *request == 99)
    );
}

#[test]
fn a_run_of_some_ids_runs_those_and_ignores_ids_it_never_made() {
    let mut adapter = FakeAdapter::default();
    ask_run(&mut adapter, 7, 1, Some(vec!["t4", "nonsense", "t99"]));

    let finished: Vec<String> = events(&advance(&mut adapter, 10))
        .into_iter()
        .filter_map(|event| match event {
            RunEvent::Finished { id, .. } => Some(id),
            _ => None,
        })
        .collect();

    assert_eq!(finished, vec!["t4".to_owned()]);
}

#[test]
fn some_tests_fail_with_a_message_and_always_the_same_ones() {
    let mut adapter = FakeAdapter::default();
    ask_run(&mut adapter, 7, 1, None);

    let statuses: Vec<Status> = events(&advance(&mut adapter, 10))
        .into_iter()
        .filter_map(|event| match event {
            RunEvent::Finished {
                status, message, ..
            } => {
                assert_eq!(message.is_empty(), status != Status::Failed);
                Some(status)
            }
            _ => None,
        })
        .collect();

    assert_eq!(statuses[3], Status::Failed);
    assert_eq!(statuses[0], Status::Passed);
}

#[test]
fn a_cancelled_run_is_stopped_and_one_that_does_not_listen_goes_on() {
    let mut listening = FakeAdapter::default();
    ask_run(&mut listening, 7, 1, None);
    listening.handle(ToPlugin::CallCancelled {
        from: "pane".into(),
        request: 7,
    });
    assert_eq!(
        said(&advance(&mut listening, 3)).len(),
        1,
        "only the answer to advance"
    );

    let mut deaf = FakeAdapter::default();
    configure(
        &mut deaf,
        &Scenario {
            talks_after_cancel: true,
            ..Scenario::default()
        },
    );
    ask_run(&mut deaf, 7, 1, None);
    deaf.handle(ToPlugin::CallCancelled {
        from: "pane".into(),
        request: 7,
    });
    assert!(
        said(&advance(&mut deaf, 1)).len() > 1,
        "it still tells the run"
    );
}

#[test]
fn a_cancel_of_another_call_stops_nothing() {
    let mut adapter = FakeAdapter::default();
    ask_run(&mut adapter, 7, 1, None);

    adapter.handle(ToPlugin::CallCancelled {
        from: "pane".into(),
        request: 8,
    });
    adapter.handle(ToPlugin::CallCancelled {
        from: "other".into(),
        request: 7,
    });

    assert!(said(&advance(&mut adapter, 1)).len() > 1);
}

#[test]
fn two_runs_are_both_advanced_and_each_tells_its_own_call() {
    let mut adapter = FakeAdapter::default();
    ask_run(&mut adapter, 7, 1, Some(vec!["t0"]));
    ask_run(&mut adapter, 8, 2, Some(vec!["t1"]));

    let replies = said(&advance(&mut adapter, 1));

    assert!(replies.iter().any(|(request, ..)| *request == 7));
    assert!(replies.iter().any(|(request, ..)| *request == 8));
}

#[test]
#[should_panic(expected = "told to die")]
fn an_adapter_told_to_die_dies_when_a_run_is_going() {
    let mut adapter = FakeAdapter::default();
    configure(
        &mut adapter,
        &Scenario {
            traps_on_advance: true,
            ..Scenario::default()
        },
    );
    ask_run(&mut adapter, 7, 1, None);

    advance(&mut adapter, 1);
}

#[test]
fn an_adapter_told_to_die_is_fine_with_no_run_going() {
    let mut adapter = FakeAdapter::default();
    configure(
        &mut adapter,
        &Scenario {
            traps_on_advance: true,
            ..Scenario::default()
        },
    );

    assert_eq!(said(&advance(&mut adapter, 1)).len(), 1);
}

#[test]
fn a_method_it_does_not_know_is_answered_so_the_call_is_not_left_open() {
    let mut adapter = FakeAdapter::default();

    let replies = said(&call(&mut adapter, 3, "unheard_of", String::new()));

    assert_eq!(replies, vec![(3, String::new(), true)]);
}

#[test]
fn what_it_is_not_told_it_ignores() {
    let mut adapter = FakeAdapter::default();

    assert!(adapter.handle(ToPlugin::Start).is_empty());
    assert!(adapter.handle(ToPlugin::Refresh).is_empty());
}
