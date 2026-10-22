use std::collections::HashMap;

use crate::tree::{Kind, Tree};
use trcode_plugin_api::{FromPlugin, Plugin, Refusal, ToPlugin, WireLine, WireView};
use trcode_test_protocol::{
    AdvanceArgs, DiscoverArgs, Page, RunArgs, RunEvent, Status, TestItem, from_payload, method,
    to_payload,
};

/// The Rust adapter, which the pane tries first.
pub const RUST_ADAPTER: &str = "rust-adapter";

/// The fake adapter, for trying the pane without a project.
pub const FAKE_ADAPTER: &str = "fake-adapter";

/// The adapters the pane knows, in the order it tries them. A pane that finds its adapters by
/// itself comes later.
const ADAPTERS: [&str; 2] = [RUST_ADAPTER, FAKE_ADAPTER];

/// How many rows the pane shows before it is told its size.
const DEFAULT_HEIGHT: usize = 20;

/// With this many tests or fewer every folder starts open; with more they start closed, which
/// is what a project with thousands of tests needs to be readable.
const OPEN_ALL_UP_TO: usize = 200;

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
    pub(crate) fn mark(self) -> &'static str {
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

    pub(crate) fn index(self) -> usize {
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
    tree: Tree,
    /// The node that is selected, and the first one shown (`None`: the first of the tree).
    selected: Option<usize>,
    top: Option<usize>,
    height: usize,
    notice: String,
    /// Which of [`ADAPTERS`] it talks to.
    adapter: usize,
    /// Whether the user chose the adapter. When not, an adapter that is not installed is passed
    /// over for the next one.
    user_chose: bool,
    /// How many adapters the pane has passed over since the user last asked for the tests.
    passed_over: usize,
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
                self.click(row);
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
            "run_selected" => self.run_selected(),
            "cancel" => self.cancel(),
            "adapter" => self.use_adapter((self.adapter + 1) % ADAPTERS.len()),
            "use_rust" => self.use_adapter(0),
            "use_fake" => self.use_adapter(1),
            "up" => {
                self.move_up();
                Vec::new()
            }
            "down" => {
                self.move_down();
                Vec::new()
            }
            "right" => {
                self.open_or_enter();
                Vec::new()
            }
            "left" => {
                self.close_or_leave();
                Vec::new()
            }
            "advance" => {
                let (_, call) = self.call(method::ADVANCE, to_payload(&AdvanceArgs { n: 1 }));
                vec![call]
            }
            _ => Vec::new(),
        }
    }

    /// The user chose the adapter: what was going is given up and the tests are asked for again.
    fn use_adapter(&mut self, adapter: usize) -> Vec<FromPlugin> {
        let mut said = self.cancel();
        self.adapter = adapter;
        self.user_chose = true;
        said.extend(self.discover());
        said
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
        self.passed_over = 0;
        self.ask_for_the_tests()
    }

    fn ask_for_the_tests(&mut self) -> Vec<FromPlugin> {
        self.rows.clear();
        self.by_id.clear();
        self.tree.clear();
        self.counts = Counts::default();
        self.total = 0;
        self.selected = None;
        self.top = None;
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
            None => self.change_all(None, RowStatus::Queued),
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
        let open = page.total <= OPEN_ALL_UP_TO;
        for item in page.tests {
            let row = self.rows.len();
            self.tree.add(row, &item.group, &item.name, open);
            self.by_id.insert(item.id.clone(), row);
            self.counts.0[RowStatus::Unrun.index()] += 1;
            self.rows.push(Row {
                item,
                status: RowStatus::Unrun,
            });
        }
        if self.selected.is_none() {
            self.selected = self.tree.first();
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
            // An adapter that is not installed is passed over, until one answers or all were
            // tried, unless the user chose this one.
            let passes = matches!(reason, Refusal::NoSuchPlugin)
                && !self.user_chose
                && self.passed_over + 1 < ADAPTERS.len();
            if passes {
                self.passed_over += 1;
                self.adapter = (self.adapter + 1) % ADAPTERS.len();
                return self.ask_for_the_tests();
            }
            self.notice = match matches!(reason, Refusal::NoSuchPlugin) && !self.user_chose {
                true => "no test adapter is installed".to_owned(),
                false => what,
            };
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
        let before = row.status;
        self.counts.0[before.index()] -= 1;
        self.counts.0[status.index()] += 1;
        row.status = status;
        self.tree.moved(index, before, status);
    }

    /// Every test that is in one of the states `from` goes to `to`.
    fn settle(&mut self, from: &[RowStatus], to: RowStatus) {
        self.change_all(Some(from), to);
    }

    /// Every test (or those in one of the states `from`) goes to `to`, and the folders are counted
    /// again once, not for each test.
    fn change_all(&mut self, from: Option<&[RowStatus]>, to: RowStatus) {
        for row in &mut self.rows {
            if from.is_some_and(|from| !from.contains(&row.status)) {
                continue;
            }
            self.counts.0[row.status.index()] -= 1;
            self.counts.0[to.index()] += 1;
            row.status = to;
        }
        self.tree.recount(self.rows.iter().map(|row| row.status));
    }

    // --- the tree: moving about in it ---

    /// Selects the test or folder the line `row` of the view shows (the first line is the header),
    /// and opens or closes it when it is a folder.
    fn click(&mut self, row: usize) {
        let mut at = self.top.or(self.tree.first());
        for _ in 1..row {
            at = at.and_then(|node| self.tree.next(node));
        }
        let Some(node) = at.filter(|_| row > 0) else {
            return;
        };
        self.selected = Some(node);
        if self.tree.node(node).is_folder() {
            let open = self.tree.is_open(node);
            self.tree.set_open(node, !open);
        }
    }

    fn move_down(&mut self) {
        let Some(selected) = self.selected else {
            return;
        };
        let Some(next) = self.tree.next(selected) else {
            return;
        };
        self.selected = Some(next);
        self.keep_in_sight(true);
    }

    fn move_up(&mut self) {
        let Some(selected) = self.selected else {
            return;
        };
        let Some(before) = self.tree.previous(selected) else {
            return;
        };
        self.selected = Some(before);
        self.keep_in_sight(false);
    }

    /// A folder is opened; one that is open (or a test) goes to its first test or folder.
    fn open_or_enter(&mut self) {
        let Some(selected) = self.selected else {
            return;
        };
        if !self.tree.node(selected).is_folder() {
            return;
        }
        match self.tree.is_open(selected) {
            false => self.tree.set_open(selected, true),
            true => self.move_down(),
        }
    }

    /// A folder that is open is closed; anything else goes to the folder it is in.
    fn close_or_leave(&mut self) {
        let Some(selected) = self.selected else {
            return;
        };
        if self.tree.is_open(selected) {
            self.tree.set_open(selected, false);
            return;
        }
        if let Some(parent) = self.tree.node(selected).parent {
            self.selected = Some(parent);
            self.keep_in_sight(false);
        }
    }

    /// Scrolls so that the selected node is shown: it is the first shown when it went up, the
    /// last when it went down.
    fn keep_in_sight(&mut self, down: bool) {
        let Some(selected) = self.selected else {
            return;
        };
        let height = self.window();
        let mut at = self.top.or(self.tree.first());
        for _ in 0..height {
            match at {
                Some(node) if node == selected => return,
                Some(node) => at = self.tree.next(node),
                None => break,
            }
        }
        if !down {
            self.top = Some(selected);
            return;
        }
        let mut first = selected;
        for _ in 1..height {
            match self.tree.previous(first) {
                Some(before) => first = before,
                None => break,
            }
        }
        self.top = Some(first);
    }

    /// Runs the selected test, or every test in the selected folder.
    fn run_selected(&mut self) -> Vec<FromPlugin> {
        let Some(selected) = self.selected else {
            return Vec::new();
        };
        let rows = self.tree.rows_under(selected);
        if rows.is_empty() {
            return Vec::new();
        }
        // Everything is asked for as everything, not as every id.
        let ids = match rows.len() == self.rows.len() {
            true => None,
            false => Some(
                rows.iter()
                    .map(|row| self.rows[*row].item.id.clone())
                    .collect(),
            ),
        };
        self.start_run(ids)
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
        let mut selected_line = None;
        let mut at = self.top.or(self.tree.first());
        for _ in 0..self.window() {
            let Some(node) = at else {
                break;
            };
            if Some(node) == self.selected {
                selected_line = Some(lines.len());
            }
            lines.push(self.line_of(node));
            at = self.tree.next(node);
        }
        WireView::rows(lines, selected_line)
    }

    /// The line of a node: indented by its depth, a folder with an arrow and how many of its
    /// tests are done, a test with its state.
    fn line_of(&self, node: usize) -> WireLine {
        let tree = &self.tree;
        let this = tree.node(node);
        let indent = "  ".repeat(tree.depth(node));
        match this.kind {
            Kind::Folder { open } => {
                let arrow = if open { "v" } else { ">" };
                let done = this.counts[RowStatus::Passed.index()]
                    + this.counts[RowStatus::Failed.index()]
                    + this.counts[RowStatus::Skipped.index()];
                let text = format!(
                    "{indent}{arrow} [{}] {} ({done}/{})",
                    this.status().mark(),
                    this.name,
                    this.tests
                );
                WireLine::new(text, "explorer.dir")
            }
            Kind::Test { row } => {
                let text = format!("{indent}[{}] {}", self.rows[row].status.mark(), this.name);
                WireLine::new(text, "explorer.file")
            }
        }
    }
}
