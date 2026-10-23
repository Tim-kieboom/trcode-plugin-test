//! Where the code of a test is: the line its panic names when it failed, otherwise the
//! line of its `fn`, found with `grep` (nextest does not say where a test is).
//!
//! A panic message is `thread 'tests::a' panicked at src/lib.rs:12:5:` (older compilers:
//! `panicked at 'text', src/lib.rs:12:5`). Only a path inside the project is a place the
//! editor can open; a panic in the standard library or another crate (an absolute path, or one
//! that leaves the project) is not one.

use trcode_test_protocol::Location;

/// The place a panic in `message` names, when it is in the project.
pub(crate) fn panic_location(message: &str) -> Option<Location> {
    let line = message
        .lines()
        .find_map(|line| line.split_once("panicked at ").map(|(_, rest)| rest))?;
    let line = line.trim_end().trim_end_matches(':');
    // Older: `'message', src/lib.rs:12:5`; the place is after the last `', `.
    let place = line.rsplit_once("', ").map_or(line, |(_, place)| place);
    let mut parts = place.rsplitn(3, ':');
    let column = parts.next()?.trim().parse().ok()?;
    let line_number = parts.next()?.trim().parse().ok()?;
    let path = in_project(parts.next()?.trim())?;
    Some(Location {
        path,
        line: line_number,
        column,
    })
}

/// `path` as the editor wants it (from the project's root, with `/`), or `None` when it is not
/// inside the project.
fn in_project(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let path = path.strip_prefix("./").unwrap_or(&path);
    let absolute = path.starts_with('/') || path.as_bytes().get(1) == Some(&b':');
    let leaves = path.split('/').any(|part| part == "..");
    (!path.is_empty() && !absolute && !leaves).then(|| path.to_owned())
}

/// The kind of binary a test is in, from its binary id.
#[derive(Debug)]
enum Target {
    /// `fx`: the library, tests in `src`.
    Library,
    /// `fx::bin/name`: `src/main.rs` or `src/bin/name.rs`.
    Binary(String),
    /// `fx::it`: the file `tests/it.rs` of the crate.
    Integration(String),
    /// An example or a benchmark.
    Other,
}

/// Reads what `grep -rnwF` printed for `fn name` and keeps the line that suits the test best.
#[derive(Debug)]
pub(crate) struct Locator {
    /// The modules the test is in, from its name: `["tests", "flood_tests"]`.
    modules: Vec<String>,
    /// What the binary the test is in is made of.
    target: Target,
    /// The start of a line whose end has not come yet.
    line: String,
    best: Option<(i32, Location)>,
}

impl Locator {
    /// For the test `id` (the binary id, a space, the name of the test).
    pub(crate) fn new(id: &str) -> Self {
        let (binary, name) = id.split_once(' ').unwrap_or(("", id));
        let mut parts: Vec<&str> = name.split("::").collect();
        parts.pop();
        let target = match binary.split_once("::") {
            None => Target::Library,
            Some((_, target)) => match target.split_once('/') {
                Some(("bin", name)) => Target::Binary(name.to_owned()),
                Some(_) => Target::Other,
                None => Target::Integration(target.to_owned()),
            },
        };
        Self {
            modules: parts.into_iter().map(str::to_owned).collect(),
            target,
            line: String::new(),
            best: None,
        }
    }

    pub(crate) fn feed(&mut self, text: &str) {
        self.line.push_str(text);
        while let Some(end) = self.line.find('\n') {
            let line: String = self.line.drain(..=end).collect();
            self.read(line.trim_end());
        }
    }

    pub(crate) fn finish(mut self) -> Option<Location> {
        let rest = std::mem::take(&mut self.line);
        self.read(rest.trim_end());
        self.best.map(|(_, place)| place)
    }

    /// `./src/a.rs:12:    fn name() {`
    fn read(&mut self, line: &str) {
        let mut parts = line.splitn(3, ':');
        let (Some(path), Some(number), Some(code)) = (parts.next(), parts.next(), parts.next())
        else {
            return;
        };
        let (Ok(number), Some(path)) = (number.parse::<u32>(), in_project(path)) else {
            return;
        };
        let column = code.find("fn ").map_or(0, |at| at as u32 + 1);
        let score = self.score(&path);
        if self.best.as_ref().is_none_or(|(best, _)| score > *best) {
            self.best = Some((
                score,
                Location {
                    path,
                    line: number,
                    column,
                },
            ));
        }
    }

    /// How well `path` suits the test: each of its modules that is a folder or the file of
    /// `path` counts one, and the kind of binary it is in decides between the code of the
    /// library and the integration tests next to it.
    fn score(&self, path: &str) -> i32 {
        let mut parts: Vec<&str> = path.split('/').collect();
        let file = parts.pop().unwrap_or_default();
        let stem = file.strip_suffix(".rs").unwrap_or(file);
        // `tests/` that is not `src/tests/` is the folder of the integration tests of a crate.
        let crate_tests = parts
            .iter()
            .enumerate()
            .any(|(at, part)| *part == "tests" && (at == 0 || parts[at - 1] != "src"));
        // `mod.rs` and `main.rs` stand for the folder they are in.
        if !matches!(stem, "mod" | "main") {
            parts.push(stem);
        }
        let modules = self
            .modules
            .iter()
            .filter(|module| parts.contains(&module.as_str()))
            .count() as i32;
        let kind = match (&self.target, crate_tests) {
            (Target::Integration(name), true)
                if name == stem || parts.last() == Some(&name.as_str()) =>
            {
                10
            }
            (Target::Integration(_), _) | (_, true) => -10,
            (Target::Library, false) if stem == "lib" => 5,
            (Target::Binary(name), false) if stem == "main" || stem == name => 5,
            _ => 0,
        };
        modules + kind
    }
}
