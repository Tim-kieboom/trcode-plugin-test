use trcode_plugin_api::{FromPlugin, Plugin, Refusal, RunStream, ToPlugin};
use trcode_test_protocol::{
    DiscoverArgs, Page, RunArgs, RunEvent, TestItem, from_payload, method, to_payload,
};

use crate::filter;
use crate::list::ListScanner;
use crate::output::{Outcome, RunParser, cut};

/// How much of what a command wrote to its standard error is kept, to say what went wrong.
const TAIL: usize = 1_500;

/// Someone waiting for the first page of the list of tests while it is being made.
#[derive(Debug)]
struct Waiter {
    from: String,
    request: u64,
    limit: usize,
}

/// The list of tests being made by `cargo nextest list`.
#[derive(Debug)]
struct Listing {
    command: u64,
    scanner: ListScanner,
    tail: String,
    waiters: Vec<Waiter>,
}

/// A run being made by `cargo nextest run`: who asked, and what it has told so far.
#[derive(Debug)]
struct ActiveRun {
    from: String,
    request: u64,
    run_id: u64,
    command: u64,
    parser: RunParser,
    tail: String,
    results: usize,
}

#[derive(Debug, Default)]
pub struct RustAdapter {
    /// The number the next command is given.
    commands: u64,
    listing: Option<Listing>,
    /// The tests, as the last list made them.
    tests: Vec<TestItem>,
    runs: Vec<ActiveRun>,
}

impl Plugin for RustAdapter {
    fn handle(&mut self, message: ToPlugin) -> Vec<FromPlugin> {
        match message {
            ToPlugin::Call {
                from,
                request,
                method,
                payload,
            } => self.call(&from, request, &method, &payload),
            ToPlugin::CallCancelled { from, request } => self.cancelled(&from, request),
            ToPlugin::RunOutput {
                request,
                stream,
                text,
            } => self.output(request, stream, &text),
            ToPlugin::RunDone {
                request,
                code,
                cancelled,
                truncated,
            } => self.done(request, code, cancelled, truncated),
            ToPlugin::Refused { request, reason } => self.refused(request, &reason),
            _ => Vec::new(),
        }
    }
}

impl RustAdapter {
    fn call(&mut self, from: &str, request: u64, name: &str, payload: &str) -> Vec<FromPlugin> {
        match name {
            method::DISCOVER => self.discover(from, request, payload),
            method::RUN => self.run(from, request, payload),
            // Methods of the fake adapter, and ones that are not known: answered, so that a
            // call is never left open.
            _ => vec![reply(from, request, "{}".to_owned(), true)],
        }
    }

    // --- discovery ---

    /// The first page makes the list (again); the pages after it are of the list that was made.
    fn discover(&mut self, from: &str, request: u64, payload: &str) -> Vec<FromPlugin> {
        let args = from_payload(payload).unwrap_or_else(|| DiscoverArgs::page_at(0));
        if args.offset > 0 {
            return vec![self.page_reply(from, request, args.offset, args.limit)];
        }
        let waiter = Waiter {
            from: from.to_owned(),
            request,
            limit: args.limit,
        };
        if let Some(listing) = &mut self.listing {
            listing.waiters.push(waiter);
            return Vec::new();
        }
        let command = self.next_command();
        self.listing = Some(Listing {
            command,
            scanner: ListScanner::default(),
            tail: String::new(),
            waiters: vec![waiter],
        });
        let args = [
            "nextest",
            "list",
            "--message-format",
            "json",
            "--color",
            "never",
        ];
        vec![start(command, &args, None)]
    }

    fn page_reply(&self, to: &str, request: u64, offset: usize, limit: usize) -> FromPlugin {
        let total = self.tests.len();
        let start = offset.min(total);
        let end = start.saturating_add(limit).min(total);
        let page = Page {
            offset: start,
            total,
            tests: self.tests[start..end].to_vec(),
            error: None,
        };
        reply(to, request, to_payload(&page), true)
    }

