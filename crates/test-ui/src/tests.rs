use trcode_plugin_api::{FromPlugin, Plugin, Refusal, ToPlugin, WireView};
use trcode_test_protocol::{
    DiscoverArgs, PAGE, Page, RunArgs, RunEvent, Status, TestItem, from_payload, method, to_payload,
};

use super::{FAKE_ADAPTER, RUST_ADAPTER, RowStatus, TestUi};

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
        error: None,
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
                assert_eq!(to, RUST_ADAPTER);
                Some((*request, method.as_str(), ""))
            }
            _ => None,
        })
        .collect()
}

fn reply(request: u64, payload: String, last: bool) -> ToPlugin {
    ToPlugin::Reply {
        from: RUST_ADAPTER.into(),
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
    assert!(lines(&said)[0].starts_with("rust-adapter | tests 0/0"));
}

#[test]
fn a_page_is_shown_at_once_and_the_next_is_asked_for_until_all_are_known() {
    let mut ui = TestUi::default();
    ui.handle(ToPlugin::Start);

    let said = ui.handle(reply(1, page(0, PAGE, 2 * PAGE + 1), true));

    assert_eq!(ui.discovered(), (PAGE, 2 * PAGE + 1));
    assert_eq!(calls(&said), vec![(2, method::DISCOVER, "")]);
    assert!(lines(&said)[0].starts_with(&format!("rust-adapter | tests {PAGE}/{}", 2 * PAGE + 1)));
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
    // The first line is the folder; the next two are the first two tests.
    ui.handle(ToPlugin::action("down"));
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
fn adapters_that_are_not_there_are_said_not_to_be() {
    let mut ui = TestUi::default();
    ui.handle(ToPlugin::Start);
    for request in [1, 2] {
        ui.handle(ToPlugin::Refused {
            request,
            reason: Refusal::NoSuchPlugin,
        });
    }

    assert!(
        ui.notice().contains("no test adapter is installed"),
        "{}",
        ui.notice()
    );
}

#[test]
fn the_view_is_a_window_that_follows_the_selection_and_the_size() {
    let mut ui = with(100);
    ui.handle(ToPlugin::Size {
        width: 80,
        height: 6,
    });

    let said = ui.handle(ToPlugin::action("down"));
    assert_eq!(lines(&said).len(), 6, "the header and five nodes");
    for _ in 0..9 {
        ui.handle(ToPlugin::action("down"));
    }
    let said = ui.handle(ToPlugin::action("down"));

    // The folder is the first node, so eleven steps down is the eleventh test.
    let shown = lines(&said);
    assert_eq!(shown.len(), 6);
    assert!(
        shown[5].contains("case_10"),
        "the selected test is the last shown: {shown:?}"
    );
    let Some(FromPlugin::SetView {
        view: WireView::Rows { selected, .. },
    }) = said.last()
    else {
        panic!();
    };
    assert_eq!(*selected, Some(5));
    // And back up: the window stays until the selection reaches its top.
    for _ in 0..4 {
        ui.handle(ToPlugin::action("up"));
    }
    let said = ui.handle(ToPlugin::action("up"));
    assert!(lines(&said)[1].contains("case_5"), "{:?}", lines(&said));
}

#[test]
fn a_click_selects_the_line_and_one_past_the_end_selects_nothing_new() {
    let mut ui = with(3);

    // The header is the first line, the folder the second, then the tests.
    ui.handle(ToPlugin::Click { row: 3 });
    let said = ui.handle(ToPlugin::action("run_selected"));
    let FromPlugin::Call { payload, .. } = &said[0] else {
        panic!();
    };
    let args: RunArgs = from_payload(payload).unwrap();
    assert_eq!(args.ids, Some(vec!["t1".to_owned()]));
    ui.handle(ToPlugin::Click { row: 50 });
    ui.handle(ToPlugin::Click { row: 0 });
    let said = ui.handle(ToPlugin::action("run_selected"));
    let FromPlugin::Call { payload, .. } = said
        .iter()
        .find(|m| matches!(m, FromPlugin::Call { .. }))
        .unwrap()
    else {
        panic!("{said:?}");
    };
    assert_eq!(
        from_payload::<RunArgs>(payload).unwrap().ids,
        Some(vec!["t1".to_owned()]),
        "the header and a line past the end select nothing"
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

// --- choosing the adapter ---

fn call_to(said: &[FromPlugin]) -> Option<(&str, &str)> {
    said.iter().find_map(|message| match message {
        FromPlugin::Call { to, method, .. } => Some((to.as_str(), method.as_str())),
        _ => None,
    })
}

fn not_there(request: u64) -> ToPlugin {
    ToPlugin::Refused {
        request,
        reason: Refusal::NoSuchPlugin,
    }
}

#[test]
fn the_pane_starts_with_the_rust_adapter() {
    let mut ui = TestUi::default();

    let said = ui.handle(ToPlugin::Start);

    assert_eq!(call_to(&said), Some((RUST_ADAPTER, "discover")));
    assert_eq!(ui.adapter(), RUST_ADAPTER);
}

#[test]
fn an_adapter_that_is_not_installed_is_passed_over_for_the_next_one() {
    let mut ui = TestUi::default();
    ui.handle(ToPlugin::Start);

    let said = ui.handle(not_there(1));

    assert_eq!(call_to(&said), Some((FAKE_ADAPTER, "discover")));
    assert_eq!(ui.adapter(), FAKE_ADAPTER);
    assert_eq!(ui.notice(), "", "it found one: nothing to say");
    // The fake adapter answers.
    ui.handle(reply(2, page(0, 3, 3), true));
    assert_eq!(ui.discovered(), (3, 3));
}

#[test]
fn when_no_adapter_is_installed_it_says_so_and_does_not_ask_for_ever() {
    let mut ui = TestUi::default();
    ui.handle(ToPlugin::Start);
    ui.handle(not_there(1));

    let said = ui.handle(not_there(2));

    assert_eq!(call_to(&said), None, "both were tried");
    assert_eq!(ui.notice(), "no test adapter is installed");
}

#[test]
fn the_choice_of_the_user_is_never_passed_over() {
    let mut ui = TestUi::default();
    ui.handle(ToPlugin::Start);
    let said = ui.handle(ToPlugin::action("use_fake"));
    assert_eq!(call_to(&said), Some((FAKE_ADAPTER, "discover")));
    let request = 2;

    let said = ui.handle(not_there(request));

    assert_eq!(call_to(&said), None);
    assert_eq!(ui.adapter(), FAKE_ADAPTER);
    assert!(
        ui.notice()
            .contains("there is no adapter called fake-adapter"),
        "{}",
        ui.notice()
    );
}

#[test]
fn the_actions_choose_the_adapter_and_ask_for_its_tests_and_the_old_one_is_given_up() {
    let mut ui = with(3);
    ui.handle(ToPlugin::action("run_all"));
    let run = ui.running_call().unwrap();

    let said = ui.handle(ToPlugin::action("use_fake"));

    assert!(matches!(&said[0], FromPlugin::CallCancel { request } if *request == run));
    assert_eq!(call_to(&said), Some((FAKE_ADAPTER, "discover")));
    assert_eq!(ui.running(), None);
    assert_eq!(ui.discovered(), (0, 0));
    let said = ui.handle(ToPlugin::action("use_rust"));
    assert_eq!(call_to(&said), Some((RUST_ADAPTER, "discover")));
    let said = ui.handle(ToPlugin::action("adapter"));
    assert_eq!(
        call_to(&said),
        Some((FAKE_ADAPTER, "discover")),
        "it goes round"
    );
}

#[test]
fn asking_again_starts_the_passing_over_again_from_the_adapter_in_use() {
    let mut ui = TestUi::default();
    ui.handle(ToPlugin::Start);
    ui.handle(not_there(1));
    ui.handle(not_there(2));
    assert_eq!(ui.notice(), "no test adapter is installed");

    let said = ui.handle(ToPlugin::action("discover"));

    assert!(call_to(&said).is_some(), "it asks again");
}

// --- the tree ---

fn named(tests: &[(&str, &str)]) -> Vec<TestItem> {
    tests
        .iter()
        .enumerate()
        .map(|(n, (group, name))| TestItem {
            id: format!("t{n}"),
            name: (*name).to_owned(),
            group: (*group).to_owned(),
        })
        .collect()
}

/// A pane that was told about these tests, which it shows from the top.
fn with_tests(tests: &[(&str, &str)]) -> (TestUi, Vec<String>) {
    let mut ui = TestUi::default();
    ui.handle(ToPlugin::Start);
    let page = to_payload(&Page {
        offset: 0,
        total: tests.len(),
        tests: named(tests),
        error: None,
    });
    let said = ui.handle(reply(1, page, true));
    let shown = lines(&said);
    (ui, shown)
}

fn press(ui: &mut TestUi, action: &str) -> Vec<String> {
    lines(&ui.handle(ToPlugin::action(action)))
}

const PROJECT: [(&str, &str); 5] = [
    ("fx", "tests::fails_here"),
    ("fx", "tests::parses_empty"),
    ("fx", "deep::er::leaf"),
    ("fx::it", "parses_empty"),
    ("fx::bin/fx", "tests::parses_empty"),
];

#[test]
fn the_tests_are_folders_for_the_binary_and_each_module_with_the_test_inside() {
    let (_, shown) = with_tests(&PROJECT);

    assert_eq!(
        &shown[1..],
        [
            "v [  ] fx (0/3)",
            "  v [  ] tests (0/2)",
            "    [  ] fails_here",
            "    [  ] parses_empty",
            "  v [  ] deep (0/1)",
            "    v [  ] er (0/1)",
            "      [  ] leaf",
            "v [  ] fx::it (0/1)",
            "  [  ] parses_empty",
            "v [  ] fx::bin/fx (0/1)",
            "  v [  ] tests (0/1)",
            "    [  ] parses_empty",
        ]
    );
}

#[test]
fn a_name_without_modules_is_a_test_directly_in_the_folder_of_its_group() {
    let (_, shown) = with_tests(&[("bin", "case_1"), ("bin", "case_2")]);

    assert_eq!(
        &shown[1..],
        ["v [  ] bin (0/2)", "  [  ] case_1", "  [  ] case_2"]
    );
}

#[test]
fn a_small_project_is_open_and_a_big_one_is_closed() {
    let small: Vec<(&str, &str)> = vec![("g", "a::t"); 200];
    let big: Vec<(&str, &str)> = vec![("g", "a::t"); 201];

    let (_, small) = with_tests(&small);
    let (_, big) = with_tests(&big);

    assert_eq!(
        small.len(),
        20,
        "open: a window of the folders and their tests"
    );
    assert_eq!(small[2], "  v [  ] a (0/200)");
    assert_eq!(&big[1..], ["> [  ] g (0/201)"]);
}

#[test]
fn left_closes_an_open_folder_and_goes_to_the_folder_of_anything_else_and_right_opens_or_goes_in() {
    let (mut ui, _) = with_tests(&PROJECT);

    // On the first folder (fx): close it, and the others are what is left.
    let shown = press(&mut ui, "left");
    assert_eq!(shown[1], "> [  ] fx (0/3)");
    assert_eq!(shown[2], "v [  ] fx::it (0/1)", "{shown:?}");
    // Open it again, and go in: the first thing in it is the folder `tests`.
    press(&mut ui, "right");
    press(&mut ui, "right");
    press(&mut ui, "down");
    // Now on the test fails_here: left goes to its folder, which is `tests`.
    press(&mut ui, "left");
    let said = ui.handle(ToPlugin::action("run_selected"));
    let FromPlugin::Call { payload, .. } = &said[0] else {
        panic!("{said:?}");
    };
    let args: RunArgs = from_payload(payload).unwrap();
    assert_eq!(args.ids, Some(vec!["t0".to_owned(), "t1".to_owned()]));
}

#[test]
fn up_and_down_visit_only_what_is_shown() {
    let (mut ui, _) = with_tests(&PROJECT);
    press(&mut ui, "left");
    // fx is closed: down goes to the next folder, and up comes back to fx.
    press(&mut ui, "down");
    let said = ui.handle(ToPlugin::action("run_selected"));
    let FromPlugin::Call { payload, .. } = &said[0] else {
        panic!("{said:?}");
    };
    assert_eq!(
        from_payload::<RunArgs>(payload).unwrap().ids,
        Some(vec!["t3".to_owned()])
    );
    ui.handle(ToPlugin::action("cancel"));
    press(&mut ui, "up");
    let said = ui.handle(ToPlugin::action("run_selected"));
    let FromPlugin::Call { payload, .. } = said
        .iter()
        .find(|m| matches!(m, FromPlugin::Call { .. }))
        .unwrap()
    else {
        panic!();
    };
    let ids = from_payload::<RunArgs>(payload).unwrap().ids.unwrap();
    assert_eq!(ids, ["t0", "t1", "t2"], "fx, with everything under it");
}

#[test]
fn running_the_folder_that_has_every_test_runs_everything_without_listing_the_ids() {
    let (mut ui, _) = with_tests(&[("only", "a::x"), ("only", "a::y")]);

    let said = ui.handle(ToPlugin::action("run_selected"));

    let FromPlugin::Call { payload, .. } = &said[0] else {
        panic!("{said:?}");
    };
    assert_eq!(from_payload::<RunArgs>(payload).unwrap().ids, None);
}

#[test]
fn a_click_on_a_folder_opens_or_closes_it() {
    let (mut ui, _) = with_tests(&PROJECT);

    // The folder `tests` is the second line under the header, and is open.
    let shown = lines(&ui.handle(ToPlugin::Click { row: 2 }));
    assert_eq!(shown[2], "  > [  ] tests (0/2)", "closed: {shown:?}");
    let shown = lines(&ui.handle(ToPlugin::Click { row: 2 }));
    assert_eq!(shown[2], "  v [  ] tests (0/2)", "open again: {shown:?}");
    let shown = lines(&ui.handle(ToPlugin::Click { row: 1 }));
    assert_eq!(shown[1], "> [  ] fx (0/3)");
}

#[test]
fn a_folder_says_how_its_tests_are_doing_and_how_many_are_done() {
    let (mut ui, _) = with_tests(&PROJECT);
    ui.handle(ToPlugin::action("run_all"));
    let request = ui.running_call().unwrap();
    assert_eq!(
        lines(&ui.handle(ToPlugin::Refresh))[1],
        "v [..] fx (0/3)",
        "queued"
    );

    ui.handle(reply(request, event(1, "t0", Status::Passed), false));
    let shown = lines(&ui.handle(reply(request, event(1, "t1", Status::Passed), false)));
    assert_eq!(shown[1], "v [..] fx (2/3)", "two done, one to go");
    assert_eq!(shown[2], "  v [ok] tests (2/2)", "all of tests passed");

    let shown = lines(&ui.handle(reply(request, event(1, "t2", Status::Failed), false)));
    assert_eq!(
        shown[1], "v [FAIL] fx (3/3)",
        "a failure marks the folders above it"
    );
    assert!(shown[6].starts_with("    v [FAIL] er"), "{shown:?}");
    assert_eq!(shown[8], "v [..] fx::it (0/1)");
}

#[test]
fn what_a_cancel_gives_up_is_not_counted_in_the_folders() {
    let (mut ui, _) = with_tests(&PROJECT);
    ui.handle(ToPlugin::action("run_all"));
    let request = ui.running_call().unwrap();
    ui.handle(reply(request, event(1, "t0", Status::Passed), false));

    let shown = lines(&ui.handle(ToPlugin::action("cancel")));

    assert_eq!(shown[1], "v [  ] fx (1/3)");
    assert_eq!(shown[3], "    [ok] fails_here");
}

#[test]
fn an_adapter_that_died_makes_the_folders_unknown() {
    let (mut ui, _) = with_tests(&PROJECT);
    ui.handle(ToPlugin::action("run_all"));
    let request = ui.running_call().unwrap();

    let shown = lines(&ui.handle(ToPlugin::Refused {
        request,
        reason: Refusal::Gone,
    }));

    assert_eq!(shown[1], "v [?] fx (0/3)");
}

#[test]
fn the_pages_of_a_big_project_add_to_the_same_folders() {
    let mut ui = TestUi::default();
    ui.handle(ToPlugin::Start);
    let tests: Vec<(&str, &str)> = vec![("g", "m::t"); 6];
    let first = to_payload(&Page {
        offset: 0,
        total: 6,
        tests: named(&tests[..3]),
        error: None,
    });
    ui.handle(reply(1, first, true));
    let second = to_payload(&Page {
        offset: 3,
        total: 6,
        tests: named(&tests)[3..].to_vec(),
        error: None,
    });

    let shown = lines(&ui.handle(reply(2, second, true)));

    assert_eq!(shown[1], "v [  ] g (0/6)");
    assert_eq!(shown[2], "  v [  ] m (0/6)");
    assert_eq!(shown.len(), 9, "six tests in one folder, not two");
}

#[test]
fn a_big_flat_folder_is_walked_in_steps_the_size_of_the_pane_not_the_folder() {
    let tests: Vec<(&str, &str)> = vec![("bin", "case"); 2_000];
    let (mut ui, shown) = with_tests(&tests);
    assert_eq!(&shown[1..], ["> [  ] bin (0/2000)"], "it starts closed");
    press(&mut ui, "right");
    press(&mut ui, "right");
    for _ in 0..1_500 {
        press(&mut ui, "down");
    }

    let shown = press(&mut ui, "down");

    assert_eq!(shown.len(), 20);
    assert!(shown.last().unwrap().contains("case"));
}
