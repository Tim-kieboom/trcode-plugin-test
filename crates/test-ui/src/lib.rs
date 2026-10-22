//! The throwaway test pane. It lists the tests an adapter has, runs all of them or the
//! selected one, cancels, and shows what each did. It exists to find out whether the
//! calls between plugins are the right shape before anything else is built on them, so
//! it is plain on purpose and talks to one adapter, [`ADAPTER`].
//!
//! The logic is in [`TestUi`], which answers messages with messages and knows nothing
//! of WebAssembly; the last line makes it the module.

pub use ui::{ADAPTER, Counts, RowStatus, TestUi};

#[cfg(test)]
mod tests;
mod ui;

trcode_plugin_api::export_plugin!(TestUi);
