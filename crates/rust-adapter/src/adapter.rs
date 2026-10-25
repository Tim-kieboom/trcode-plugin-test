use trcode_plugin_api::{FromPlugin, Plugin, Refusal, RunStream, ToPlugin};
use trcode_test_protocol::{
    DebugArgs, DebugReply, DiscoverArgs, LocateArgs, LocateReply, Page, RunArgs, RunEvent, Table,
    from_payload, method, to_payload,
};

use crate::filter;
use crate::list::{ListScanner, Suite};
use crate::locate::{Locator, panic_location};
use crate::output::{Outcome, RunParser, cut};

/// How much of what a command wrote to its standard error is kept, to say what went wrong.
const TAIL: usize = 1_500;

/// How much of what the build of the tests wrote is kept, for the user to read in a popup: the
/// errors of the compiler are above the line that says it could not compile.
const BUILD_TAIL: usize = 6_000;

/// Someone waiting for the first page of the list of tests while it is being made.
#[derive(Debug)]
struct Waiter {
    from: String,
    request: u64,
    limit: usize,
}

/// Someone waiting for a test to be built, to be debugged: the list that builds it is made
/// again, since the code may have changed since the last.
#[derive(Debug)]
struct Debugger {
    from: String,
    request: u64,
    id: String,
}

/// The list of tests being made by `cargo nextest list`.
#[derive(Debug)]
struct Listing {
    command: u64,
    scanner: ListScanner,
    tail: String,
    waiters: Vec<Waiter>,
    debuggers: Vec<Debugger>,
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

/// A search for the code of a test, by `grep`: who asked, and what it has found so far.
#[derive(Debug)]
struct ActiveLocate {
    from: String,
    request: u64,
    command: u64,
    locator: Locator,
}

#[derive(Debug, Default)]
pub struct RustAdapter {
    /// The number the next command is given.
    commands: u64,
    listing: Option<Listing>,
    /// The tests, as the last list made them, and the binaries they are in.
    tests: Table,
    suites: Vec<Suite>,
    runs: Vec<ActiveRun>,
    locates: Vec<ActiveLocate>,
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
            method::LOCATE => self.locate(from, request, payload),
            method::DEBUG => self.debug(from, request, payload),
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
        self.list(vec![waiter], Vec::new())
    }

