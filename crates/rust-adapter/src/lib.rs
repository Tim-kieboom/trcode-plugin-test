//! The Rust language adapter of the test pane. It lists the tests of a project with
//! `cargo nextest list`, runs them with `cargo nextest run` and tells the pane how each
//! ended, as the commands write it, so the pane shows progress while a run goes on.
//!
//! Nextest (https://nexte.st) must be installed. The list is read from its JSON
//! (`--message-format json`, documented), a run from what it prints while it runs: there
//! is no stable machine-readable output of a run that needs no setup in the project (its
//! JUnit file needs a path in the project's own configuration, and its JSON for a run is
//! experimental). That text is not a documented format, so [`output`] is tested against
//! what nextest really prints (`tests/fixtures`, and nextest itself when it is installed).
//!
//! The logic is in [`RustAdapter`], which answers messages with messages and knows nothing
//! of WebAssembly; the last line makes it the module.

pub use adapter::RustAdapter;

mod adapter;
mod filter;
mod list;
mod locate;
mod output;
#[cfg(test)]
mod tests;

trcode_plugin_api::export_plugin!(RustAdapter);
