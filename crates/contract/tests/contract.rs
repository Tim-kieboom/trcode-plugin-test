//! What v0 has to prove: the five things that break the shape of the calls between the
//! test pane and an adapter, each with the one sentence that says it is passed. The pane
//! is the real plugin and so is the adapter; the editor is the stand-in of this crate.

use trcode_plugin_api::{Abi, Capability, MAX_MESSAGE, Manifest, ToPlugin};
use trcode_test_contract::MiniHost;
use trcode_test_fake_adapter::FakeAdapter;
use trcode_test_protocol::{
    AdvanceArgs, PAGE, Page, RunEvent, Scenario, from_payload, method, to_payload,
};
use trcode_test_ui::{RowStatus, TestUi};

const UI: &str = "test-ui";
const ADAPTER: &str = "fake-adapter";

/// The editor with the adapter (told what to be like) and then the pane, which asks for
/// the tests as it starts.
fn started(scenario: &Scenario) -> MiniHost {
    let mut host = MiniHost::new();
    host.add::<FakeAdapter>(ADAPTER, &[Capability::PluginServe]);
    host.run_until_quiet();
    call_adapter(&mut host, method::CONFIGURE, to_payload(scenario));
    host.add::<TestUi>(UI, &[Capability::PluginCall]);
    host.run_until_quiet();
    host
}

/// A call to the adapter from nobody in particular, as a test of the editor's would.
fn call_adapter(host: &mut MiniHost, name: &str, payload: String) {
    host.send(
        ADAPTER,
        ToPlugin::Call {
            from: "harness".into(),
            request: 0,
            method: name.into(),
            payload,
        },
    );
}

/// Turns the fake clock: each run finishes `n` more tests.
fn advance(host: &mut MiniHost, n: usize) {
    call_adapter(host, method::ADVANCE, to_payload(&AdvanceArgs { n }));
}

fn ui(host: &MiniHost) -> &TestUi {
    host.plugin::<TestUi>(UI).expect("the pane is there")
}

fn press(host: &mut MiniHost, action: &str) {
    host.send(UI, ToPlugin::action(action));
}

fn status(host: &MiniHost, id: &str) -> RowStatus {
    ui(host).status_of(id).expect("a test of that id")
}

// --- ids: two tests with the same name are two rows, and one runs alone ---

#[test]
fn ids_two_tests_with_the_same_name_are_two_rows_and_re_running_one_runs_only_that_one() {
    let mut host = started(&Scenario {
        tests: 4,
        same_names: true,
        ..Scenario::default()
    });
    let lines = host.lines(UI);
    // The header, and four rows, two of which have the same name.
    assert_eq!(lines.len(), 5, "{lines:?}");
    assert!(lines[1].ends_with("bin0 case_0"), "{lines:?}");
    assert!(lines[2].ends_with("bin1 case_0"), "{lines:?}");

    press(&mut host, "down");
    press(&mut host, "run_selected");
    advance(&mut host, 10);

    assert_eq!(status(&host, "t1"), RowStatus::Passed);
    for other in ["t0", "t2", "t3"] {
        assert_eq!(status(&host, other), RowStatus::Unrun, "{other}");
    }
}

// --- cancel: a re-run right after a cancel shows only the second run ---

/// The events that reached the pane that belong to the run `run_id`.
fn events_delivered(host: &MiniHost, run_id: u64) -> usize {
    host.delivered()
        .iter()
        .filter(|delivered| delivered.to == UI)
        .filter_map(|delivered| match &delivered.message {
            ToPlugin::Reply { payload, .. } => from_payload::<RunEvent>(payload),
            _ => None,
        })
        .filter(|event| event.run_id() == Some(run_id))
        .count()
}

#[test]
fn cancel_then_an_immediate_re_run_shows_only_the_second_runs_results() {
    // An adapter that does not listen to a cancel: it goes on telling the first run.
    let mut host = started(&Scenario {
        tests: 10,
        talks_after_cancel: true,
        ..Scenario::default()
    });
    press(&mut host, "run_all");
    advance(&mut host, 2);
    let first_run_events = events_delivered(&host, 1);
    assert_eq!(first_run_events, 4, "two tests, each started and finished");

    press(&mut host, "cancel");
    press(&mut host, "run_all");
    advance(&mut host, 3);

    assert_eq!(
        events_delivered(&host, 1),
        first_run_events,
        "nothing of the first run came after it was given up"
    );
    assert_eq!(events_delivered(&host, 2), 6);
    assert_eq!(ui(&host).running(), Some(2));
    // The three tests of the second run have results; the rest of the first run is gone.
    assert_eq!(status(&host, "t2"), RowStatus::Passed);
    assert_eq!(status(&host, "t5"), RowStatus::Queued);
}