    /// Starts `cargo nextest list`, which builds the tests, for those who wait for it.
    fn list(&mut self, waiters: Vec<Waiter>, debuggers: Vec<Debugger>) -> Vec<FromPlugin> {
        let command = self.next_command();
        self.listing = Some(Listing {
            command,
            scanner: ListScanner::default(),
            tail: String::new(),
            waiters,
            debuggers,
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
            tests: self.tests.items(start, end - start),
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
            (Some(0), false) => listing.scanner.finish_with_binaries(),
            _ => Err(failure("cargo nextest list", code, &listing.tail)),
        };
        let mut said = Vec::new();
        match outcome {
            Ok((tests, suites)) => {
                self.tests = tests;
                self.suites = suites;
                for waiter in &listing.waiters {
                    said.push(self.page_reply(&waiter.from, waiter.request, 0, waiter.limit));
                }
                for debugger in &listing.debuggers {
                    let answer = self.debug_answer(&debugger.id);
                    said.push(reply(
                        &debugger.from,
                        debugger.request,
                        to_payload(&answer),
                        true,
                    ));
                }
            }
            Err(message) => {
                self.tests.clear();
                self.suites.clear();
                for waiter in &listing.waiters {
                    let page = to_payload(&Page::failed(message.clone()));
                    said.push(reply(&waiter.from, waiter.request, page, true));
                }
                for debugger in &listing.debuggers {
                    let answer = to_payload(&DebugReply::failed(message.clone()));
                    said.push(reply(&debugger.from, debugger.request, answer, true));
                }
            }
        }
        said
    }

    // --- finding the code of a test ---

    /// Looks for `fn name` in the project with `grep`; the test is known by what the last list
    /// made of it.
    fn locate(&mut self, from: &str, request: u64, payload: &str) -> Vec<FromPlugin> {
        let none = |error: Option<String>| {
            let answer = LocateReply {
                location: None,
                error,
            };
            vec![reply(from, request, to_payload(&answer), true)]
        };
        let Some(args) = from_payload::<LocateArgs>(payload) else {
            return none(None);
        };
        let Some(row) = self.tests.find(&args.id) else {
            return none(None);
        };
        let name = self.tests.name(row).to_owned();
        let leaf = name.rsplit("::").next().unwrap_or(&name);
        let command = self.next_command();
        self.locates.push(ActiveLocate {
            from: from.to_owned(),
            request,
            command,
            locator: Locator::new(&args.id),
        });
        let args = [
            "-rnwF",
            "-I",
            "--exclude-dir=target",
            "--exclude-dir=.git",
            "-e",
            &format!("fn {leaf}"),
            ".",
        ];
        vec![FromPlugin::Run {
            request: command,
            program: "grep".to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd: String::new(),
        }]
    }

    /// The search is over: what it found, or why it could not look.
    fn located(&mut self, at: usize, code: Option<i32>) -> Vec<FromPlugin> {
        let search = self.locates.remove(at);
        let location = search.locator.finish();
        // grep: 0 found, 1 found nothing, 2 trouble (it may still have found some).
        let error = match (&location, code) {
            (None, Some(0 | 1)) => None,
            (None, _) => Some("the search for the test with grep did not work".to_owned()),
            (Some(_), _) => None,
        };
        let answer = LocateReply { location, error };
        vec![reply(
            &search.from,
            search.request,
            to_payload(&answer),
            true,
        )]
    }

    // --- debugging ---

    /// Builds the tests (the list does) and then says how to start the one asked for, which
    /// the editor starts under the debugger.
    fn debug(&mut self, from: &str, request: u64, payload: &str) -> Vec<FromPlugin> {
        let Some(args) = from_payload::<DebugArgs>(payload) else {
            let answer = DebugReply::failed("the adapter was asked to debug no test");
            return vec![reply(from, request, to_payload(&answer), true)];
        };
        let debugger = Debugger {
            from: from.to_owned(),
            request,
            id: args.id,
        };
        if let Some(listing) = &mut self.listing {
            listing.debuggers.push(debugger);
            return Vec::new();
        }
        self.list(Vec::new(), vec![debugger])
    }

    /// How to start the test `id` (a binary id, a space and the name): its binary, run for that
    /// test only, in the folder of its package and with what cargo gives a test.
    fn debug_answer(&self, id: &str) -> DebugReply {
        let Some((binary, name)) = id.split_once(' ') else {
            return DebugReply::failed("that is not a test of this adapter");
        };
        let Some(suite) = self.suites.iter().find(|suite| suite.binary_id == binary) else {
            return DebugReply::failed(format!("the binary {binary} is not in the list of tests"));
        };
        if suite.binary_path.is_empty() {
            return DebugReply::failed(format!("nextest did not say where {binary} is"));
        }
        let mut env = Vec::new();
        if !suite.cwd.is_empty() {
            env.push(format!("CARGO_MANIFEST_DIR={}", suite.cwd));
        }
        if !suite.package.is_empty() {
            env.push(format!("CARGO_PKG_NAME={}", suite.package));
        }
        DebugReply {
            program: suite.binary_path.clone(),
            args: ["--exact", name, "--nocapture"]
                .iter()
                .map(|arg| (*arg).to_owned())
                .collect(),
            env,
            cwd: suite.cwd.clone(),
            error: None,
        }
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
        if let Some(search) = self.locates.iter_mut().find(|l| l.command == command) {
            if stream == RunStream::Stdout {
                search.locator.feed(text);
            }
            return Vec::new();
        }
        if let Some(listing) = self.listing.as_mut().filter(|l| l.command == command) {
            match stream {
                RunStream::Stdout => listing.scanner.feed(text),
                _ => keep_tail(&mut listing.tail, text, BUILD_TAIL),
            }
            return Vec::new();
        }
        let Some(run) = self.runs.iter_mut().find(|run| run.command == command) else {
            return Vec::new();
        };
        if stream != RunStream::Stderr {
            return Vec::new();
        }
        keep_tail(&mut run.tail, text, TAIL);
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
        if let Some(at) = self.locates.iter().position(|l| l.command == command) {
            return self.located(at, code);
        }
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
        if let Some(at) = self.locates.iter().position(|l| l.command == command) {
            let search = self.locates.remove(at);
            let answer = LocateReply {
                location: None,
                error: Some(match reason {
                    Refusal::Failed { .. } => {
                        "searching the code needs grep, which is not here".to_owned()
                    }
                    _ => message,
                }),
            };
            return vec![reply(
                &search.from,
                search.request,
                to_payload(&answer),
                true,
            )];
        }
        if self.listing.as_ref().is_some_and(|l| l.command == command) {
            let Some(listing) = self.listing.take() else {
                return Vec::new();
            };
            let page = to_payload(&Page::failed(message.clone()));
            let failed = to_payload(&DebugReply::failed(message));
            let waiting = listing
                .waiters
                .iter()
                .map(|waiter| reply(&waiter.from, waiter.request, page.clone(), true));
            let debugging = listing
                .debuggers
                .iter()
                .map(|debugger| reply(&debugger.from, debugger.request, failed.clone(), true));
            return waiting.chain(debugging).collect();
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
            .locates
            .iter()
            .position(|search| search.from == from && search.request == request)
        {
            let search = self.locates.remove(at);
            said.push(FromPlugin::RunCancel {
                request: search.command,
            });
        }
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
            listing
                .debuggers
                .retain(|debugger| !(debugger.from == from && debugger.request == request));
            if listing.waiters.is_empty() && listing.debuggers.is_empty() {
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
            location: panic_location(&outcome.message),
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

/// Keeps the last `most` characters of what a command wrote.
fn keep_tail(tail: &mut String, text: &str, most: usize) {
    tail.push_str(text);
    let extra = tail.chars().count().saturating_sub(most);
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
        tail if is_build_error(tail) => {
            // The first line says what it is; the compiler's own words follow, for a popup.
            format!(
                "the project does not build ({how})\n\n{}",
                cut(tail, BUILD_TAIL)
            )
        }
        tail => format!("{how}: {}", cut(tail, TAIL)),
    }
}

/// Whether what cargo wrote says the code did not compile (or a build script failed).
fn is_build_error(tail: &str) -> bool {
    tail.contains("could not compile")
        || tail.contains("error: aborting due to")
        || tail.contains("error: failed to run custom build command")
        || tail.contains("\nerror[E")
        || tail.starts_with("error[E")
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
