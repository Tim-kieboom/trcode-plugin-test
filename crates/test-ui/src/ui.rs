use std::collections::HashMap;

use crate::tree::{Kind, Tree, leaf};
use trcode_plugin_api::{
    FromPlugin, Plugin, Refusal, ToPlugin, WireButton, WireLine, WireMenuItem, WireView,
};
use trcode_test_protocol::{
    AdvanceArgs, DebugArgs, DebugReply, DiscoverArgs, LocateArgs, LocateReply, Location, Page,
    RunArgs, RunEvent, Status, Table, from_payload, method, to_payload,
};

/// The Rust adapter, which the pane tries first.
pub const RUST_ADAPTER: &str = "rust-adapter";

/// The fake adapter, for trying the pane without a project. It is only used when it is chosen
/// (`use_fake`): the pane never falls back to it.
pub const FAKE_ADAPTER: &str = "fake-adapter";

/// The adapters the pane knows, which the action `adapter` goes through. The pane starts with the
/// first. A pane that finds its adapters by itself comes later.
const ADAPTERS: [&str; 2] = [RUST_ADAPTER, FAKE_ADAPTER];

/// How many rows the pane shows before it is told its size.
const DEFAULT_HEIGHT: usize = 20;

/// How many columns the pane has before it is told its size.
const DEFAULT_WIDTH: usize = 80;

/// With this many tests or fewer every folder starts open; with more they start closed, which
/// is what a project with thousands of tests needs to be readable.
const OPEN_ALL_UP_TO: usize = 200;

/// The most lines under the tests for what the selected failed test wrote, with the line above
/// them that says whose it is.
const DETAIL_LINES: usize = 10;

/// What is known of a test that failed: what it wrote, and where it failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub message: String,
    pub location: Option<Location>,
}

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
    /// The tests, in one piece of memory (see [`Table`]), and how each is doing.
    table: Table,
    states: Vec<RowStatus>,
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
    width: usize,
    /// Whether the list of what the Run button offers is open.
    run_menu_open: bool,
    notice: String,
    /// Which of [`ADAPTERS`] it talks to.
    adapter: usize,
    /// What the tests that failed wrote and where they failed, by row.
    failures: HashMap<usize, Failure>,
    /// The call that asks the adapter where the code of a test is, and the test.
    locating: Option<(u64, usize)>,
    /// The request to the editor to open a file, and the name of the file.
    opening: Option<(u64, String)>,
    /// The call that asks the adapter to build a test for the debugger, and the test.
    building: Option<(u64, usize)>,
    /// The request to the editor to start the test under the debugger, and the name of the test.
    starting: Option<(u64, String)>,
}

