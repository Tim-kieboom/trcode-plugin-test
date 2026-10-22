//! The adapter against nextest itself, on the project in `tests/fixture-project`: what the
//! adapter asks the editor to run is run, and what it writes is given back, so the commands
//! and the parsers are tried on the real thing. Skipped when nextest is not installed.

use std::path::PathBuf;
use std::process::Command;

use trcode_plugin_api::{FromPlugin, Plugin, RunStream, ToPlugin};
use trcode_test_protocol::{
    DiscoverArgs, Page, RunArgs, RunEvent, Status, from_payload, method, to_payload,
};

use crate::RustAdapter;

fn nextest_is_installed() -> bool {
    Command::new("cargo")
        .args(["nextest", "--version"])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture-project")
}

/// What a command the adapter asked for wrote, and how it ended.
struct Ran {
    command: u64,
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

/// Runs the command a `Run` message asks for, in the fixture project, with a target folder of
/// its own so that the repository is left as it is.
fn execute(said: &[FromPlugin], target: &std::path::Path) -> Ran {
    let [
        FromPlugin::Run {
            request,
            program,
            args,
            cwd,
        },
    ] = said
    else {
        panic!("not one command: {said:?}");
    };
    assert_eq!(cwd, "");
    let output = Command::new(program)
        .args(args)
        .current_dir(project())
        .env("CARGO_TARGET_DIR", target)
        .output()
        .expect("cargo starts");
    Ran {
        command: *request,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code(),
    }
}

/// Gives the adapter what the command wrote, in pieces of 4096 bytes as a pipe gives it.
fn feed(adapter: &mut RustAdapter, ran: &Ran) -> Vec<FromPlugin> {
    let mut said = Vec::new();
    for (stream, text) in [
        (RunStream::Stdout, &ran.stdout),
        (RunStream::Stderr, &ran.stderr),
    ] {
        let chars: Vec<char> = text.chars().collect();
        for piece in chars.chunks(4096) {
            said.extend(adapter.handle(ToPlugin::RunOutput {
                request: ran.command,
                stream,
                text: piece.iter().collect(),
            }));
        }
    }
    said.extend(adapter.handle(ToPlugin::RunDone {
        request: ran.command,
        code: ran.code,
        cancelled: false,
        truncated: false,
    }));
    said
}

fn call(adapter: &mut RustAdapter, request: u64, name: &str, payload: String) -> Vec<FromPlugin> {
    adapter.handle(ToPlugin::Call {
        from: "pane".into(),
        request,
        method: name.into(),
        payload,
    })
}

fn payloads(said: &[FromPlugin]) -> Vec<String> {
    said.iter()
        .filter_map(|message| match message {
            FromPlugin::Reply { payload, .. } => Some(payload.clone()),
            _ => None,
        })
        .collect()
}

fn events(said: &[FromPlugin]) -> Vec<RunEvent> {
    payloads(said)
        .iter()
        .map(|payload| from_payload(payload).expect("an event"))
        .collect()
}

fn finished(events: &[RunEvent]) -> Vec<(String, Status)> {
    let mut found: Vec<(String, Status)> = events
        .iter()
        .filter_map(|event| match event {
            RunEvent::Finished { id, status, .. } => Some((id.clone(), *status)),
            _ => None,
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

#[test]
fn the_adapter_lists_and_runs_a_real_project_with_nextest() {
    if !nextest_is_installed() {
        eprintln!("cargo-nextest is not installed: skipped");
        return;
    }
    let target = tempfile::tempdir().unwrap();
    let mut adapter = RustAdapter::default();

    // Discover: the command it asks for, run for real.
    let args = DiscoverArgs {
        offset: 0,
        limit: 100,
    };
    let said = call(&mut adapter, 1, method::DISCOVER, to_payload(&args));
    let said = feed(&mut adapter, &execute(&said, target.path()));
    let page: Page = from_payload(&payloads(&said)[0]).unwrap();
    assert_eq!(page.error, None);
    let mut ids: Vec<&str> = page.tests.iter().map(|test| test.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "fx tests::fails_here",
            "fx tests::panics_ok",
            "fx tests::parses_empty",
            "fx::bin/fx tests::parses_empty",
            "fx::it parses_empty",
        ],
        "the ignored test is not listed"
    );

    // Run everything.
    let run = RunArgs {
        run_id: 1,
        ids: None,
    };
    let said = call(&mut adapter, 2, method::RUN, to_payload(&run));
    let said = feed(&mut adapter, &execute(&said, target.path()));
    let all = events(&said);
    assert_eq!(
        finished(&all),
        [
            ("fx tests::fails_here".to_owned(), Status::Failed),
            ("fx tests::panics_ok".to_owned(), Status::Passed),
            ("fx tests::parses_empty".to_owned(), Status::Passed),
            ("fx::bin/fx tests::parses_empty".to_owned(), Status::Passed),
            ("fx::it parses_empty".to_owned(), Status::Passed),
        ]
    );
    assert_eq!(all.last(), Some(&RunEvent::Done { run_id: 1 }));
    assert!(
        !all.iter()
            .any(|event| matches!(event, RunEvent::Error { .. }))
    );
    let message = all.iter().find_map(|event| match event {
        RunEvent::Finished {
            status: Status::Failed,
            message,
            ..
        } => Some(message.clone()),
        _ => None,
    });
    assert!(message.unwrap().contains("left: 2"));

    // Run one test, the one of the binary that shares its name with two others.
    let run = RunArgs {
        run_id: 2,
        ids: Some(vec!["fx::bin/fx tests::parses_empty".to_owned()]),
    };
    let said = call(&mut adapter, 3, method::RUN, to_payload(&run));
    let said = feed(&mut adapter, &execute(&said, target.path()));
    assert_eq!(
        finished(&events(&said)),
        [("fx::bin/fx tests::parses_empty".to_owned(), Status::Passed)],
        "only that one ran"
    );

    // Run two of a binary.
    let run = RunArgs {
        run_id: 3,
        ids: Some(vec![
            "fx tests::fails_here".to_owned(),
            "fx tests::parses_empty".to_owned(),
        ]),
    };
    let said = call(&mut adapter, 4, method::RUN, to_payload(&run));
    let said = feed(&mut adapter, &execute(&said, target.path()));
    assert_eq!(
        finished(&events(&said)),
        [
            ("fx tests::fails_here".to_owned(), Status::Failed),
            ("fx tests::parses_empty".to_owned(), Status::Passed),
        ]
    );
}
