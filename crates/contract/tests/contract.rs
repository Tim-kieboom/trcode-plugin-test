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
    // The pane starts with the Rust adapter and never falls back to the fake one: it is chosen.
    host.send(UI, ToPlugin::action("use_fake"));
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
    // Two folders, each with a test of the same name.
    assert_eq!(
        host.lines(UI)[1..],
        [
            "v [  ] bin0 (0/2)",
            "  [  ] case_0",
            "  [  ] case_1",
            "v [  ] bin1 (0/2)",
            "  [  ] case_0",
            "  [  ] case_1",
        ]
    );

    // The first case_0 of bin1: the folder, two tests, the next folder, then it.
    for _ in 0..4 {
        press(&mut host, "down");
    }
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
        location: None,
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
    // The fresh pane starts with the Rust adapter, as any pane: the fake one is chosen again.
    host.send(UI, ToPlugin::action("use_fake"));
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
    host.run_until_quiet();
    host.tell(UI, ToPlugin::action("use_fake"));

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
    // A project this big starts with its folders closed.
    assert_eq!(host.lines(UI)[1..], ["> [  ] bin (0/20000)"]);

    press(&mut host, "right");
    let lines = host.lines(UI).len();
    for _ in 0..100 {
        press(&mut host, "down");
    }

    assert_eq!(
        lines, 19,
        "a pane of 20: the buttons, the header, the folder and what fits of its tests"
    );
    assert_eq!(host.lines(UI).len(), 19, "still a window after scrolling");
    host.send(
        UI,
        ToPlugin::Size {
            width: 80,
            height: 10,
        },
    );
    assert_eq!(
        host.lines(UI).len(),
        9,
        "a row of buttons takes one of the ten"
    );
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

// --- the buttons ---

