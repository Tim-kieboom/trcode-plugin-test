use trcode_plugin_api::{FromPlugin, Plugin, Refusal, ToPlugin, WireView};
use trcode_test_protocol::{
    DiscoverArgs, PAGE, Page, RunArgs, RunEvent, Status, TestItem, from_payload, method, to_payload,
};

use super::{ADAPTER, RowStatus, TestUi};

fn items(range: std::ops::Range<usize>) -> Vec<TestItem> {
    range
        .map(|n| TestItem {
            id: format!("t{n}"),
            name: format!("case_{n}"),
            group: "bin".into(),
        })
        .collect()
}

fn page(offset: usize, count: usize, total: usize) -> String {
    to_payload(&Page {
        offset,
        total,
        tests: items(offset..offset + count),
    })
}

fn calls(said: &[FromPlugin]) -> Vec<(u64, &str, &str)> {
    said.iter()
        .filter_map(|message| match message {
            FromPlugin::Call {
                request,
                to,
                method,
                ..
            } => {
                assert_eq!(to, ADAPTER);
                Some((*request, method.as_str(), ""))
            }
            _ => None,
        })
        .collect()
}

fn reply(request: u64, payload: String, last: bool) -> ToPlugin {
    ToPlugin::Reply {
        from: ADAPTER.into(),
        request,
        payload,
        last,
    }
}

fn lines(said: &[FromPlugin]) -> Vec<String> {
    match said.last() {
        Some(FromPlugin::SetView {
            view: WireView::Rows { lines, .. },
        }) => lines.iter().map(|line| line.text.clone()).collect(),
        other => panic!("not a view of rows: {other:?}"),
    }
}

/// A pane that has been told about `count` tests in one page.
fn with(count: usize) -> TestUi {
    let mut ui = TestUi::default();
    ui.handle(ToPlugin::Start);
    ui.handle(reply(1, page(0, count, count), true));
    ui
}

#[test]
fn it_asks_for_the_first_page_when_it_starts_and_draws() {
    let mut ui = TestUi::default();

    let said = ui.handle(ToPlugin::Start);

    assert_eq!(calls(&said), vec![(1, method::DISCOVER, "")]);
    let FromPlugin::Call { payload, .. } = &said[0] else {
        panic!("{said:?}");
    };
    assert_eq!(from_payload(payload), Some(DiscoverArgs::page_at(0)));
    assert!(lines(&said)[0].starts_with("tests 0/0"));
}

#[test]
fn a_page_is_shown_at_once_and_the_next_is_asked_for_until_all_are_known() {
    let mut ui = TestUi::default();
    ui.handle(ToPlugin::Start);

    let said = ui.handle(reply(1, page(0, PAGE, 2 * PAGE + 1), true));

    assert_eq!(ui.discovered(), (PAGE, 2 * PAGE + 1));
    assert_eq!(calls(&said), vec![(2, method::DISCOVER, "")]);
    assert!(lines(&said)[0].starts_with(&format!("tests {PAGE}/{}", 2 * PAGE + 1)));
    let said = ui.handle(reply(2, page(PAGE, PAGE, 2 * PAGE + 1), true));
    assert_eq!(calls(&said).len(), 1);
    let said = ui.handle(reply(3, page(2 * PAGE, 1, 2 * PAGE + 1), true));
    assert!(calls(&said).is_empty(), "the last page ends it");
    assert_eq!(ui.discovered(), (2 * PAGE + 1, 2 * PAGE + 1));
}

#[test]
fn a_page_from_the_wrong_place_or_that_is_not_one_is_not_added() {
    let mut ui = TestUi::default();
    ui.handle(ToPlugin::Start);
    ui.handle(reply(1, page(5, 3, 10), true));
    assert_eq!(ui.discovered().0, 0);
    assert!(ui.notice().contains("came when 0 were known"));

    let mut ui = TestUi::default();
    ui.handle(ToPlugin::Start);
    ui.handle(reply(1, "nonsense".into(), true));
    assert_eq!(ui.discovered().0, 0);
    assert!(ui.notice().contains("not one"));
}