#[test]
fn cancel_what_the_run_had_not_finished_is_not_run_and_what_it_had_keeps_its_result() {
    let mut host = started(&Scenario {
        tests: 6,
        ..Scenario::default()
    });
    press(&mut host, "run_all");
    advance(&mut host, 2);

    press(&mut host, "cancel");
    advance(&mut host, 10);

    assert_eq!(status(&host, "t0"), RowStatus::Passed);
    assert_eq!(status(&host, "t3"), RowStatus::Unrun);
    assert_eq!(ui(&host).running(), None);
    assert_eq!(host.open_calls(), 0, "nothing is left open");
}

#[test]
fn cancel_an_event_of_an_earlier_run_that_got_through_anyway_is_ignored_by_the_pane() {
    let mut host = started(&Scenario {
        tests: 3,
        ..Scenario::default()
    });
    press(&mut host, "run_all");
    press(&mut host, "run_all");
    let request = ui(&host).running_call().expect("a run is going");
    let stale = RunEvent::Finished {
        run_id: 1,
        id: "t2".into(),
        status: trcode_test_protocol::Status::Failed,
        duration_ms: 1,
        message: String::new(),
    };

    // The editor would drop this; if it did not, the pane still does.
    host.send(
        UI,
        ToPlugin::Reply {
            from: ADAPTER.into(),
            request,
            payload: to_payload(&stale),
            last: false,
        },
    );

    assert_eq!(status(&host, "t2"), RowStatus::Queued);
}

// --- death: the adapter traps in the middle of a run ---

#[test]
fn death_running_tests_become_unknown_at_once_and_the_next_run_works() {
    let mut host = started(&Scenario {
        tests: 10,
        traps_on_advance: true,
        ..Scenario::default()
    });
    press(&mut host, "run_all");

    advance(&mut host, 1);

    assert_eq!(host.deaths(), [ADAPTER]);
    assert_eq!(ui(&host).running(), None);
    assert_eq!(ui(&host).counts().of(RowStatus::Unknown), 10);
    assert_eq!(
        ui(&host).counts().of(RowStatus::Queued),
        0,
        "no spinner is left"
    );
    assert!(
        ui(&host).notice().contains("went away"),
        "{}",
        ui(&host).notice()
    );
    assert_eq!(host.open_calls(), 0);
    let header = host.lines(UI).remove(0);
    assert!(
        header.contains("? 10") && header.contains("went away"),
        "{header}"
    );

    // The adapter is a fresh one, with its own six tests: a run of one of them works.
    press(&mut host, "run_selected");
    advance(&mut host, 1);
    assert_eq!(status(&host, "t0"), RowStatus::Passed);
}

#[test]
fn death_a_pane_that_dies_cancels_its_run_and_starts_again_with_the_tests() {
    let mut host = started(&Scenario {
        tests: 5,
        ..Scenario::default()
    });
    press(&mut host, "run_all");
    assert_eq!(host.open_calls(), 1);

    host.kill(UI);

    assert_eq!(host.deaths(), [UI]);
    // The adapter was told, and the fresh pane asked for the tests again.
    assert!(
        host.delivered()
            .iter()
            .any(|delivered| delivered.to == ADAPTER
                && matches!(delivered.message, ToPlugin::CallCancelled { .. }))
    );
    assert_eq!(host.open_calls(), 0);
    assert_eq!(ui(&host).discovered(), (5, 5));
    assert_eq!(ui(&host).running(), None);
    // What the adapter had not said is not said to the fresh pane.
    advance(&mut host, 5);
    assert_eq!(ui(&host).counts().of(RowStatus::Passed), 0);
}

// --- size: twenty thousand tests, pulled a page at a time ---

#[test]
fn size_twenty_thousand_tests_come_in_pages_and_the_first_is_shown_before_the_last_arrives() {
    let mut host = MiniHost::new();
    host.add::<FakeAdapter>(ADAPTER, &[Capability::PluginServe]);
    host.run_until_quiet();
    call_adapter(
        &mut host,
        method::CONFIGURE,
        to_payload(&Scenario {
            tests: 20_000,
            ..Scenario::default()
        }),
    );
    host.add::<TestUi>(UI, &[Capability::PluginCall]);

    let mut first_page_shown = None;
    while host.step() {
        let known = host.plugin::<TestUi>(UI).map_or(0, |ui| ui.discovered().0);
        if first_page_shown.is_none() && known > 0 {
            first_page_shown = Some((known, host.lines(UI).len(), pages_delivered(&host)));
        }
    }

    let (known, shown, pages_by_then) = first_page_shown.expect("a page was shown");
    assert_eq!(known, PAGE);
    assert!(shown > 1, "rows were drawn after the first page");
    assert_eq!(pages_by_then, 1, "only one page had arrived");
    assert_eq!(ui(&host).discovered(), (20_000, 20_000));
    assert_eq!(pages_delivered(&host), 20_000 / PAGE);
}

