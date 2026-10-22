use std::collections::VecDeque;
use std::hint::black_box;

use trcode_plugin_api::{FromPlugin, Plugin, ToPlugin};
use trcode_test_protocol::{
    AdvanceArgs, DiscoverArgs, Page, RunArgs, RunEvent, Scenario, Status, TestItem, from_payload,
    method, to_payload,
};

/// A run that was asked for and is not finished: who asked, and the tests still to run.
#[derive(Debug)]
struct Run {
    caller: String,
    request: u64,
    run_id: u64,
    pending: VecDeque<usize>,
}

#[derive(Debug, Default)]
pub struct FakeAdapter {
    scenario: Scenario,
    runs: Vec<Run>,
}

impl Plugin for FakeAdapter {
    fn handle(&mut self, message: ToPlugin) -> Vec<FromPlugin> {
        match message {
            ToPlugin::Call {
                from,
                request,
                method,
                payload,
            } => self.call(&from, request, &method, &payload),
            ToPlugin::CallCancelled { from, request } => {
                self.cancelled(&from, request);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

impl FakeAdapter {
    fn call(&mut self, from: &str, request: u64, name: &str, payload: &str) -> Vec<FromPlugin> {
        // Slow work, which the editor's budget for a message has to stop.
        // `black_box` in the loop: counted outside it, the compiler adds the numbers up at once.
        let mut sum = 0u64;
        for n in 0..self.scenario.burns {
            sum = black_box(sum.wrapping_add(n));
        }
        black_box(sum);
        match name {
            method::DISCOVER => self.discover(from, request, payload),
            method::RUN => self.run(from, request, payload),
            method::ADVANCE => self.advance(from, request, payload),
            method::CONFIGURE => self.configure(from, request, payload),
            _ => vec![reply(from, request, String::new(), true)],
        }
    }

    fn configure(&mut self, from: &str, request: u64, payload: &str) -> Vec<FromPlugin> {
        if let Some(scenario) = from_payload(payload) {
            self.scenario = scenario;
        }
        vec![reply(from, request, "{}".to_owned(), true)]
    }

    fn discover(&self, from: &str, request: u64, payload: &str) -> Vec<FromPlugin> {
        let args = from_payload(payload).unwrap_or_else(|| DiscoverArgs::page_at(0));
        let total = self.scenario.tests;
        let start = args.offset.min(total);
        let end = start.saturating_add(args.limit).min(total);
        let page = Page {
            offset: start,
            total,
            tests: (start..end).map(|index| self.item(index)).collect(),
            error: None,
        };
        vec![reply(from, request, to_payload(&page), true)]
    }

    /// Asks for a run: nothing is said until the clock is turned.
    fn run(&mut self, from: &str, request: u64, payload: &str) -> Vec<FromPlugin> {
        let Some(args) = from_payload::<RunArgs>(payload) else {
            return vec![reply(from, request, String::new(), true)];
        };
        let pending: VecDeque<usize> = match args.ids {
            None => (0..self.scenario.tests).collect(),
            Some(ids) => ids.iter().filter_map(|id| self.index_of(id)).collect(),
        };
        self.runs.push(Run {
            caller: from.to_owned(),
            request,
            run_id: args.run_id,
            pending,
        });
        Vec::new()
    }

    /// Turns the clock: each run finishes `n` more of its tests.
    fn advance(&mut self, from: &str, request: u64, payload: &str) -> Vec<FromPlugin> {
        assert!(
            !(self.scenario.traps_on_advance && !self.runs.is_empty()),
            "the fake adapter was told to die while a run is going"
        );
        let n = from_payload::<AdvanceArgs>(payload).map_or(1, |args| args.n);
        let mut said = Vec::new();
        for run in &mut self.runs {
            for _ in 0..n {
                let Some(index) = run.pending.pop_front() else {
                    break;
                };
                let id = id_of(index);
                let event = RunEvent::Started {
                    run_id: run.run_id,
                    id: id.clone(),
                };
                said.push(reply(&run.caller, run.request, to_payload(&event), false));
                let (status, message) = outcome(index);
                let event = RunEvent::Finished {
                    run_id: run.run_id,
                    id,
                    status,
                    duration_ms: 1 + (index % 5) as u64,
                    message,
                };
                said.push(reply(&run.caller, run.request, to_payload(&event), false));
            }
            if run.pending.is_empty() {
                let done = RunEvent::Done { run_id: run.run_id };
                said.push(reply(&run.caller, run.request, to_payload(&done), true));
            }
        }
        self.runs.retain(|run| !run.pending.is_empty());
        said.push(reply(from, request, "{}".to_owned(), true));
        said
    }

    /// The caller gave a run up. An adapter that listens stops; this one can be made not
    /// to, and keeps telling it, which the editor must drop.
    fn cancelled(&mut self, from: &str, request: u64) {
        if self.scenario.talks_after_cancel {
            return;
        }
        self.runs
            .retain(|run| !(run.caller == from && run.request == request));
    }

    fn item(&self, index: usize) -> TestItem {
        match self.scenario.same_names {
            // Two groups with a test of every name in each.
            true => TestItem {
                id: id_of(index),
                name: format!("case_{}", index / 2),
                group: format!("bin{}", index % 2),
            },
            false => TestItem {
                id: id_of(index),
                name: format!("case_{index}"),
                group: "bin".to_owned(),
            },
        }
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        let index: usize = id.strip_prefix('t')?.parse().ok()?;
        (index < self.scenario.tests).then_some(index)
    }
}

/// What the adapter makes the id of the test number `index`.
fn id_of(index: usize) -> String {
    format!("t{index}")
}

/// How the test number `index` ends: some fail and some are skipped, always the same ones.
fn outcome(index: usize) -> (Status, String) {
    match (index % 7, index % 11) {
        (3, _) => (Status::Failed, format!("assertion failed in case {index}")),
        (_, 5) => (Status::Skipped, String::new()),
        _ => (Status::Passed, String::new()),
    }
}

fn reply(to: &str, request: u64, payload: String, last: bool) -> FromPlugin {
    FromPlugin::Reply {
        to: to.to_owned(),
        request,
        payload,
        last,
    }
}