#[test]
fn an_answer_to_a_call_it_does_not_know_changes_nothing() {
    let mut ui = with(3);

    ui.handle(reply(99, page(0, 3, 3), true));

    assert_eq!(ui.discovered(), (3, 3));
}

#[test]
fn run_all_queues_every_test_and_asks_with_a_number_for_the_run() {
    let mut ui = with(3);

    let said = ui.handle(ToPlugin::action("run_all"));

    assert_eq!(ui.counts().of(RowStatus::Queued), 3);
    let FromPlugin::Call {
        method, payload, ..
    } = &said[0]
    else {
        panic!("{said:?}");
    };
    assert_eq!(method, "run");
    assert_eq!(
        from_payload(payload),
        Some(RunArgs {
            run_id: 1,
            ids: None
        })
    );
    assert_eq!(ui.running(), Some(1));
}

#[test]
fn run_selected_asks_for_the_id_of_the_selected_row() {
    let mut ui = with(3);
    ui.handle(ToPlugin::action("down"));

    let said = ui.handle(ToPlugin::action("run_selected"));

    let FromPlugin::Call { payload, .. } = &said[0] else {
        panic!("{said:?}");
    };
    let args: RunArgs = from_payload(payload).unwrap();
    assert_eq!(args.ids, Some(vec!["t1".to_owned()]));
    assert_eq!(ui.status_of("t1"), Some(RowStatus::Queued));
    assert_eq!(ui.status_of("t0"), Some(RowStatus::Unrun));
}

fn event(run_id: u64, id: &str, status: Status) -> String {
    to_payload(&RunEvent::Finished {
        run_id,
        id: id.into(),
        status,
        duration_ms: 1,
        message: String::new(),
    })
}

#[test]
fn events_of_the_run_set_the_state_of_their_tests_and_done_ends_the_run() {
    let mut ui = with(3);
    ui.handle(ToPlugin::action("run_all"));
    let request = ui.running_call().unwrap();

    let started = to_payload(&RunEvent::Started {
        run_id: 1,
        id: "t0".into(),
    });
    ui.handle(reply(request, started, false));
    assert_eq!(ui.status_of("t0"), Some(RowStatus::Running));
    ui.handle(reply(request, event(1, "t0", Status::Failed), false));
    assert_eq!(ui.status_of("t0"), Some(RowStatus::Failed));
    ui.handle(reply(
        request,
        to_payload(&RunEvent::Done { run_id: 1 }),
        true,
    ));

    assert_eq!(ui.running(), None);
}

#[test]
fn an_event_of_another_run_is_not_this_ones() {
    let mut ui = with(3);
    ui.handle(ToPlugin::action("run_all"));
    let request = ui.running_call().unwrap();

    ui.handle(reply(request, event(7, "t1", Status::Passed), false));

    assert_eq!(ui.status_of("t1"), Some(RowStatus::Queued));
}

#[test]
fn a_run_started_while_one_goes_gives_the_first_up_before_it_asks_for_the_second() {
    let mut ui = with(3);
    ui.handle(ToPlugin::action("run_all"));
    let first = ui.running_call().unwrap();

    let said = ui.handle(ToPlugin::action("run_all"));

    assert!(matches!(&said[0], FromPlugin::CallCancel { request } if *request == first));
    assert!(matches!(&said[1], FromPlugin::Call { .. }));
    assert_eq!(ui.running(), Some(2));
}

#[test]
fn cancel_gives_the_call_up_and_what_was_not_finished_is_not_run() {
    let mut ui = with(3);
    ui.handle(ToPlugin::action("run_all"));
    let request = ui.running_call().unwrap();
    ui.handle(reply(request, event(1, "t0", Status::Passed), false));

    let said = ui.handle(ToPlugin::action("cancel"));

    assert!(matches!(&said[0], FromPlugin::CallCancel { request: r } if *r == request));
    assert_eq!(ui.status_of("t0"), Some(RowStatus::Passed));
    assert_eq!(ui.status_of("t1"), Some(RowStatus::Unrun));
    assert_eq!(ui.running(), None);
    assert!(
        ui.handle(ToPlugin::action("cancel")).len() == 1,
        "nothing more to cancel"
    );
}