/// The pages of discovery that reached the pane: none is more than a page or a message.
fn pages_delivered(host: &MiniHost) -> usize {
    let mut pages = 0;
    for delivered in host
        .delivered()
        .iter()
        .filter(|delivered| delivered.to == UI)
    {
        let ToPlugin::Reply { payload, .. } = &delivered.message else {
            continue;
        };
        let Some(page) = from_payload::<Page>(payload) else {
            continue;
        };
        assert!(page.tests.len() <= PAGE);
        assert!(
            payload.len() < MAX_MESSAGE / 10,
            "a page is well under the limit"
        );
        pages += 1;
    }
    pages
}

#[test]
fn size_the_pane_draws_a_window_however_many_tests_there_are() {
    let mut host = started(&Scenario {
        tests: 20_000,
        ..Scenario::default()
    });

    let lines = host.lines(UI).len();
    for _ in 0..100 {
        press(&mut host, "down");
    }

    assert_eq!(lines, 20, "the header and the first nineteen");
    assert_eq!(host.lines(UI).len(), 20, "still a window after scrolling");
    host.send(
        UI,
        ToPlugin::Size {
            width: 80,
            height: 10,
        },
    );
    assert_eq!(host.lines(UI).len(), 10);
}

#[test]
fn size_a_run_of_all_of_them_ends_with_every_test_counted_once() {
    let mut host = started(&Scenario {
        tests: 20_000,
        ..Scenario::default()
    });
    press(&mut host, "run_all");

    advance(&mut host, 20_000);

    let counts = ui(&host).counts();
    let finished =
        counts.of(RowStatus::Passed) + counts.of(RowStatus::Failed) + counts.of(RowStatus::Skipped);
    assert_eq!(finished, 20_000);
    assert_eq!(ui(&host).running(), None);
    assert_eq!(host.open_calls(), 0);
}

// --- stalls: what the manifests have to say, since a stand-in has no thread ---

fn manifest(path: &str) -> Manifest {
    let text = std::fs::read_to_string(format!("{}/../{path}", env!("CARGO_MANIFEST_DIR")))
        .unwrap_or_else(|error| panic!("{path}: {error}"));
    toml::from_str(&text).unwrap_or_else(|error| panic!("{path}: {error}"))
}

#[test]
fn stalls_an_adapter_asks_for_a_thread_of_its_own_so_a_slow_one_stops_only_itself() {
    let adapter = manifest("fake-adapter/manifest.toml");

    assert_eq!(adapter.thread, trcode_plugin_api::Thread::Own);
}

#[test]
fn the_manifests_are_for_this_plugin_api_and_ask_for_what_the_calls_need_and_no_more() {
    let adapter = manifest("fake-adapter/manifest.toml");
    let pane = manifest("test-ui/manifest.toml");

    for manifest in [&adapter, &pane] {
        assert_eq!(Abi::CURRENT.check(manifest.abi.parse().unwrap()), Ok(()));
        assert!(manifest.run.is_empty(), "v0 runs no commands");
    }
    assert!(!adapter.view, "an adapter has no pane to place");
    assert!(pane.view);
    assert_eq!(pane.thread, trcode_plugin_api::Thread::Own);
    assert_eq!(adapter.capabilities, ["plugin.serve"]);
    assert_eq!(pane.capabilities, ["plugin.call"]);
    for name in adapter.capabilities.iter().chain(&pane.capabilities) {
        assert!(Capability::parse(name).is_some(), "{name}");
    }
    let ids: Vec<&str> = pane
        .actions
        .iter()
        .map(|action| action.id.as_str())
        .collect();
    for action in [
        "discover",
        "run_all",
        "run_selected",
        "cancel",
        "up",
        "down",
        "advance",
    ] {
        assert!(ids.contains(&action), "{action}");
    }
}

// --- the Rust adapter, with the pane and a real nextest ---

