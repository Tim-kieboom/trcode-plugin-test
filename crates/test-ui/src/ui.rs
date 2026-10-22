use std::collections::HashMap;

use trcode_plugin_api::{FromPlugin, Plugin, Refusal, ToPlugin, WireLine, WireView};
use trcode_test_protocol::{
    AdvanceArgs, DiscoverArgs, Page, RunArgs, RunEvent, Status, TestItem, from_payload, method,
    to_payload,
};

/// The adapter the pane starts with. A pane that finds its adapters comes later.
pub const ADAPTER: &str = "fake-adapter";

/// The adapters the pane knows, which the action `adapter` goes through.
const ADAPTERS: [&str; 2] = [ADAPTER, "rust-adapter"];

/// How many rows the pane shows before it is told its size.
const DEFAULT_HEIGHT: usize = 20;

/// Where a test is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowStatus {
    /// Not run, or its run was cancelled.
    Unrun,
    /// Asked for, not started.
    Queued,
    Running,
    Passed,
    Failed,
    Skipped,
    /// It was running when the adapter went away: nobody knows.
    Unknown,
}

impl RowStatus {
    fn mark(self) -> &'static str {
        match self {
            Self::Unrun => "  ",
            Self::Queued => "..",
            Self::Running => ">>",
            Self::Passed => "ok",
            Self::Failed => "FAIL",
            Self::Skipped => "--",
            Self::Unknown => "?",
        }
    }

    fn index(self) -> usize {
        self as usize
    }

    fn of(status: Status) -> Self {
        match status {
            Status::Passed => Self::Passed,
            Status::Failed => Self::Failed,
            Status::Skipped => Self::Skipped,
            Status::Unknown => Self::Unknown,
        }
    }
}

/// How many tests are in each state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts([usize; 7]);

impl Counts {
    pub fn of(&self, status: RowStatus) -> usize {
        self.0[status.index()]
    }
}

#[derive(Debug)]
struct Row {
    item: TestItem,
    status: RowStatus,
}

/// A run that was asked for and is not over: the call it is told by, and its number.
#[derive(Debug, Clone, Copy)]
struct ActiveRun {
    request: u64,
    run_id: u64,
}

/// Where the pulling of the list of tests is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Discovery {
    #[default]
    Idle,
    /// A page was asked for with this call.
    Pulling(u64),
}

#[derive(Debug, Default)]
pub struct TestUi {
    rows: Vec<Row>,
    by_id: HashMap<String, usize>,
    total: usize,
    counts: Counts,
    discovery: Discovery,
    run: Option<ActiveRun>,
    next_request: u64,
    next_run: u64,
    selected: usize,
    top: usize,
    height: usize,
    notice: String,
    /// Which of [`ADAPTERS`] it talks to.
    adapter: usize,
}

impl Plugin for TestUi {
    fn handle(&mut self, message: ToPlugin) -> Vec<FromPlugin> {
        let mut said = match message {
            ToPlugin::Start => self.discover(),
            ToPlugin::Size { height, .. } => {
                self.height = usize::from(height);
                Vec::new()
            }
            ToPlugin::Action { action } => self.action(&action),
            ToPlugin::Click { row } => {
                // The first row of the view is the header.
                self.select(self.top + row.saturating_sub(1));
                Vec::new()
            }
            ToPlugin::Reply {
                request,
                payload,
                last,
                ..
            } => self.replied(request, &payload, last),
            ToPlugin::Refused { request, reason } => self.refused(request, &reason),
            _ => Vec::new(),
        };
        said.push(FromPlugin::set_view(self.view()));
        said
    }
}