#[test]
fn an_adapter_that_is_gone_makes_what_was_going_unknown_and_says_so() {
    let mut ui = with(3);
    ui.handle(ToPlugin::action("run_all"));
    let request = ui.running_call().unwrap();
    ui.handle(reply(request, event(1, "t0", Status::Passed), false));

    let said = ui.handle(ToPlugin::Refused {
        request,
        reason: Refusal::Gone,
    });

    assert_eq!(ui.status_of("t0"), Some(RowStatus::Passed));
    assert_eq!(ui.status_of("t1"), Some(RowStatus::Unknown));
    assert_eq!(ui.counts().of(RowStatus::Queued), 0);
    assert_eq!(ui.running(), None);
    assert!(lines(&said)[0].contains("the adapter went away"));
}

#[test]
fn an_adapter_that_is_not_there_is_said_not_to_be() {
    let mut ui = TestUi::default();
    ui.handle(ToPlugin::Start);

    ui.handle(ToPlugin::Refused {
        request: 1,
        reason: Refusal::NoSuchPlugin,
    });

    assert!(ui.notice().contains("no adapter called fake-adapter"));
}

#[test]
fn the_view_is_a_window_that_follows_the_selection_and_the_size() {
    let mut ui = with(100);
    ui.handle(ToPlugin::Size {
        width: 80,
        height: 6,
    });

    let said = ui.handle(ToPlugin::action("down"));
    assert_eq!(lines(&said).len(), 6, "the header and five tests");
    for _ in 0..9 {
        ui.handle(ToPlugin::action("down"));
    }
    let said = ui.handle(ToPlugin::action("down"));

    let shown = lines(&said);
    assert_eq!(shown.len(), 6);
    assert!(
        shown[5].contains("case_11"),
        "the selected test is the last shown: {shown:?}"
    );
    let Some(FromPlugin::SetView {
        view: WireView::Rows { selected, .. },
    }) = said.last()
    else {
        panic!();
    };
    assert_eq!(*selected, Some(5));
}

#[test]
fn a_click_selects_the_row_and_one_past_the_end_selects_the_last() {
    let mut ui = with(3);

    ui.handle(ToPlugin::Click { row: 2 });
    let said = ui.handle(ToPlugin::action("run_selected"));

    let FromPlugin::Call { payload, .. } = &said[0] else {
        panic!();
    };
    let args: RunArgs = from_payload(payload).unwrap();
    assert_eq!(args.ids, Some(vec!["t1".to_owned()]));
    ui.handle(ToPlugin::Click { row: 50 });
    let said = ui.handle(ToPlugin::action("run_selected"));
    let FromPlugin::Call { payload, .. } = &said[1] else {
        panic!("{said:?}");
    };
    assert_eq!(
        from_payload::<RunArgs>(payload).unwrap().ids,
        Some(vec!["t2".to_owned()])
    );
}

#[test]
fn with_nothing_discovered_there_is_nothing_to_select_or_run() {
    let mut ui = TestUi::default();

    assert!(calls(&ui.handle(ToPlugin::action("run_selected"))).is_empty());
    ui.handle(ToPlugin::action("down"));
    ui.handle(ToPlugin::Click { row: 3 });
    assert_eq!(ui.discovered(), (0, 0));
}

#[test]
fn discover_starts_the_list_again_from_nothing() {
    let mut ui = with(3);
    ui.handle(ToPlugin::action("run_all"));

    let said = ui.handle(ToPlugin::action("discover"));

    assert_eq!(ui.discovered(), (0, 0));
    assert_eq!(ui.counts().of(RowStatus::Queued), 0);
    assert!(
        said.iter().any(
            |message| matches!(message, FromPlugin::Call { method, .. } if method == "discover")
        )
    );
}

#[test]
fn what_it_is_not_told_about_it_ignores() {
    let mut ui = with(2);

    let said = ui.handle(ToPlugin::Refresh);

    assert_eq!(said.len(), 1, "only the view");
    assert!(ui.handle(ToPlugin::action("unheard_of")).len() == 1);
}