    /// The list is over: everyone who waited for it gets the first page, or why there is none.
    fn listed(&mut self, code: Option<i32>, truncated: bool) -> Vec<FromPlugin> {
        let Some(listing) = self.listing.take() else {
            return Vec::new();
        };
        let outcome = match (code, truncated) {
            (_, true) => Err("the list of tests is too long to read".to_owned()),
            (Some(0), false) => listing.scanner.finish(),
            _ => Err(failure("cargo nextest list", code, &listing.tail)),
        };
        let mut said = Vec::new();
        match outcome {
            Ok(tests) => {
                self.tests = tests;
                for waiter in &listing.waiters {
                    said.push(self.page_reply(&waiter.from, waiter.request, 0, waiter.limit));
                }
            }
            Err(message) => {
                self.tests.clear();
                for waiter in &listing.waiters {
                    let page = to_payload(&Page::failed(message.clone()));
                    said.push(reply(&waiter.from, waiter.request, page, true));
                }
            }
        }
        said
    }

    // --- running ---

    fn run(&mut self, from: &str, request: u64, payload: &str) -> Vec<FromPlugin> {
        let Some(args) = from_payload::<RunArgs>(payload) else {
            return vec![reply(from, request, String::new(), true)];
        };
        let filter = match &args.ids {
            None => None,
            Some(ids) => match filter::expression(ids) {
                Some(expression) => Some(expression),
                None => {
                    let message = "none of the tests asked for is one this adapter listed";
                    return finished_with_error(from, request, args.run_id, message);
                }
            },
        };
        let command = self.next_command();
        self.runs.push(ActiveRun {
            from: from.to_owned(),
            request,
            run_id: args.run_id,
            command,
            parser: RunParser::default(),
            tail: String::new(),
            results: 0,
        });
        let base = ["nextest", "run", "--no-fail-fast", "--color", "never"];
        vec![start(command, &base, filter.as_deref())]
    }

    /// What nextest wrote: a run is told as the tests end; the list is read as it comes.
    fn output(&mut self, command: u64, stream: RunStream, text: &str) -> Vec<FromPlugin> {
        if let Some(listing) = self.listing.as_mut().filter(|l| l.command == command) {
            match stream {
                RunStream::Stdout => listing.scanner.feed(text),
                _ => keep_tail(&mut listing.tail, text),
            }
            return Vec::new();
        }
        let Some(run) = self.runs.iter_mut().find(|run| run.command == command) else {
            return Vec::new();
        };
        if stream != RunStream::Stderr {
            return Vec::new();
        }
        keep_tail(&mut run.tail, text);
        let ended = run.parser.feed(text);
        run.results += ended.len();
        events(run, &ended)
    }

    fn done(
        &mut self,
        command: u64,
        code: Option<i32>,
        cancelled: bool,
        truncated: bool,
    ) -> Vec<FromPlugin> {
        if self.listing.as_ref().is_some_and(|l| l.command == command) {
            return self.listed(code, truncated && !cancelled);
        }
        let Some(at) = self.runs.iter().position(|run| run.command == command) else {
            return Vec::new();
        };
        let mut run = self.runs.remove(at);
        let ended = run.parser.finish();
        run.results += ended.len();
        let mut said = events(&run, &ended);
        // 0: all passed. 100: some failed. Anything else with nothing run is why it did not.
        if run.results == 0 && !matches!(code, Some(0 | 100)) {
            let message = match code {
                Some(4) => "no test matched".to_owned(),
                _ => failure("cargo nextest run", code, &run.tail),
            };
            said.push(event(
                &run,
                &RunEvent::Error {
                    run_id: run.run_id,
                    message,
                },
                false,
            ));
        }
        said.push(event(&run, &RunEvent::Done { run_id: run.run_id }, true));
        said
    }

    /// The editor would not run a command: the listing or the run says why.
    fn refused(&mut self, command: u64, reason: &Refusal) -> Vec<FromPlugin> {
        let message = refusal(reason);
        if self.listing.as_ref().is_some_and(|l| l.command == command) {
            let Some(listing) = self.listing.take() else {
                return Vec::new();
            };
            let page = to_payload(&Page::failed(message));
            return listing
                .waiters
                .iter()
                .map(|waiter| reply(&waiter.from, waiter.request, page.clone(), true))
                .collect();
        }
        let Some(at) = self.runs.iter().position(|run| run.command == command) else {
            return Vec::new();
        };
        let run = self.runs.remove(at);
        vec![
            event(
                &run,
                &RunEvent::Error {
                    run_id: run.run_id,
                    message,
                },
                false,
            ),
            event(&run, &RunEvent::Done { run_id: run.run_id }, true),
        ]
    }

