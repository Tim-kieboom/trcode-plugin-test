//! A language adapter that is not one. It has tests that it makes up, and runs them
//! by a clock you turn (`advance`), so what a real adapter does that is hard to
//! happen on purpose can be made to: a run that is cancelled in the middle, an adapter
//! that keeps talking after it was cancelled, one that dies, one that is slow, tests
//! with the same name, twenty thousand tests.
//!
//! The logic is in [`FakeAdapter`], which answers messages with messages and knows
//! nothing of WebAssembly; the last line makes it the module.

pub use adapter::FakeAdapter;

mod adapter;
#[cfg(test)]
mod tests;

trcode_plugin_api::export_plugin!(FakeAdapter);
