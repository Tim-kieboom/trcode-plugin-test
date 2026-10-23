//! What the test pane and a language adapter say to each other. They are two plugins
//! and the editor passes their calls on without reading them, so this is the whole
//! contract: the `payload` of each call and reply is the JSON of a type here.
//!
//! The test pane calls the adapter:
//!
//! | method | payload | answer |
//! |---|---|---|
//! | `discover` | [`DiscoverArgs`] | one reply, the last: a [`Page`] |
//! | `run` | [`RunArgs`] | replies as [`RunEvent`]s, the last is `Done` (an `Error` before it when the run could not be done) |
//! | `advance` | [`AdvanceArgs`] | one reply, the last: `{}` (only the fake adapter) |
//! | `configure` | [`Scenario`] | one reply, the last: `{}` (only the fake adapter) |
//!
//! A test is known by its [`TestItem::id`], which the adapter makes up and the pane
//! never reads. The name is for people, and two tests can have the same one.
//! Discovery is pulled a page at a time, one call each, because an adapter has a
//! small budget for each message it handles and a message has a size limit.

use serde::{Deserialize, Serialize};

pub use table::Table;

mod table;

/// The method names.
pub mod method {
    pub const DISCOVER: &str = "discover";
    pub const RUN: &str = "run";
    pub const ADVANCE: &str = "advance";
    pub const CONFIGURE: &str = "configure";
}

/// The most tests in one page of discovery. Measured, not chosen: the editor gives a
/// plugin 5 million fuel for each message it handles. For a page of 500 the fake adapter
/// (which only formats names) used 7.5 million; for 200 the adapter used 3.1 million and
/// the pane taking it 5.6 million, which is over. At 100 each side uses about half. An
/// adapter that does real work for each test needs fewer (see `docs/plan.md`).
pub const PAGE: usize = 100;

/// One test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestItem {
    /// Made up by the adapter; what a run is asked for by.
    pub id: String,
    /// What a person reads. Not unique.
    pub name: String,
    /// What it is in, such as the binary; shown before the name.
    #[serde(default)]
    pub group: String,
}

/// How a test ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Passed,
    Failed,
    Skipped,
    /// A status a later adapter has.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoverArgs {
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "page_size")]
    pub limit: usize,
}

fn page_size() -> usize {
    PAGE
}

impl DiscoverArgs {
    pub fn page_at(offset: usize) -> Self {
        Self {
            offset,
            limit: PAGE,
        }
    }
}

/// The answer to `discover`: the tests from `offset`, and how many there are in all. An
/// adapter that could not list them says why in `error` and has none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    pub offset: usize,
    pub total: usize,
    pub tests: Vec<TestItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Page {
    /// A page that says the tests could not be listed, and why.
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            offset: 0,
            total: 0,
            tests: Vec::new(),
            error: Some(message.into()),
        }
    }
}

/// What to run, and the number the pane gave this run: every event says which run it
/// belongs to, so an event of an earlier run is never taken for this one's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunArgs {
    pub run_id: u64,
    /// The tests to run; all of them when left out.
    #[serde(default)]
    pub ids: Option<Vec<String>>,
}

/// What an adapter says about a run, one reply each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RunEvent {
    Started {
        run_id: u64,
        id: String,
    },
    Finished {
        run_id: u64,
        id: String,
        status: Status,
        #[serde(default)]
        duration_ms: u64,
        #[serde(default)]
        message: String,
    },
    /// The run could not be done (the project does not build, the folder is not trusted):
    /// what to tell the user. `Done` follows.
    Error {
        run_id: u64,
        message: String,
    },
    /// The run is over; the last reply.
    Done {
        run_id: u64,
    },
    /// An event a later adapter has.
    #[serde(other)]
    Unknown,
}

impl RunEvent {
    /// The run an event belongs to.
    pub fn run_id(&self) -> Option<u64> {
        match self {
            Self::Started { run_id, .. }
            | Self::Finished { run_id, .. }
            | Self::Error { run_id, .. }
            | Self::Done { run_id } => Some(*run_id),
            Self::Unknown => None,
        }
    }
}

/// How many tests of each run the fake adapter finishes now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvanceArgs {
    pub n: usize,
}

/// What the fake adapter does, to provoke what a real one does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scenario {
    /// How many tests it has.
    #[serde(default = "six")]
    pub tests: usize,
    /// Tests that share a name, in groups of their own.
    #[serde(default)]
    pub same_names: bool,
    /// Keeps telling a run after it was cancelled, as an adapter that does not listen.
    #[serde(default)]
    pub talks_after_cancel: bool,
    /// Traps (panics) when told to advance a run.
    #[serde(default)]
    pub traps_on_advance: bool,
    /// Counts this far before answering a call: slow work that a budget must stop.
    #[serde(default)]
    pub burns: u64,
}

fn six() -> usize {
    6
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            tests: six(),
            same_names: false,
            talks_after_cancel: false,
            traps_on_advance: false,
            burns: 0,
        }
    }
}

/// The text of a payload.
pub fn to_payload<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// What a payload says, when it says a `T`.
pub fn from_payload<'a, T: Deserialize<'a>>(payload: &'a str) -> Option<T> {
    serde_json::from_str(payload).ok()
}

#[cfg(test)]
mod tests;
