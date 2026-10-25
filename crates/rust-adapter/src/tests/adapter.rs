use trcode_plugin_api::{FromPlugin, Plugin, Refusal, RunStream, ToPlugin};
use trcode_test_protocol::{
    DebugArgs, DebugReply, DiscoverArgs, LocateArgs, LocateReply, Location, Page, RunArgs,
    RunEvent, Status, from_payload, method, to_payload,
};

use super::fixture;
use crate::RustAdapter;

const PANE: &str = "test-ui";

fn call(adapter: &mut RustAdapter, request: u64, name: &str, payload: String) -> Vec<FromPlugin> {
    adapter.handle(ToPlugin::Call {
        from: PANE.into(),
        request,
        method: name.into(),
        payload,
    })
}

fn discover(adapter: &mut RustAdapter, request: u64, offset: usize) -> Vec<FromPlugin> {
    let args = DiscoverArgs { offset, limit: 3 };
    call(adapter, request, method::DISCOVER, to_payload(&args))
}

fn run(
    adapter: &mut RustAdapter,
    request: u64,
    run_id: u64,
    ids: Option<&[&str]>,
) -> Vec<FromPlugin> {
    let args = RunArgs {
        run_id,
        ids: ids.map(|ids| ids.iter().map(|id| (*id).to_owned()).collect()),
    };
    call(adapter, request, method::RUN, to_payload(&args))
}

fn output(
    adapter: &mut RustAdapter,
    command: u64,
    stream: RunStream,
    text: &str,
) -> Vec<FromPlugin> {
    adapter.handle(ToPlugin::RunOutput {
        request: command,
        stream,
        text: text.into(),
    })
}

fn done(adapter: &mut RustAdapter, command: u64, code: Option<i32>) -> Vec<FromPlugin> {
    adapter.handle(ToPlugin::RunDone {
        request: command,
        code,
        cancelled: false,
        truncated: false,
    })
}

/// The command a message asks the editor to run.
fn command(said: &[FromPlugin]) -> (u64, &str, Vec<&str>) {
    match said {
        [
            FromPlugin::Run {
                request,
                program,
                args,
                cwd,
            },
        ] => {
            assert_eq!(cwd, "", "in the project");
            (*request, program, args.iter().map(String::as_str).collect())
        }
        other => panic!("not one command: {other:?}"),
    }
}

/// The replies to a call: its number, the payload and whether it is the last.
fn replies(said: &[FromPlugin]) -> Vec<(u64, String, bool)> {
    said.iter()
        .map(|message| match message {
            FromPlugin::Reply {
                to,
                request,
                payload,
                last,
            } => {
                assert_eq!(to, PANE);
                (*request, payload.clone(), *last)
            }
            other => panic!("not a reply: {other:?}"),
        })
        .collect()
}

fn run_events(said: &[FromPlugin]) -> Vec<RunEvent> {
    replies(said)
        .into_iter()
        .map(|(_, payload, _)| from_payload(&payload).expect("a run event"))
        .collect()
}

/// An adapter that has listed the fixture project.
fn listed() -> RustAdapter {
    let mut adapter = RustAdapter::default();
    let said = discover(&mut adapter, 1, 0);
    let (list, ..) = command(&said);
    output(&mut adapter, list, RunStream::Stdout, &fixture("list.json"));
    done(&mut adapter, list, Some(0));
    adapter
}

#[test]
fn the_first_page_asks_nextest_for_the_list_and_says_nothing_until_it_has_it() {
    let mut adapter = RustAdapter::default();

    let said = discover(&mut adapter, 1, 0);

    let (_, program, args) = command(&said);
    assert_eq!(program, "cargo");
    assert_eq!(
        args,
        [
            "nextest",
            "list",
            "--message-format",
            "json",
            "--color",
            "never"
        ]
    );
}

#[test]
fn when_the_list_is_there_the_caller_gets_the_first_page_and_the_pages_after_come_from_it() {
    let mut adapter = RustAdapter::default();
    let said = discover(&mut adapter, 7, 0);
    let (list, ..) = command(&said);
    let text = fixture("list.json");
    let (first, rest) = text.split_at(text.len() / 2);
    assert!(output(&mut adapter, list, RunStream::Stdout, first).is_empty());
    assert!(output(&mut adapter, list, RunStream::Stdout, rest).is_empty());

    let said = done(&mut adapter, list, Some(0));

    let replies = replies(&said);
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].0, 7);
    assert!(replies[0].2, "a page ends its call");
    let page: Page = from_payload(&replies[0].1).unwrap();
    assert_eq!((page.offset, page.total, page.tests.len()), (0, 5, 3));
    assert_eq!(page.error, None);
    // The next page needs no command.
    let said = discover(&mut adapter, 8, 3);
    let page: Page = from_payload(&replies_of(&said)[0]).unwrap();
    assert_eq!((page.offset, page.tests.len()), (3, 2));
    let said = discover(&mut adapter, 9, 50);
    let page: Page = from_payload(&replies_of(&said)[0]).unwrap();
    assert!(page.tests.is_empty(), "past the end");
}