impl TestUi {
    /// The adapter the pane talks to.
    pub fn adapter(&self) -> &'static str {
        ADAPTERS[self.adapter % ADAPTERS.len()]
    }

    /// How many tests are in each state.
    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// How many tests were discovered so far, and how many the adapter says there are.
    pub fn discovered(&self) -> (usize, usize) {
        (self.rows.len(), self.total)
    }

    /// The state of the test with this id.
    pub fn status_of(&self, id: &str) -> Option<RowStatus> {
        self.by_id.get(id).map(|index| self.rows[*index].status)
    }

    /// The number of the run that is going, if one is.
    pub fn running(&self) -> Option<u64> {
        self.run.map(|run| run.run_id)
    }

    /// The call the run that is going is told by, if one is.
    pub fn running_call(&self) -> Option<u64> {
        self.run.map(|run| run.request)
    }

    /// What the pane last had to say about something that went wrong or was stopped.
    pub fn notice(&self) -> &str {
        &self.notice
    }

    fn action(&mut self, action: &str) -> Vec<FromPlugin> {
        match action {
            "discover" => self.discover(),
            "run_all" => self.start_run(None),
            "run_selected" => {
                let Some(row) = self.rows.get(self.selected) else {
                    return Vec::new();
                };
                let ids = vec![row.item.id.clone()];
                self.start_run(Some(ids))
            }
            "cancel" => self.cancel(),
            "adapter" => {
                let mut said = self.cancel();
                self.adapter = (self.adapter + 1) % ADAPTERS.len();
                said.extend(self.discover());
                said
            }
            "up" => {
                self.select(self.selected.saturating_sub(1));
                Vec::new()
            }
            "down" => {
                self.select(self.selected + 1);
                Vec::new()
            }
            "advance" => {
                let (_, call) = self.call(method::ADVANCE, to_payload(&AdvanceArgs { n: 1 }));
                vec![call]
            }
            _ => Vec::new(),
        }
    }

    /// A call to the adapter, with the number the pane gives it.
    fn call(&mut self, name: &str, payload: String) -> (u64, FromPlugin) {
        self.next_request += 1;
        let request = self.next_request;
        let call = FromPlugin::Call {
            request,
            to: self.adapter().to_owned(),
            method: name.to_owned(),
            payload,
        };
        (request, call)
    }

    /// Starts again with the list of tests, a page at a time.
    fn discover(&mut self) -> Vec<FromPlugin> {
        self.rows.clear();
        self.by_id.clear();
        self.counts = Counts::default();
        self.total = 0;
        self.selected = 0;
        self.top = 0;
        self.next_page(0)
    }

    fn next_page(&mut self, offset: usize) -> Vec<FromPlugin> {
        let (request, call) =
            self.call(method::DISCOVER, to_payload(&DiscoverArgs::page_at(offset)));
        self.discovery = Discovery::Pulling(request);
        vec![call]
    }

    /// Runs `ids`, or everything. A run that is going is given up first.
    fn start_run(&mut self, ids: Option<Vec<String>>) -> Vec<FromPlugin> {
        let mut said = self.cancel();
        self.next_run += 1;
        let run_id = self.next_run;
        match &ids {
            None => {
                for index in 0..self.rows.len() {
                    self.set_status(index, RowStatus::Queued);
                }
            }
            Some(ids) => {
                for id in ids {
                    if let Some(index) = self.by_id.get(id).copied() {
                        self.set_status(index, RowStatus::Queued);
                    }
                }
            }
        }
        let (request, call) = self.call(method::RUN, to_payload(&RunArgs { run_id, ids }));
        self.run = Some(ActiveRun { request, run_id });
        self.notice.clear();
        said.push(call);
        said
    }

    /// Gives the run that is going up: what it had not finished is not run.
    fn cancel(&mut self) -> Vec<FromPlugin> {
        let Some(run) = self.run.take() else {
            return Vec::new();
        };
        self.settle(&[RowStatus::Queued, RowStatus::Running], RowStatus::Unrun);
        self.notice = format!("run {} was cancelled", run.run_id);
        vec![FromPlugin::CallCancel {
            request: run.request,
        }]
    }

    fn replied(&mut self, request: u64, payload: &str, last: bool) -> Vec<FromPlugin> {
        if self.discovery == Discovery::Pulling(request) {
            return self.page(payload);
        }
        match self.run {
            Some(run) if run.request == request => {
                self.event(run, payload, last);
                Vec::new()
            }
            // An answer to something that is over: a run that was given up, or the clock.
            _ => Vec::new(),
        }
    }

    fn page(&mut self, payload: &str) -> Vec<FromPlugin> {
        self.discovery = Discovery::Idle;
        let Some(page) = from_payload::<Page>(payload) else {
            self.notice = "the adapter sent a page that is not one".to_owned();
            return Vec::new();
        };
        if let Some(error) = page.error {
            self.notice = error;
            return Vec::new();
        }
        if page.offset != self.rows.len() {
            self.notice = format!(
                "a page from {} came when {} were known",
                page.offset,
                self.rows.len()
            );
            return Vec::new();
        }
        self.total = page.total;
        for item in page.tests {
            self.by_id.insert(item.id.clone(), self.rows.len());
            self.counts.0[RowStatus::Unrun.index()] += 1;
            self.rows.push(Row {
                item,
                status: RowStatus::Unrun,
            });
        }
        match self.rows.len() < self.total {
            true => self.next_page(self.rows.len()),
            false => Vec::new(),
        }
    }

    /// What the adapter says about the run. An event of another run is not this one's.
    fn event(&mut self, run: ActiveRun, payload: &str, last: bool) {
        let Some(event) = from_payload::<RunEvent>(payload) else {
            return;
        };
        if event.run_id() != Some(run.run_id) {
            return;
        }
        match event {
            RunEvent::Started { id, .. } => self.set_status_of(&id, RowStatus::Running),
            RunEvent::Finished { id, status, .. } => {
                self.set_status_of(&id, RowStatus::of(status));
            }
            RunEvent::Error { message, .. } => {
                // What was asked for is not run.
                self.settle(&[RowStatus::Queued, RowStatus::Running], RowStatus::Unrun);
                self.notice = message;
            }
            RunEvent::Done { .. } | RunEvent::Unknown => {}
        }
        if last {
            self.run = None;
        }
    }

    fn refused(&mut self, request: u64, reason: &Refusal) -> Vec<FromPlugin> {
        let what = match reason {
            Refusal::Gone => "the adapter went away".to_owned(),
            Refusal::NoSuchPlugin => format!("there is no adapter called {}", self.adapter()),
            other => format!("the adapter was refused: {other:?}"),
        };
        if self.run.is_some_and(|run| run.request == request) {
            self.run = None;
            // Nobody knows how these ended.
            self.settle(&[RowStatus::Queued, RowStatus::Running], RowStatus::Unknown);
            self.notice = what;
        } else if self.discovery == Discovery::Pulling(request) {
            self.discovery = Discovery::Idle;
            self.notice = what;
        }
        Vec::new()
    }

    fn set_status_of(&mut self, id: &str, status: RowStatus) {
        if let Some(index) = self.by_id.get(id).copied() {
            self.set_status(index, status);
        }
    }

    fn set_status(&mut self, index: usize, status: RowStatus) {
        let row = &mut self.rows[index];
        self.counts.0[row.status.index()] -= 1;
        self.counts.0[status.index()] += 1;
        row.status = status;
    }

    /// Every test that is in one of the states `from` goes to `to`.
    fn settle(&mut self, from: &[RowStatus], to: RowStatus) {
        for index in 0..self.rows.len() {
            if from.contains(&self.rows[index].status) {
                self.set_status(index, to);
            }
        }
    }

    fn select(&mut self, row: usize) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = row.min(self.rows.len() - 1);
        let height = self.window();
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + height {
            self.top = self.selected + 1 - height;
        }
    }

    /// How many tests are shown at once: the pane without its header.
    fn window(&self) -> usize {
        let height = match self.height {
            0 => DEFAULT_HEIGHT,
            height => height,
        };
        height.saturating_sub(1).max(1)
    }

    fn view(&self) -> WireView {
        let counts = self.counts;
        let mut header = format!(
            "{} | tests {}/{} | ok {} FAIL {} -- {} ? {}",
            self.adapter(),
            self.rows.len(),
            self.total.max(self.rows.len()),
            counts.of(RowStatus::Passed),
            counts.of(RowStatus::Failed),
            counts.of(RowStatus::Skipped),
            counts.of(RowStatus::Unknown),
        );
        if let Some(run) = self.run {
            header.push_str(&format!(" | run {}", run.run_id));
        }
        if !self.notice.is_empty() {
            header.push_str(&format!(" | {}", self.notice));
        }
        let mut lines = vec![WireLine::new(header, "explorer.dir")];
        let end = (self.top + self.window()).min(self.rows.len());
        for row in &self.rows[self.top.min(end)..end] {
            let text = format!(
                "[{}] {} {}",
                row.status.mark(),
                row.item.group,
                row.item.name
            );
            lines.push(WireLine::new(text, "explorer.file"));
        }
        let selected = match self.rows.is_empty() {
            true => None,
            false => Some(self.selected - self.top + 1),
        };
        WireView::rows(lines, selected)
    }
}