fn nextest_is_installed() -> bool {
    std::process::Command::new("cargo")
        .args(["nextest", "--version"])
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn the_pane_lists_runs_and_shows_the_tests_of_a_real_project_through_the_rust_adapter() {
    if !nextest_is_installed() {
        eprintln!("cargo-nextest is not installed: skipped");
        return;
    }
    let project = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rust-adapter/tests/fixture-project");
    let target = tempfile::tempdir().unwrap();
    let manifest = manifest("rust-adapter/manifest.toml");
    let patterns = trcode_plugin_api::parse_all(&manifest.run).unwrap();
    let mut host = MiniHost::new();
    host.run_commands_in(project, target.path().to_path_buf());
    host.add::<trcode_test_rust_adapter::RustAdapter>(
        "rust-adapter",
        &[Capability::PluginServe, Capability::ProcessRun],
    );
    host.allow_commands("rust-adapter", patterns);
    host.add::<TestUi>(UI, &[Capability::PluginCall]);
    host.run_until_quiet();
    // The pane starts with the fake adapter, which is not here.
    assert!(
        ui(&host)
            .notice()
            .contains("no adapter called fake-adapter")
    );

    press(&mut host, "adapter");

    assert_eq!(ui(&host).adapter(), "rust-adapter");
    assert_eq!(ui(&host).discovered(), (5, 5), "{}", ui(&host).notice());
    press(&mut host, "run_all");
    assert_eq!(
        ui(&host).running(),
        None,
        "the run is over: {}",
        ui(&host).notice()
    );
    assert_eq!(status(&host, "fx tests::fails_here"), RowStatus::Failed);
    assert_eq!(status(&host, "fx tests::parses_empty"), RowStatus::Passed);
    assert_eq!(
        status(&host, "fx::bin/fx tests::parses_empty"),
        RowStatus::Passed
    );
    assert_eq!(status(&host, "fx::it parses_empty"), RowStatus::Passed);
    assert_eq!(ui(&host).counts().of(RowStatus::Failed), 1);
    assert_eq!(ui(&host).counts().of(RowStatus::Passed), 4);
    assert_eq!(host.open_calls(), 0);

    // Re-run one: the one that shares its name with two others in other binaries.
    for _ in 0..4 {
        press(&mut host, "down");
    }
    host.send(UI, ToPlugin::action("run_selected"));
    assert_eq!(ui(&host).running(), None);
    assert_eq!(ui(&host).counts().of(RowStatus::Queued), 0);
}

#[test]
fn the_rust_adapters_manifest_allows_what_it_asks_to_run_and_nothing_to_start_with() {
    use trcode_plugin_api::{FromPlugin, Plugin};
    let manifest = manifest("rust-adapter/manifest.toml");
    let patterns = trcode_plugin_api::parse_all(&manifest.run).unwrap();
    let mut adapter = trcode_test_rust_adapter::RustAdapter::default();
    let call = |method: &str, payload: String| ToPlugin::Call {
        from: UI.into(),
        request: 1,
        method: method.into(),
        payload,
    };

    let mut asked = Vec::new();
    asked.extend(adapter.handle(call(method::DISCOVER, "{}".into())));
    asked.extend(adapter.handle(call(
        method::RUN,
        to_payload(&trcode_test_protocol::RunArgs {
            run_id: 1,
            ids: None,
        }),
    )));
    asked.extend(adapter.handle(call(
        method::RUN,
        to_payload(&trcode_test_protocol::RunArgs {
            run_id: 2,
            ids: Some(vec!["a::b t".into(), "c x y".into()]),
        }),
    )));

    let commands: Vec<(&String, &Vec<String>)> = asked
        .iter()
        .filter_map(|message| match message {
            FromPlugin::Run { program, args, .. } => Some((program, args)),
            _ => None,
        })
        .collect();
    assert_eq!(commands.len(), 3);
    for (program, args) in commands {
        assert!(
            trcode_plugin_api::allows(&patterns, program, args),
            "{program} {args:?}"
        );
    }
    // And nothing else: cargo install is not on the list.
    let install = vec!["install".to_owned(), "x".to_owned()];
    assert!(!trcode_plugin_api::allows(&patterns, "cargo", &install));
    assert!(!trcode_plugin_api::allows(
        &patterns,
        "sh",
        &["-c".to_owned()]
    ));
    // The manifest is what a pane-less adapter that runs commands is.
    assert!(!manifest.view);
    assert_eq!(manifest.thread, trcode_plugin_api::Thread::Own);
    assert_eq!(manifest.capabilities, ["plugin.serve", "process.run"]);
    assert_eq!(Abi::CURRENT.check(manifest.abi.parse().unwrap()), Ok(()));
}