fn replies_of(said: &[FromPlugin]) -> Vec<String> {
    replies(said)
        .into_iter()
        .map(|(_, payload, _)| payload)
        .collect()
}

#[test]
fn a_second_first_page_while_the_list_is_being_made_waits_for_the_same_command() {
    let mut adapter = RustAdapter::default();
    let said = discover(&mut adapter, 1, 0);
    let (list, ..) = command(&said);

    assert!(discover(&mut adapter, 2, 0).is_empty(), "no second command");
    output(&mut adapter, list, RunStream::Stdout, &fixture("list.json"));
    let said = done(&mut adapter, list, Some(0));

    let calls: Vec<u64> = replies(&said)
        .iter()
        .map(|(request, ..)| *request)
        .collect();
    assert_eq!(calls, [1, 2]);
}

#[test]
fn a_list_that_asks_again_makes_the_list_again() {
    let mut adapter = listed();

    let said = discover(&mut adapter, 2, 0);

    let (second, ..) = command(&said);
    assert_eq!(second, 2, "a command of its own");
}

#[test]
fn a_list_that_failed_is_a_page_that_says_why() {
    let mut adapter = RustAdapter::default();
    let said = discover(&mut adapter, 1, 0);
    let (list, ..) = command(&said);
    output(
        &mut adapter,
        list,
        RunStream::Stderr,
        "error: could not compile `fx`\n",
    );

    let said = done(&mut adapter, list, Some(101));

    let page: Page = from_payload(&replies_of(&said)[0]).unwrap();
    let error = page.error.expect("it says why");
    assert!(
        error.contains("cargo nextest list ended with 101"),
        "{error}"
    );
    assert!(error.contains("could not compile"), "{error}");
    assert_eq!(page.total, 0);
}

#[test]
fn a_list_that_was_cut_short_or_too_long_is_a_page_that_says_so() {
    let mut adapter = RustAdapter::default();
    let said = discover(&mut adapter, 1, 0);
    let (list, ..) = command(&said);
    output(
        &mut adapter,
        list,
        RunStream::Stdout,
        "{\"rust-suites\":{\"a\":",
    );
    let said = done(&mut adapter, list, Some(0));
    let page: Page = from_payload(&replies_of(&said)[0]).unwrap();
    assert!(page.error.unwrap().contains("ended in the middle"));

    let said = discover(&mut adapter, 2, 0);
    let (list, ..) = command(&said);
    let said = adapter.handle(ToPlugin::RunDone {
        request: list,
        code: Some(0),
        cancelled: false,
        truncated: true,
    });
    let page: Page = from_payload(&replies_of(&said)[0]).unwrap();
    assert!(page.error.unwrap().contains("too long"));
}

#[test]
fn a_command_the_editor_would_not_run_is_said_to_the_caller_in_words() {
    for (reason, words) in [
        (Refusal::Untrusted, "not trusted"),
        (
            Refusal::Failed {
                detail: "No such file".into(),
            },
            "cargo could not be started: No such file",
        ),
        (
            Refusal::NotAllowed {
                detail: "no entry".into(),
            },
            "no entry",
        ),
    ] {
        let mut adapter = RustAdapter::default();
        let said = discover(&mut adapter, 1, 0);
        let (list, ..) = command(&said);

        let said = adapter.handle(ToPlugin::Refused {
            request: list,
            reason,
        });

        let page: Page = from_payload(&replies_of(&said)[0]).unwrap();
        assert!(page.error.unwrap().contains(words), "{words}");
    }
}

#[test]
fn running_everything_is_one_command_without_a_filter() {
    let mut adapter = listed();

    let said = run(&mut adapter, 5, 1, None);

    let (_, program, args) = command(&said);
    assert_eq!(program, "cargo");
    assert_eq!(
        args,
        ["nextest", "run", "--no-fail-fast", "--color", "never"]
    );
}