#[test]
fn find_tests_and_the_run_list_are_buttons_of_the_pane_that_do_what_they_say() {
    let mut host = started(&Scenario {
        tests: 4,
        ..Scenario::default()
    });
    assert_eq!(
        host.buttons(UI),
        [vec![
            ("find".to_owned(), "Find tests".to_owned(), true),
            ("run_menu".to_owned(), "Run v".to_owned(), true),
        ]]
    );

    // Open the list and choose Run all: the four tests run.
    host.send(
        UI,
        ToPlugin::Press {
            button: "run_menu".into(),
        },
    );
    assert_eq!(host.buttons(UI).len(), 2);
    host.send(
        UI,
        ToPlugin::Press {
            button: "run_all".into(),
        },
    );
    assert_eq!(host.buttons(UI).len(), 1, "the list closes");
    advance(&mut host, 10);
    assert_eq!(ui(&host).counts().of(RowStatus::Passed), 3);
    assert_eq!(ui(&host).counts().of(RowStatus::Failed), 1);

    // Find tests asks the adapter again: the results are gone, the tests are the same.
    host.send(
        UI,
        ToPlugin::Press {
            button: "find".into(),
        },
    );
    assert_eq!(ui(&host).discovered(), (4, 4));
    assert_eq!(ui(&host).counts().of(RowStatus::Passed), 0);
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
    assert_eq!(
        pane.capabilities,
        ["plugin.call", "editor.open", "debug.start"]
    );
    for name in adapter.capabilities.iter().chain(&pane.capabilities) {
        assert!(Capability::parse(name).is_some(), "{name}");
    }
    let ids: Vec<&str> = pane
        .actions
        .iter()
        .map(|action| action.id.as_str())
        .collect();
    for action in [
        "run_all",
        "run_selected",
        "cancel",
        "up",
        "down",
        "left",
        "right",
        "goto",
        "goto_failure",
        "show_error",
        "debug_selected",
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
    host.add::<TestUi>(UI, &[Capability::PluginCall, Capability::EditorOpen]);
    host.run_until_quiet();

    // The pane tries the Rust adapter first, which is here.
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

/// The pane, the Rust adapter and the project in `fixture-project`, with everything run.
#[cfg(unix)]
fn run_fixture() -> Option<(MiniHost, tempfile::TempDir)> {
    if !nextest_is_installed() {
        eprintln!("cargo-nextest is not installed: skipped");
        return None;
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
    host.add::<TestUi>(
        UI,
        &[
            Capability::PluginCall,
            Capability::EditorOpen,
            Capability::DebugStart,
        ],
    );
    host.run_until_quiet();
    press(&mut host, "run_all");
    Some((host, target))
}

/// The rows (lines of the view) that show a test called `name`.
#[cfg(unix)]
fn rows_of(host: &MiniHost, name: &str) -> Vec<usize> {
    let Some(trcode_plugin_api::WireView::Column { parts }) = host.view(UI) else {
        panic!("not a column");
    };
    parts
        .iter()
        .find_map(|part| match part {
            trcode_plugin_api::WireView::Rows { lines, .. } => Some(
                lines
                    .iter()
                    .enumerate()
                    .filter(|(_, line)| line.text.trim_end().ends_with(&format!("] {name}")))
                    .map(|(row, _)| row)
                    .collect(),
            ),
            _ => None,
        })
        .expect("rows")
}

#[cfg(unix)]
#[test]
fn a_click_on_a_failed_test_of_a_real_project_opens_the_line_its_panic_names() {
    let Some((mut host, _target)) = run_fixture() else {
        return;
    };

    let row = rows_of(&host, "fails_here")[0];
    host.send(UI, ToPlugin::Click { row });

    // `assert_eq!` in `src/lib.rs` is at line 14, column 9.
    assert_eq!(
        host.opened(),
        [("src/lib.rs".to_owned(), Some(14), Some(9))],
        "{}",
        ui(&host).notice()
    );
}

#[cfg(unix)]
#[test]
fn a_click_on_a_test_that_passed_opens_its_fn_found_with_grep_in_the_file_of_its_binary() {
    let Some((mut host, _target)) = run_fixture() else {
        return;
    };

    // `parses_empty` is in the library, in the binary and in the integration test.
    let rows = rows_of(&host, "parses_empty");
    assert_eq!(rows.len(), 3, "{:?}", host.view(UI));
    for row in rows {
        host.send(UI, ToPlugin::Click { row });
    }

    let mut found: Vec<(String, Option<u32>)> = host
        .opened()
        .iter()
        .map(|(path, line, _)| (path.clone(), *line))
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            ("src/lib.rs".to_owned(), Some(8)),
            ("src/main.rs".to_owned(), Some(6)),
            ("tests/it.rs".to_owned(), Some(2)),
        ],
        "{}",
        ui(&host).notice()
    );
}

#[cfg(unix)]
#[test]
fn the_menu_of_a_failed_test_of_a_real_project_has_its_error_and_the_popup_shows_the_panic() {
    let Some((mut host, _target)) = run_fixture() else {
        return;
    };
    let row = rows_of(&host, "fails_here")[0];

    host.send(UI, ToPlugin::ContextClick { row });
    host.run_until_quiet();

    let menu = host.menus().last().expect("a menu");
    assert!(
        menu.contains(&"show_error".to_owned()) && menu.contains(&"debug_selected".to_owned()),
        "{menu:?}"
    );
    press(&mut host, "show_error");
    let (title, text) = host.popups().last().expect("a popup");
    assert_eq!(title, "tests::fails_here");
    assert!(text.contains("assertion"), "{text}");
    assert!(text.ends_with("at src/lib.rs:14"), "{text}");
}

#[cfg(unix)]
#[test]
fn a_test_of_a_real_project_is_built_for_the_debugger_and_what_the_editor_is_told_to_start_runs_it()
{
    let Some((mut host, _target)) = run_fixture() else {
        return;
    };
    let rows = rows_of(&host, "panics_ok");
    assert_eq!(rows.len(), 1, "{:?}", host.view(UI));
    host.send(UI, ToPlugin::Click { row: rows[0] });
    host.run_until_quiet();

    press(&mut host, "debug_selected");

    let asked = host
        .debugs()
        .last()
        .unwrap_or_else(|| panic!("{}", ui(&host).notice()));
    let program = std::path::Path::new(&asked.program);
    assert!(program.is_file(), "{}", asked.program);
    assert_eq!(asked.args, ["--exact", "tests::panics_ok", "--nocapture"]);
    // The folder of the package, whole, as nextest says it.
    assert!(
        asked.cwd.ends_with("rust-adapter/tests/fixture-project"),
        "{}",
        asked.cwd
    );
    // What the editor is told to start is the test, and only that one.
    let mut command = std::process::Command::new(program);
    command.args(&asked.args).current_dir(&asked.cwd);
    for pair in &asked.env {
        let (name, value) = pair.split_once('=').unwrap();
        command.env(name, value);
    }
    let output = command.output().unwrap();
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{said}");
    assert!(said.contains("1 passed"), "{said}");
    assert_eq!(ui(&host).notice(), "", "the editor said it started");
}

#[cfg(unix)]
#[test]
fn a_project_that_does_not_build_says_so_in_the_pane_and_show_problem_has_the_compilers_errors() {
    if !nextest_is_installed() {
        eprintln!("cargo-nextest is not installed: skipped");
        return;
    }
    let project = tempfile::tempdir().unwrap();
    let from = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rust-adapter/tests/fixture-project");
    for file in [
        "Cargo.toml",
        "Cargo.lock",
        "src/lib.rs",
        "src/main.rs",
        "tests/it.rs",
    ] {
        let to = project.path().join(file);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(from.join(file), to).unwrap();
    }
    let lib = project.path().join("src/lib.rs");
    let broken =
        std::fs::read_to_string(&lib).unwrap() + "\nfn broken() { let _ = missing_name; }\n";
    std::fs::write(&lib, broken).unwrap();
    let target = tempfile::tempdir().unwrap();
    let manifest = manifest("rust-adapter/manifest.toml");
    let mut host = MiniHost::new();
    host.run_commands_in(project.path().to_path_buf(), target.path().to_path_buf());
    host.add::<trcode_test_rust_adapter::RustAdapter>(
        "rust-adapter",
        &[Capability::PluginServe, Capability::ProcessRun],
    );
    host.allow_commands(
        "rust-adapter",
        trcode_plugin_api::parse_all(&manifest.run).unwrap(),
    );
    host.add::<TestUi>(UI, &[Capability::PluginCall, Capability::EditorOpen]);

    host.run_until_quiet();

    let lines = host.lines(UI);
    assert!(
        lines[0].contains("| the project does not build (cargo nextest list ended with"),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("missing_name")),
        "{lines:?}"
    );
    press(&mut host, "show_problem");
    let (_, text) = host.popups().last().expect("a popup");
    assert!(text.contains("cannot find value `missing_name`"), "{text}");
}