impl Plugin for TestUi {
    fn handle(&mut self, message: ToPlugin) -> Vec<FromPlugin> {
        let mut said = match message {
            ToPlugin::Start => self.discover(),
            ToPlugin::Size { width, height } => {
                self.width = usize::from(width);
                self.height = usize::from(height);
                Vec::new()
            }
            ToPlugin::Press { button } => self.press(&button),
            ToPlugin::Action { action } => self.action(&action),
            ToPlugin::Click { row } => self.click(row),
            ToPlugin::ContextClick { row } => self.context_click(row),
            ToPlugin::Done { request } => {
                if self
                    .starting
                    .as_ref()
                    .is_some_and(|(start, _)| *start == request)
                {
                    self.starting = None;
                    self.notice.clear();
                }
                if self
                    .opening
                    .as_ref()
                    .is_some_and(|(open, _)| *open == request)
                {
                    self.opening = None;
                }
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
        (self.table.len(), self.total)
    }

    /// The state of the test with this id.
    pub fn status_of(&self, id: &str) -> Option<RowStatus> {
        self.table.find(id).map(|index| self.states[index])
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
            "goto" => self.goto_selected(),
            "show_error" => self.show_error(),
            "debug_selected" => self.debug_selected(),
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

    /// A button was pressed: Find tests asks for the tests, Run opens or closes its list, and what
    /// the list offers is done and the list closed.
    fn press(&mut self, button: &str) -> Vec<FromPlugin> {
        match button {
            "find" => self.discover(),
            "run_menu" => {
                self.run_menu_open = !self.run_menu_open;
                Vec::new()
            }
            "run_all" | "run_selected" | "cancel" => {
                self.run_menu_open = false;
                self.action(button)
            }
            _ => Vec::new(),
        }
    }

    /// The buttons at the top: Find tests, and Run with the list it opens.
    fn top_buttons(&self) -> Vec<WireButton> {
        let arrow = if self.run_menu_open { "^" } else { "v" };
        vec![
            WireButton::new("find", "Find tests"),
            WireButton::new("run_menu", format!("Run {arrow}")),
        ]
    }

    /// What the Run button offers, while its list is open: every way to run, and cancel.
    fn run_buttons(&self) -> Vec<WireButton> {
        let any = !self.table.is_empty();
        let selected = self
            .selected
            .is_some_and(|node| !self.tree.rows_under(node).is_empty());
        vec![
            WireButton::new("run_all", "Run all").enabled(any),
            WireButton::new("run_selected", "Run selected").enabled(selected),
            WireButton::new("cancel", "Cancel run").enabled(self.run.is_some()),
        ]
    }

    /// The rows of buttons above the tests, as the pane is drawn: one row, or two with the list.
    fn button_rows(&self) -> Vec<Vec<WireButton>> {
        let mut rows = vec![self.top_buttons()];
        if self.run_menu_open {
            rows.push(self.run_buttons());
        }
        rows
    }

    /// The user chose the adapter: what was going is given up and the tests are asked for again.
    fn use_adapter(&mut self, adapter: usize) -> Vec<FromPlugin> {
        let mut said = self.cancel();
        self.adapter = adapter;
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
        // What was said of the last try (an adapter that was not there) is not of this one.
        self.notice.clear();
        self.table.clear();
        self.states.clear();
        self.tree.clear();
        self.counts = Counts::default();
        self.failures.clear();
        self.locating = None;
        self.building = None;
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
            None => {
                self.failures.clear();
                self.change_all(None, RowStatus::Queued);
            }
            Some(ids) => {
                for id in ids {
                    if let Some(index) = self.table.find(id) {
                        self.failures.remove(&index);
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
        if let Some((call, row)) = self.locating
            && call == request
        {
            return match last {
                true => {
                    self.locating = None;
                    self.located(row, payload)
                }
                false => Vec::new(),
            };
        }
        if let Some((call, row)) = self.building
            && call == request
        {
            return match last {
                true => {
                    self.building = None;
                    self.built(row, payload)
                }
                false => Vec::new(),
            };
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
        if page.offset != self.table.len() {
            self.notice = format!(
                "a page from {} came when {} were known",
                page.offset,
                self.table.len()
            );
            return Vec::new();
        }
        self.total = page.total;
        let open = page.total <= OPEN_ALL_UP_TO;
        for item in page.tests {
            let row = self.table.push(&item);
            self.tree.add(row, &item.group, &item.name, open);
            self.counts.0[RowStatus::Unrun.index()] += 1;
            self.states.push(RowStatus::Unrun);
        }
        if self.selected.is_none() {
            self.selected = self.tree.first();
        }
        match self.table.len() < self.total {
            true => self.next_page(self.table.len()),
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
            RunEvent::Finished {
                id,
                status,
                message,
                location,
                ..
            } => {
                if let Some(index) = self.table.find(&id) {
                    match status {
                        Status::Failed if !message.is_empty() || location.is_some() => {
                            self.failures.insert(index, Failure { message, location });
                        }
                        _ => {
                            self.failures.remove(&index);
                        }
                    }
                }
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
            Refusal::NoSuchPlugin if self.adapter() == RUST_ADAPTER => {
                "the Rust Test Adapter is not installed or not on (Plugins: Manage)".to_owned()
            }
            Refusal::NoSuchPlugin => format!("there is no adapter called {}", self.adapter()),
            other => format!("the adapter was refused: {other:?}"),
        };
        if self.locating.is_some_and(|(call, _)| call == request) {
            self.locating = None;
            self.notice = format!("could not look for the code: {what}");
        } else if self.building.is_some_and(|(call, _)| call == request) {
            self.building = None;
            self.notice = format!("could not build the test for the debugger: {what}");
        } else if self
            .starting
            .as_ref()
            .is_some_and(|(start, _)| *start == request)
        {
            self.starting = None;
            self.notice = debugger_refusal(reason);
        } else if self
            .opening
            .as_ref()
            .is_some_and(|(open, _)| *open == request)
        {
            let name = self
                .opening
                .take()
                .map(|(_, name)| name)
                .unwrap_or_default();
            let detail = match reason {
                Refusal::Failed { detail } | Refusal::BadArgument { detail } => detail.clone(),
                other => format!("{other:?}"),
            };
            self.notice = format!("could not open {name}: {detail}");
        } else if self.run.is_some_and(|run| run.request == request) {
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
        if let Some(index) = self.table.find(id) {
            self.set_status(index, status);
        }
    }

    fn set_status(&mut self, index: usize, status: RowStatus) {
        let before = self.states[index];
        self.counts.0[before.index()] -= 1;
        self.counts.0[status.index()] += 1;
        self.states[index] = status;
        self.tree.moved(index, before, status);
    }

    /// Every test that is in one of the states `from` goes to `to`.
    fn settle(&mut self, from: &[RowStatus], to: RowStatus) {
        self.change_all(Some(from), to);
    }

    /// Every test (or those in one of the states `from`) goes to `to`, and the folders are counted
    /// again once, not for each test.
    fn change_all(&mut self, from: Option<&[RowStatus]>, to: RowStatus) {
        for state in &mut self.states {
            if from.is_some_and(|from| !from.contains(state)) {
                continue;
            }
            self.counts.0[state.index()] -= 1;
            self.counts.0[to.index()] += 1;
            *state = to;
        }
        self.tree.recount(self.states.iter().copied());
    }

    // --- the tree: moving about in it ---

    /// Selects the test or folder the line `row` of the view shows (the first line is the header),
    /// and opens or closes it when it is a folder.
    /// A test that is clicked is gone to in the editor.
    fn click(&mut self, row: usize) -> Vec<FromPlugin> {
        self.run_menu_open = false;
        let Some(node) = self.node_at(row) else {
            return Vec::new();
        };
        self.selected = Some(node);
        if self.tree.node(node).is_folder() {
            let open = self.tree.is_open(node);
            self.tree.set_open(node, !open);
            return Vec::new();
        }
        self.goto_selected()
    }

    /// The node the line `row` of the view shows (the first line is the header): none for the
    /// header, for what is below the tests (what the selected test wrote is no row of the tree)
    /// and for a line past the last node.
    fn node_at(&self, row: usize) -> Option<usize> {
        if row == 0 || row > self.window() {
            return None;
        }
        let mut at = self.top.or(self.tree.first());
        for _ in 1..row {
            at = at.and_then(|node| self.tree.next(node));
        }
        at
    }

    /// A right click: what is clicked is selected (a folder is not opened or closed), and the
    /// menu is what can be done with it. A line that is no node has the commands for all.
    fn context_click(&mut self, row: usize) -> Vec<FromPlugin> {
        self.run_menu_open = false;
        let items = match self.node_at(row) {
            Some(node) => {
                self.selected = Some(node);
                self.node_menu(node)
            }
            None => self.general_menu(),
        };
        vec![FromPlugin::Menu { items }]
    }

    /// What the menu of a test or a folder offers: for a test, its code, why it failed, run it and
    /// debug it; for a folder, run what is in it.
    fn node_menu(&self, node: usize) -> Vec<WireMenuItem> {
        let mut items = Vec::new();
        if let Kind::Test { row } = self.tree.node(node).kind {
            items.push(WireMenuItem::action("goto"));
            if self
                .failures
                .get(&row)
                .is_some_and(|f| !f.message.is_empty())
            {
                items.push(WireMenuItem::action("show_error"));
            }
            items.push(WireMenuItem::Separator);
            items.push(WireMenuItem::action("run_selected"));
            items.push(WireMenuItem::action("debug_selected"));
        } else {
            items.push(WireMenuItem::action("run_selected"));
        }
        if self.run.is_some() {
            items.push(WireMenuItem::action("cancel"));
        }
        items.push(WireMenuItem::Separator);
        items.push(WireMenuItem::action("discover"));
        items
    }

    /// What the menu offers away from the tests: run everything, find the tests again.
    fn general_menu(&self) -> Vec<WireMenuItem> {
        let mut items = vec![WireMenuItem::action("run_all")];
        if self.run.is_some() {
            items.push(WireMenuItem::action("cancel"));
        }
        items.push(WireMenuItem::Separator);
        items.push(WireMenuItem::action("discover"));
        items
    }

    /// The selected test, when a test is selected.
    fn selected_test(&self) -> Option<usize> {
        let selected = self.selected?;
        match self.tree.node(selected).kind {
            Kind::Test { row } => Some(row),
            Kind::Folder { .. } => None,
        }
    }

    /// Shows what the selected test wrote when it failed, and where, in a popup: the whole of
    /// it, which the lines under the tests cut short.
    fn show_error(&mut self) -> Vec<FromPlugin> {
        let Some(row) = self.selected_test() else {
            self.notice = "choose a test to see its error".to_owned();
            return Vec::new();
        };
        let name = self.table.name(row).to_owned();
        let Some(failure) = self.failures.get(&row) else {
            self.notice = match self.states[row] {
                RowStatus::Failed => format!("{} failed and said nothing", leaf(&name)),
                _ => format!("{} has no error to show", leaf(&name)),
            };
            return Vec::new();
        };
        let mut text = failure.message.clone();
        if let Some(place) = &failure.location {
            text.push_str(&format!("\n\nat {}:{}", place.path, place.line));
        }
        vec![FromPlugin::Popup { title: name, text }]
    }

    /// Asks the adapter for the selected test built and ready to start; the editor starts it
    /// under the debugger once the adapter has built it.
    fn debug_selected(&mut self) -> Vec<FromPlugin> {
        let Some(row) = self.selected_test() else {
            self.notice = "choose a test to debug, not a folder".to_owned();
            return Vec::new();
        };
        if self.building.is_some() {
            self.notice = "a test is being built for the debugger already".to_owned();
            return Vec::new();
        }
        let id = self.table.id(row).to_owned();
        let (request, call) = self.call(method::DEBUG, to_payload(&DebugArgs { id }));
        self.building = Some((request, row));
        self.notice = format!("building {} for the debugger", leaf(self.table.name(row)));
        vec![call]
    }

    /// The adapter built the test, or said why not: the editor is asked to debug it.
    fn built(&mut self, row: usize, payload: &str) -> Vec<FromPlugin> {
        let name = leaf(self.table.name(row)).to_owned();
        let reply = from_payload::<DebugReply>(payload).unwrap_or_default();
        if let Some(error) = reply.error {
            self.notice = error;
            return Vec::new();
        }
        if reply.program.is_empty() {
            self.notice = format!("{} cannot run tests in the debugger", self.adapter());
            return Vec::new();
        }
        self.next_request += 1;
        let request = self.next_request;
        self.starting = Some((request, name.clone()));
        self.notice = format!("starting {name} in the debugger");
        vec![FromPlugin::Debug {
            request,
            program: reply.program,
            args: reply.args,
            env: reply.env,
            cwd: reply.cwd,
        }]
    }

    /// Opens the code of the selected test: where it failed when it did, else where the adapter
    /// finds it.
    fn goto_selected(&mut self) -> Vec<FromPlugin> {
        let Some(selected) = self.selected else {
            return Vec::new();
        };
        let Kind::Test { row } = self.tree.node(selected).kind else {
            return Vec::new();
        };
        if let Some(place) = self.failures.get(&row).and_then(|f| f.location.clone()) {
            return vec![self.open(&place)];
        }
        let id = self.table.id(row).to_owned();
        let (request, call) = self.call(method::LOCATE, to_payload(&LocateArgs { id }));
        self.locating = Some((request, row));
        self.notice = format!("looking for the code of {}", leaf(self.table.name(row)));
        vec![call]
    }

    /// The adapter said where the code of the test of `row` is, or that it does not know.
    fn located(&mut self, row: usize, payload: &str) -> Vec<FromPlugin> {
        self.notice.clear();
        let reply = from_payload::<LocateReply>(payload).unwrap_or_default();
        match reply.location {
            Some(place) => vec![self.open(&place)],
            None => {
                self.notice = reply
                    .error
                    .unwrap_or_else(|| format!("no code found for {}", leaf(self.table.name(row))));
                Vec::new()
            }
        }
    }

    /// Asks the editor to open `place`.
    fn open(&mut self, place: &Location) -> FromPlugin {
        self.next_request += 1;
        let request = self.next_request;
        self.opening = Some((request, place.path.clone()));
        FromPlugin::OpenFile {
            request,
            path: place.path.clone(),
            line: Some(place.line),
            column: (place.column > 0).then_some(place.column),
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
        let ids = match rows.len() == self.table.len() {
            true => None,
            false => Some(
                rows.iter()
                    .map(|row| self.table.id(*row).to_owned())
                    .collect(),
            ),
        };
        self.start_run(ids)
    }

    /// How many tests are shown at once: the pane without its buttons and its header.
    fn window(&self) -> usize {
        let height = match self.height {
            0 => DEFAULT_HEIGHT,
            height => height,
        };
        let width = match self.width {
            0 => DEFAULT_WIDTH,
            width => width,
        };
        let buttons: usize = self
            .button_rows()
            .iter()
            .map(|row| button_lines(row, width))
            .sum();
        let below = self.detail().len();
        height
            .saturating_sub(buttons)
            .saturating_sub(1)
            .saturating_sub(below)
            .max(1)
    }

    /// What the selected test that failed wrote: a line with its name, and what it wrote as far
    /// as there is room, each line cut to the width of the pane.
    fn detail(&self) -> Vec<WireLine> {
        let Some(selected) = self.selected else {
            return Vec::new();
        };
        let Kind::Test { row } = self.tree.node(selected).kind else {
            return Vec::new();
        };
        let Some(failure) = self.failures.get(&row) else {
            return Vec::new();
        };
        let height = match self.height {
            0 => DEFAULT_HEIGHT,
            height => height,
        };
        let width = match self.width {
            0 => DEFAULT_WIDTH,
            width => width,
        };
        // At most a third of the pane, so the tests stay readable.
        let room = DETAIL_LINES.min(height / 3).max(2);
        let place = failure
            .location
            .as_ref()
            .map(|place| format!(" at {}:{}", place.path, place.line))
            .unwrap_or_default();
        let title = format!("-- {}{place}", leaf(self.table.name(row)));
        let mut lines = vec![WireLine::new(clip(&title, width), "explorer.dir")];
        let text: Vec<&str> = failure.message.lines().collect();
        for line in text.iter().take(room - 1) {
            lines.push(WireLine::new(clip(line, width), "editor.diagnostic.error"));
        }
        if text.len() > room - 1 {
            let last = lines.len() - 1;
            lines[last] = WireLine::new(
                clip(
                    &format!("{} (+{} more)", text[room - 2], text.len() - (room - 2)),
                    width,
                ),
                "editor.diagnostic.error",
            );
        }
        lines
    }

    fn view(&self) -> WireView {
        let counts = self.counts;
        let mut header = format!(
            "{} | tests {}/{} | ok {} FAIL {} -- {} ? {}",
            self.adapter(),
            self.table.len(),
            self.total.max(self.table.len()),
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
        lines.extend(self.detail());
        let tests = WireView::rows(lines, selected_line);
        let mut parts: Vec<WireView> = self
            .button_rows()
            .into_iter()
            .map(WireView::buttons)
            .collect();
        parts.push(tests);
        WireView::column(parts)
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
                let name = leaf(self.table.name(row));
                let text = format!("{indent}[{}] {name}", self.states[row].mark());
                WireLine::new(text, "explorer.file")
            }
        }
    }
}

/// Why the editor did not start the debugger, for the user.
fn debugger_refusal(reason: &Refusal) -> String {
    match reason {
        Refusal::Untrusted => {
            "the folder is not trusted, so the test may not start: answer the question and debug again"
                .to_owned()
        }
        Refusal::Busy => "a debug session is going: stop it first".to_owned(),
        Refusal::Denied { capability } => {
            format!("the editor denied {}: turn it on for this plugin", capability.name())
        }
        Refusal::Failed { detail } | Refusal::BadArgument { detail } => detail.clone(),
        other => format!("the debugger was refused: {other:?}"),
    }
}

/// `text` on one line, a tab as spaces, cut to `width` characters with `…` where it was cut.
fn clip(text: &str, width: usize) -> String {
    let text = text.replace('\t', "    ");
    match text.char_indices().nth(width.max(2) - 1) {
        Some((at, _)) if text.chars().count() > width => format!("{}…", &text[..at]),
        _ => text,
    }
}

/// How many lines a row of buttons takes in `width` columns: each is a space, its label and a
/// space, with one column between them, and one that would pass the edge starts a new line (as the
/// editor draws them).
fn button_lines(buttons: &[WireButton], width: usize) -> usize {
    let (mut lines, mut at) = (1, 0);
    for button in buttons {
        let size = button.label.chars().count() + 2;
        if at > 0 && at + size > width {
            lines += 1;
            at = 0;
        }
        at += size + 1;
    }
    lines
}