#[test]
fn running_some_tests_filters_by_binary_and_name() {
    let mut adapter = listed();

    let said = run(
        &mut adapter,
        5,
        1,
        Some(&["fx::bin/fx tests::parses_empty"]),
    );

    let (_, _, args) = command(&said);
    assert_eq!(
        &args[..5],
        ["nextest", "run", "--no-fail-fast", "--color", "never"]
    );
    assert_eq!(args[5], "-E");
    assert_eq!(
        args[6],
        "(binary_id(=fx::bin/fx) & test(=tests::parses_empty))"
    );
}

#[test]
fn a_run_tells_each_test_as_its_line_comes_and_ends_with_done() {
    let mut adapter = listed();
    let said = run(&mut adapter, 5, 3, None);
    let (command, ..) = command(&said);
    let text = fixture("run.txt");
    let (head, tail) = text.split_at(text.find("PASS").unwrap() + 60);

    let first = output(&mut adapter, command, RunStream::Stderr, head);
    let rest = output(&mut adapter, command, RunStream::Stderr, tail);
    let end = done(&mut adapter, command, Some(100));

    let mut events = run_events(&first);
    events.extend(run_events(&rest));
    let finished: Vec<&RunEvent> = events
        .iter()
        .filter(|event| matches!(event, RunEvent::Finished { .. }))
        .collect();
    assert_eq!(finished.len(), 5);
    assert!(events.iter().all(|event| event.run_id() == Some(3)));
    // Every test starts before it finishes.
    assert!(
        matches!(&events[0], RunEvent::Started { id, .. } if id == "fx::bin/fx tests::parses_empty")
    );
    let failed = events.iter().find_map(|event| match event {
        RunEvent::Finished {
            id,
            status: Status::Failed,
            message,
            ..
        } => Some((id, message)),
        _ => None,
    });
    let (id, message) = failed.expect("one failed");
    assert_eq!(id, "fx tests::fails_here");
    assert!(message.contains("panicked at"), "{message}");
    // The panic says where the test failed.
    let place = events.iter().find_map(|event| match event {
        RunEvent::Finished {
            status: Status::Failed,
            location,
            ..
        } => location.clone(),
        _ => None,
    });
    assert_eq!(
        place,
        Some(Location {
            path: "src/lib.rs".into(),
            line: 14,
            column: 9
        })
    );
    // The end: done, and the last reply of the call.
    let end = replies(&end);
    assert_eq!(end.len(), 1);
    assert_eq!(
        from_payload::<RunEvent>(&end[0].1),
        Some(RunEvent::Done { run_id: 3 })
    );
    assert!(end[0].2);
    assert!(
        replies(&first)
            .iter()
            .all(|(request, _, last)| *request == 5 && !last)
    );
}

