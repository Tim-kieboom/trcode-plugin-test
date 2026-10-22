mod adapter;
mod filter;
mod list;
mod output;
mod real;

use std::path::PathBuf;

/// What nextest printed for the project in `tests/fixture-project`, as it was.
pub(crate) fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}
