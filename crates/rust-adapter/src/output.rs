//! Reads what `cargo nextest run` prints while it runs (on its standard error) and says how
//! each test ended, as it comes:
//!
//! ```text
//!         PASS [   0.003s] (1/5) fx::bin/fx tests::parses_empty
//!         FAIL [   0.004s] (5/5) fx tests::fails_here
//!   stdout ───
//!     ...what the test wrote...
//!   stderr ───
//!     ...
//! ────────────
//!      Summary [   0.005s] 5 tests run: 4 passed, 1 failed, 1 skipped
//!         FAIL [   0.004s] (5/5) fx tests::fails_here
//! ```
//!
//! A result line is a status word, the time, `(n/m)`, the binary id and the name of the test
//! (neither has a space). What a failed test wrote follows its line, until the next result
//! or a line of `─`. After `Summary` the failures are listed again, which are not new
//! results. This is not a documented format, so what is not understood is left alone.

use trcode_test_protocol::Status;

/// The most characters of what a failed test wrote that are kept.
const MOST_MESSAGE: usize = 4_000;

/// How one test ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub binary_id: String,
    pub name: String,
    pub status: Status,
    pub duration_ms: u64,
    /// What a failed test wrote, from where it panicked.
    pub message: String,
}

/// A failed test whose output is still being read.
#[derive(Debug)]
struct Failing {
    binary_id: String,
    name: String,
    duration_ms: u64,
    lines: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct RunParser {
    /// The start of a line whose end has not come yet.
    line: String,
    failing: Option<Failing>,
    after_summary: bool,
}

impl RunParser {
    /// Reads the next piece of the output; the tests that ended in it.
    pub(crate) fn feed(&mut self, piece: &str) -> Vec<Outcome> {
        self.line.push_str(piece);
        let mut ended = Vec::new();
        while let Some(end) = self.line.find('\n') {
            let line: String = self.line.drain(..=end).collect();
            self.read(line.trim_end_matches(['\n', '\r']), &mut ended);
        }
        ended
    }

    /// The output is over: the test whose output was being read, and a last line that had no
    /// end.
    pub(crate) fn finish(&mut self) -> Vec<Outcome> {
        let mut ended = Vec::new();
        let rest = std::mem::take(&mut self.line);
        if !rest.is_empty() {
            self.read(rest.trim_end_matches('\r'), &mut ended);
        }
        self.flush(&mut ended);
        ended
    }

    fn read(&mut self, line: &str, ended: &mut Vec<Outcome>) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("───") {
            self.flush(ended);
            return;
        }
        if trimmed.starts_with("Summary [") {
            self.flush(ended);
            self.after_summary = true;
            return;
        }
        if self.after_summary {
            return;
        }
        match result(line) {
            Some(result) => {
                self.flush(ended);
                match result.status {
                    Status::Failed => {
                        self.failing = Some(Failing {
                            binary_id: result.binary_id,
                            name: result.name,
                            duration_ms: result.duration_ms,
                            lines: Vec::new(),
                        });
                    }
                    status => ended.push(Outcome {
                        binary_id: result.binary_id,
                        name: result.name,
                        status,
                        duration_ms: result.duration_ms,
                        message: String::new(),
                    }),
                }
            }
            None => {
                if let Some(failing) = &mut self.failing {
                    failing.lines.push(line.to_owned());
                }
            }
        }
    }

    fn flush(&mut self, ended: &mut Vec<Outcome>) {
        let Some(failing) = self.failing.take() else {
            return;
        };
        ended.push(Outcome {
            message: message(&failing.lines),
            binary_id: failing.binary_id,
            name: failing.name,
            status: Status::Failed,
            duration_ms: failing.duration_ms,
        });
    }
}

/// A result line, read.
struct ResultLine {
    binary_id: String,
    name: String,
    status: Status,
    duration_ms: u64,
}

/// What a line says when it is the result of a test. Nextest right-aligns the status word to
/// twelve columns, which a line a test wrote itself (it is indented four more) does not.
fn result(line: &str) -> Option<ResultLine> {
    let indent = line.len() - line.trim_start().len();
    let (word, rest) = line.trim_start().split_once(' ')?;
    if indent + word.len() != 12 {
        return None;
    }
    let status = match word {
        "PASS" | "LEAK" => Status::Passed,
        "FAIL" | "TIMEOUT" | "LEAK-FAIL" | "EXEC-FAIL" => Status::Failed,
        "SKIP" => Status::Skipped,
        signal
            if signal.starts_with("SIG") && signal[3..].chars().all(|c| c.is_ascii_uppercase()) =>
        {
            Status::Failed
        }
        _ => return None,
    };
    let rest = rest.trim_start().strip_prefix('[')?;
    let (time, rest) = rest.split_once(']')?;
    let rest = rest.trim_start();
    // `(1/5)`, or `(───)` for a test that did not run.
    let rest = match rest.strip_prefix('(') {
        Some(counted) => counted.split_once(')')?.1.trim_start(),
        None => rest,
    };
    let (binary_id, name) = rest.split_once(' ')?;
    let name = name.trim();
    if binary_id.is_empty() || name.is_empty() {
        return None;
    }
    Some(ResultLine {
        binary_id: binary_id.to_owned(),
        name: name.to_owned(),
        status,
        duration_ms: milliseconds(time),
    })
}

/// `   0.003s` as milliseconds; nothing for the blank time of a test that did not run.
fn milliseconds(time: &str) -> u64 {
    let seconds: f64 = time.trim().trim_end_matches('s').parse().unwrap_or(0.0);
    (seconds * 1000.0).round() as u64
}

/// What a failed test wrote: what it wrote to its standard error when it wrote any (that is
/// where a panic goes), else what it wrote to its standard output, without the indent.
fn message(lines: &[String]) -> String {
    let stderr = lines
        .iter()
        .position(|line| line.trim() == "stderr ───")
        .map(|at| &lines[at + 1..]);
    let wanted: Vec<&String> = match stderr {
        Some(lines) if lines.iter().any(|line| !line.trim().is_empty()) => lines.iter().collect(),
        _ => lines
            .iter()
            .filter(|line| !matches!(line.trim(), "stdout ───" | "stderr ───"))
            .collect(),
    };
    let text: Vec<&str> = wanted
        .iter()
        .map(|line| line.strip_prefix("    ").unwrap_or(line.trim_start()))
        .collect();
    let text = text.join("\n");
    cut(text.trim(), MOST_MESSAGE)
}

/// `text` cut to `most` characters, with `…` where it was cut.
pub(crate) fn cut(text: &str, most: usize) -> String {
    match text.char_indices().nth(most) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}