    /// The caller gave a call up: what it asked for stops, so no more output is read for it.
    fn cancelled(&mut self, from: &str, request: u64) -> Vec<FromPlugin> {
        let mut said = Vec::new();
        if let Some(at) = self
            .runs
            .iter()
            .position(|run| run.from == from && run.request == request)
        {
            let run = self.runs.remove(at);
            said.push(FromPlugin::RunCancel {
                request: run.command,
            });
        }
        if let Some(listing) = &mut self.listing {
            listing
                .waiters
                .retain(|waiter| !(waiter.from == from && waiter.request == request));
            if listing.waiters.is_empty() {
                said.push(FromPlugin::RunCancel {
                    request: listing.command,
                });
                self.listing = None;
            }
        }
        said
    }

    fn next_command(&mut self) -> u64 {
        self.commands += 1;
        self.commands
    }
}

/// The command to run: `cargo` with `args`, and `-E filter` when there is one, in the project.
fn start(command: u64, args: &[&str], filter: Option<&str>) -> FromPlugin {
    let mut args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    if let Some(filter) = filter {
        args.push("-E".to_owned());
        args.push(filter.to_owned());
    }
    FromPlugin::Run {
        request: command,
        program: "cargo".to_owned(),
        args,
        cwd: String::new(),
    }
}

/// The events for tests that ended: each starts and ends.
fn events(run: &ActiveRun, ended: &[Outcome]) -> Vec<FromPlugin> {
    let mut said = Vec::with_capacity(ended.len() * 2);
    for outcome in ended {
        let id = format!("{} {}", outcome.binary_id, outcome.name);
        let started = RunEvent::Started {
            run_id: run.run_id,
            id: id.clone(),
        };
        said.push(event(run, &started, false));
        let finished = RunEvent::Finished {
            run_id: run.run_id,
            id,
            status: outcome.status,
            duration_ms: outcome.duration_ms,
            message: outcome.message.clone(),
        };
        said.push(event(run, &finished, false));
    }
    said
}

fn event(run: &ActiveRun, event: &RunEvent, last: bool) -> FromPlugin {
    reply(&run.from, run.request, to_payload(event), last)
}

/// A run that cannot be made: said, and over.
fn finished_with_error(from: &str, request: u64, run_id: u64, message: &str) -> Vec<FromPlugin> {
    let error = RunEvent::Error {
        run_id,
        message: message.to_owned(),
    };
    vec![
        reply(from, request, to_payload(&error), false),
        reply(from, request, to_payload(&RunEvent::Done { run_id }), true),
    ]
}

fn reply(to: &str, request: u64, payload: String, last: bool) -> FromPlugin {
    FromPlugin::Reply {
        to: to.to_owned(),
        request,
        payload,
        last,
    }
}

/// Keeps the last `TAIL` characters of what a command wrote.
fn keep_tail(tail: &mut String, text: &str) {
    tail.push_str(text);
    let extra = tail.chars().count().saturating_sub(TAIL);
    if extra > 0 {
        let at = tail
            .char_indices()
            .nth(extra)
            .map_or(tail.len(), |(at, _)| at);
        tail.drain(..at);
    }
}

/// What to tell the user of a command that did not work: how it ended and the end of what it said.
fn failure(command: &str, code: Option<i32>, tail: &str) -> String {
    let how = match code {
        Some(code) => format!("{command} ended with {code}"),
        None => format!("{command} was stopped"),
    };
    match tail.trim() {
        "" => how,
        tail => format!("{how}: {}", cut(tail, TAIL)),
    }
}

/// Why the editor did not run a command, for the user.
fn refusal(reason: &Refusal) -> String {
    match reason {
        Refusal::Untrusted => {
            "the folder is not trusted, so no command may run in it (Plugins: Trust This Folder)"
                .to_owned()
        }
        Refusal::NotAllowed { detail } => detail.clone(),
        Refusal::Failed { detail } => format!("cargo could not be started: {detail}"),
        Refusal::Busy => "too many commands are running".to_owned(),
        Refusal::Denied { capability } => format!("the editor denied {}", capability.name()),
        other => format!("the editor refused: {other:?}"),
    }
}