#[test]
fn a_run_that_found_nothing_to_run_says_so_and_so_does_one_that_did_not_build() {
    for (code, tail, words) in [
        (Some(4), "error: no tests to run\n", "no test matched"),
        (
            Some(101),
            "error: could not compile `fx`\n",
            "ended with 101",
        ),
        (None, "", "was stopped"),
    ] {
        let mut adapter = listed();
        let said = run(&mut adapter, 5, 1, None);
        let (command, ..) = command(&said);
        output(&mut adapter, command, RunStream::Stderr, tail);

        let events = run_events(&done(&mut adapter, command, code));

        assert_eq!(events.len(), 2, "{events:?}");
        match &events[0] {
            RunEvent::Error { message, .. } => assert!(message.contains(words), "{message}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(events[1], RunEvent::Done { run_id: 1 });
    }
}

#[test]
fn a_run_where_tests_failed_is_not_an_error() {
    let mut adapter = listed();
    let said = run(&mut adapter, 5, 1, None);
    let (command, ..) = command(&said);
    output(
        &mut adapter,
        command,
        RunStream::Stderr,
        "        FAIL [   0.004s] (1/1) fx t\n",
    );

    let events = run_events(&done(&mut adapter, command, Some(100)));

    assert!(
        events
            .iter()
            .all(|event| !matches!(event, RunEvent::Error { .. }))
    );
    assert!(matches!(events.last(), Some(RunEvent::Done { .. })));
}

#[test]
fn a_cancel_stops_the_command_and_what_it_still_says_is_not_listened_to() {
    let mut adapter = listed();
    let said = run(&mut adapter, 5, 1, None);
    let (command, ..) = command(&said);

    let said = adapter.handle(ToPlugin::CallCancelled {
        from: PANE.into(),
        request: 5,
    });

    assert!(matches!(said.as_slice(), [FromPlugin::RunCancel { request }] if *request == command));
    assert!(
        output(
            &mut adapter,
            command,
            RunStream::Stderr,
            "        PASS [   0.003s] (1/1) fx a\n"
        )
        .is_empty()
    );
    assert!(done(&mut adapter, command, None).is_empty());
}

#[test]
fn a_cancel_of_a_call_that_is_not_running_does_nothing() {
    let mut adapter = listed();

    assert!(
        adapter
            .handle(ToPlugin::CallCancelled {
                from: PANE.into(),
                request: 99
            })
            .is_empty()
    );
    assert!(
        adapter
            .handle(ToPlugin::CallCancelled {
                from: "other".into(),
                request: 1
            })
            .is_empty()
    );
}

#[test]
fn giving_up_the_only_call_waiting_for_the_list_stops_the_list_and_giving_up_one_of_two_does_not() {
    let mut adapter = RustAdapter::default();
    let said = discover(&mut adapter, 1, 0);
    let (list, ..) = command(&said);
    discover(&mut adapter, 2, 0);

    assert!(
        adapter
            .handle(ToPlugin::CallCancelled {
                from: PANE.into(),
                request: 1
            })
            .is_empty()
    );
    let said = adapter.handle(ToPlugin::CallCancelled {
        from: PANE.into(),
        request: 2,
    });

    assert!(matches!(said.as_slice(), [FromPlugin::RunCancel { request }] if *request == list));
    assert!(done(&mut adapter, list, None).is_empty());
}

#[test]
fn two_runs_at_once_are_kept_apart() {
    let mut adapter = listed();
    let one = run(&mut adapter, 5, 1, None);
    let two = run(&mut adapter, 6, 2, None);
    let (first, ..) = command(&one);
    let (second, ..) = command(&two);
    assert_ne!(first, second);

    let said = output(
        &mut adapter,
        second,
        RunStream::Stderr,
        "        PASS [   0.003s] (1/1) fx a\n",
    );

    let events = replies(&said);
    assert!(events.iter().all(|(request, ..)| *request == 6));
    assert_eq!(run_events(&said)[0].run_id(), Some(2));
}

#[test]
fn a_run_that_is_refused_says_why_and_ends() {
    let mut adapter = listed();
    let said = run(&mut adapter, 5, 1, None);
    let (command, ..) = command(&said);

    let said = adapter.handle(ToPlugin::Refused {
        request: command,
        reason: Refusal::Untrusted,
    });

    let events = run_events(&said);
    assert!(
        matches!(&events[0], RunEvent::Error { message, .. } if message.contains("not trusted"))
    );
    assert_eq!(events[1], RunEvent::Done { run_id: 1 });
    assert!(replies(&said)[1].2);
}

#[test]
fn ids_that_are_not_tests_make_a_run_that_says_so_without_a_command() {
    let mut adapter = listed();

    let said = run(&mut adapter, 5, 1, Some(&["nonsense"]));

    let events = run_events(&said);
    assert!(matches!(&events[0], RunEvent::Error { .. }));
    assert_eq!(events[1], RunEvent::Done { run_id: 1 });
}

#[test]
fn what_it_is_not_asked_it_answers_or_ignores() {
    let mut adapter = RustAdapter::default();

    let said = call(&mut adapter, 3, "advance", "{}".into());
    assert_eq!(replies(&said), [(3, "{}".to_owned(), true)]);
    assert!(adapter.handle(ToPlugin::Start).is_empty());
    assert!(adapter.handle(ToPlugin::Trust { trusted: true }).is_empty());
    let said = call(&mut adapter, 4, method::RUN, "nonsense".into());
    assert_eq!(replies(&said), [(4, String::new(), true)]);
}

#[test]
fn output_for_a_command_it_does_not_know_is_ignored() {
    let mut adapter = RustAdapter::default();

    assert!(
        output(
            &mut adapter,
            77,
            RunStream::Stderr,
            "        PASS [   0.003s] (1/1) fx a\n"
        )
        .is_empty()
    );
    assert!(done(&mut adapter, 77, Some(0)).is_empty());
}

#[test]
fn only_what_nextest_writes_to_stderr_is_read_for_a_run() {
    let mut adapter = listed();
    let said = run(&mut adapter, 5, 1, None);
    let (command, ..) = command(&said);

    let said = output(
        &mut adapter,
        command,
        RunStream::Stdout,
        "        PASS [   0.003s] (1/1) fx a\n",
    );

    assert!(said.is_empty());
}

// --- finding the code of a test ---

fn locate(adapter: &mut RustAdapter, request: u64, id: &str) -> Vec<FromPlugin> {
    let args = LocateArgs { id: id.to_owned() };
    call(adapter, request, method::LOCATE, to_payload(&args))
}

fn located(said: &[FromPlugin]) -> LocateReply {
    let replies = replies(said);
    assert_eq!(replies.len(), 1);
    assert!(replies[0].2, "the last reply");
    from_payload(&replies[0].1).expect("a locate reply")
}

#[test]
fn a_test_is_looked_for_with_grep_in_the_project_and_the_best_match_is_the_answer() {
    let mut adapter = listed();

    let said = locate(&mut adapter, 8, "fx tests::parses_empty");
    let (search, program, args) = command(&said);

    assert_eq!(program, "grep");
    assert!(args.contains(&"fn parses_empty"), "{args:?}");
    assert!(args.contains(&"--exclude-dir=target"), "{args:?}");
    let found = output(
        &mut adapter,
        search,
        RunStream::Stdout,
        "./src/lib.rs:20:    fn parses_empty() {\n",
    );
    assert!(found.is_empty(), "nothing is said until it is over");
    let end = done(&mut adapter, search, Some(0));
    assert_eq!(
        located(&end),
        LocateReply {
            location: Some(Location {
                path: "src/lib.rs".into(),
                line: 20,
                column: 5
            }),
            error: None
        }
    );
}

#[test]
fn a_test_nobody_listed_or_with_no_match_has_no_place_and_no_complaint() {
    let mut adapter = listed();
    let said = locate(&mut adapter, 8, "fx nothing::of::the_kind");
    assert_eq!(located(&said), LocateReply::default());

    let said = locate(&mut adapter, 9, "fx tests::parses_empty");
    let (search, ..) = command(&said);
    let end = done(&mut adapter, search, Some(1));
    assert_eq!(located(&end), LocateReply::default());
}

#[test]
fn a_search_that_could_not_be_made_says_so() {
    let mut adapter = listed();
    let said = locate(&mut adapter, 8, "fx tests::parses_empty");
    let (search, ..) = command(&said);

    let end = adapter.handle(ToPlugin::Refused {
        request: search,
        reason: Refusal::Failed {
            detail: "no such program".into(),
        },
    });

    let answer = located(&end);
    assert_eq!(answer.location, None);
    assert!(answer.error.unwrap().contains("grep"));
}

#[test]
fn a_search_the_pane_gave_up_is_stopped() {
    let mut adapter = listed();
    let said = locate(&mut adapter, 8, "fx tests::parses_empty");
    let (search, ..) = command(&said);

    let said = adapter.handle(ToPlugin::CallCancelled {
        from: PANE.into(),
        request: 8,
    });

    assert!(matches!(
        said.as_slice(),
        [FromPlugin::RunCancel { request }] if *request == search
    ));
    // What the command says after that is not anyone's.
    assert!(output(&mut adapter, search, RunStream::Stdout, "./a.rs:1:fn x\n").is_empty());
}

// --- debugging ---

fn debug(adapter: &mut RustAdapter, request: u64, id: &str) -> Vec<FromPlugin> {
    let args = DebugArgs { id: id.into() };
    call(adapter, request, method::DEBUG, to_payload(&args))
}

fn debug_reply(said: &[FromPlugin]) -> DebugReply {
    let replies = replies(said);
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert!(replies[0].2, "the last");
    from_payload(&replies[0].1).expect("a debug reply")
}

/// The list command a debug asked for, finished with the fixture's list.
fn debug_with_the_fixture(id: &str) -> DebugReply {
    let mut adapter = listed();
    let said = debug(&mut adapter, 9, id);
    let (list, program, args) = command(&said);
    assert_eq!(program, "cargo");
    assert_eq!(args[..3], ["nextest", "list", "--message-format"]);
    output(&mut adapter, list, RunStream::Stdout, &fixture("list.json"));
    debug_reply(&done(&mut adapter, list, Some(0)))
}

#[test]
fn debugging_builds_the_tests_again_and_then_says_how_to_start_the_one_asked_for() {
    let reply = debug_with_the_fixture("fx tests::panics_ok");

    assert!(
        reply.program.ends_with("/debug/deps/fx-f32bc118f496b3b9"),
        "{reply:?}"
    );
    assert_eq!(reply.args, ["--exact", "tests::panics_ok", "--nocapture"]);
    assert!(reply.cwd.ends_with("tests/fixture-project"), "{reply:?}");
    assert_eq!(
        reply.env,
        [
            format!("CARGO_MANIFEST_DIR={}", reply.cwd),
            "CARGO_PKG_NAME=fx".to_owned()
        ]
    );
    assert_eq!(reply.error, None);
}

#[test]
fn a_test_of_the_binary_of_a_folder_of_tests_is_started_from_that_binary() {
    let reply = debug_with_the_fixture("fx::it tests::parses_empty");

    assert!(reply.program.contains("/it-d61f47b8e89ce4d9"), "{reply:?}");
}

#[test]
fn a_binary_that_is_not_in_the_list_cannot_be_debugged_and_the_reply_says_so() {
    let reply = debug_with_the_fixture("nothing like it");

    assert_eq!(reply.program, "");
    assert!(
        reply.error.clone().unwrap().contains("not in the list"),
        "{reply:?}"
    );
    let reply = debug_with_the_fixture("ghost some::test");
    assert!(reply.error.unwrap().contains("ghost"));
}

#[test]
fn a_build_that_fails_is_the_answer_to_a_debug_with_what_cargo_said() {
    let mut adapter = RustAdapter::default();
    let said = debug(&mut adapter, 9, "fx tests::a");
    let (list, ..) = command(&said);
    output(
        &mut adapter,
        list,
        RunStream::Stderr,
        "error[E0425]: cannot find value `x`\n",
    );

    let reply = debug_reply(&done(&mut adapter, list, Some(101)));

    assert_eq!(reply.program, "");
    let error = reply.error.unwrap();
    assert!(
        error.contains("ended with 101") && error.contains("E0425"),
        "{error}"
    );
}

#[test]
fn a_debug_that_is_given_up_stops_the_build_when_nobody_else_waits_for_it() {
    let mut adapter = RustAdapter::default();
    let said = debug(&mut adapter, 9, "fx tests::a");
    let (list, ..) = command(&said);

    let said = adapter.handle(ToPlugin::CallCancelled {
        from: PANE.into(),
        request: 9,
    });

    assert_eq!(said, [FromPlugin::RunCancel { request: list }]);
}

#[test]
fn a_debug_and_a_discovery_at_the_same_time_share_one_build() {
    let mut adapter = RustAdapter::default();
    let said = discover(&mut adapter, 1, 0);
    let (list, ..) = command(&said);

    assert_eq!(debug(&mut adapter, 2, "fx tests::panics_ok"), []);
    output(&mut adapter, list, RunStream::Stdout, &fixture("list.json"));
    let said = done(&mut adapter, list, Some(0));

    let replies = replies(&said);
    assert_eq!(replies.iter().map(|r| r.0).collect::<Vec<_>>(), [1, 2]);
}

#[test]
fn a_list_that_fails_to_build_says_so_first_and_then_what_the_compiler_said() {
    let mut adapter = RustAdapter::default();
    let said = discover(&mut adapter, 1, 0);
    let (list, ..) = command(&said);
    let errors = "   Compiling fx v0.1.0\nerror[E0425]: cannot find value `x`\n --> src/lib.rs:3:5\nerror: could not compile `fx` (lib) due to 1 previous error\n";
    output(&mut adapter, list, RunStream::Stderr, errors);

    let page = &replies(&done(&mut adapter, list, Some(101)))[0].1;

    let error = from_payload::<Page>(page).unwrap().error.unwrap();
    let mut lines = error.lines();
    assert_eq!(
        lines.next(),
        Some("the project does not build (cargo nextest list ended with 101)")
    );
    assert!(
        error.contains("error[E0425]: cannot find value `x`\n --> src/lib.rs:3:5"),
        "{error}"
    );
}

#[test]
fn a_list_that_fails_for_another_reason_keeps_its_one_line() {
    let mut adapter = RustAdapter::default();
    let said = discover(&mut adapter, 1, 0);
    let (list, ..) = command(&said);
    output(
        &mut adapter,
        list,
        RunStream::Stderr,
        "error: no such command: `nextest`\n",
    );

    let page = &replies(&done(&mut adapter, list, Some(101)))[0].1;

    let error = from_payload::<Page>(page).unwrap().error.unwrap();
    assert_eq!(
        error,
        "cargo nextest list ended with 101: error: no such command: `nextest`"
    );
}
