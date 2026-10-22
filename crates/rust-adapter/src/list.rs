//! Reads the tests out of the JSON of `cargo nextest list --message-format json` as it
//! comes, a piece at a time, without holding the document: the listing of a big project is
//! megabytes, a plugin has a small memory, and each piece the editor sends gets a budget
//! that fits the piece, so the work has to be done piece by piece.
//!
//! Only what is needed is kept: in `"rust-suites"`, each suite (named by its binary id) has
//! `"testcases"`, each a name with an `"ignored"` flag. Tests that are ignored are left out:
//! nextest does not run them.

use trcode_test_protocol::TestItem;

/// Where in the document the scanner is: an object (and the key whose value is being read) or
/// an array.
#[derive(Debug)]
enum Level {
    Object {
        key: Option<String>,
        wants_key: bool,
    },
    Array,
}

/// A string that is being read.
#[derive(Debug, Default)]
struct Text {
    buf: String,
    escaping: Escaping,
    /// A high surrogate of a `\u` pair, waiting for its low half.
    high: Option<u32>,
}

#[derive(Debug, Default)]
enum Escaping {
    #[default]
    No,
    Backslash,
    Unicode(String),
}

enum Read {
    More,
    Done(String),
}

impl Text {
    fn step(&mut self, c: char) -> Read {
        match std::mem::take(&mut self.escaping) {
            Escaping::No => match c {
                '\\' => self.escaping = Escaping::Backslash,
                '"' => return Read::Done(std::mem::take(&mut self.buf)),
                other => self.buf.push(other),
            },
            Escaping::Backslash => match c {
                'n' => self.buf.push('\n'),
                't' => self.buf.push('\t'),
                'r' => self.buf.push('\r'),
                'b' => self.buf.push('\u{8}'),
                'f' => self.buf.push('\u{c}'),
                'u' => self.escaping = Escaping::Unicode(String::new()),
                other => self.buf.push(other),
            },
            Escaping::Unicode(mut digits) => {
                digits.push(c);
                match digits.len() {
                    4 => self.unicode(&digits),
                    _ => self.escaping = Escaping::Unicode(digits),
                }
            }
        }
        Read::More
    }

    fn unicode(&mut self, digits: &str) {
        let code = u32::from_str_radix(digits, 16).unwrap_or(0xfffd);
        match (self.high.take(), code) {
            (Some(high), 0xdc00..=0xdfff) => {
                let joined = 0x10000 + ((high - 0xd800) << 10) + (code - 0xdc00);
                self.buf.push(char::from_u32(joined).unwrap_or('\u{fffd}'));
            }
            (_, 0xd800..=0xdbff) => self.high = Some(code),
            (_, other) => self.buf.push(char::from_u32(other).unwrap_or('\u{fffd}')),
        }
    }
}

/// A test that is being read: its suite and name, and whether it turned out ignored.
#[derive(Debug)]
struct Pending {
    suite: String,
    name: String,
    ignored: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ListScanner {
    levels: Vec<Level>,
    text: Option<Text>,
    /// A number, `true`, `false` or `null`, which has no quotes and so no end of its own.
    literal: String,
    pending: Option<Pending>,
    tests: Vec<TestItem>,
}

impl ListScanner {
    /// Reads the next piece of the document.
    pub(crate) fn feed(&mut self, piece: &str) {
        for c in piece.chars() {
            self.step(c);
        }
    }

    /// The tests, when the document ended where it should have.
    pub(crate) fn finish(mut self) -> Result<Vec<TestItem>, String> {
        self.flush_literal();
        if self.text.is_some() || !self.levels.is_empty() {
            return Err("the list of tests ended in the middle".to_owned());
        }
        Ok(self.tests)
    }

    fn step(&mut self, c: char) {
        if let Some(text) = &mut self.text {
            if let Read::Done(done) = text.step(c) {
                self.text = None;
                self.string(done);
            }
            return;
        }
        match c {
            '"' => {
                self.flush_literal();
                self.text = Some(Text::default());
            }
            '{' => {
                self.flush_literal();
                self.open_object();
            }
            '[' => {
                self.flush_literal();
                self.levels.push(Level::Array);
            }
            '}' => {
                self.flush_literal();
                self.levels.pop();
                self.close_object();
            }
            ']' => {
                self.flush_literal();
                self.levels.pop();
            }
            ':' => {
                if let Some(Level::Object { wants_key, .. }) = self.levels.last_mut() {
                    *wants_key = false;
                }
            }
            ',' => {
                self.flush_literal();
                if let Some(Level::Object { wants_key, .. }) = self.levels.last_mut() {
                    *wants_key = true;
                }
            }
            c if c.is_whitespace() => self.flush_literal(),
            other => self.literal.push(other),
        }
    }

    /// A string that ended: a key, or a value (which nothing here wants).
    fn string(&mut self, text: String) {
        if let Some(Level::Object { key, wants_key }) = self.levels.last_mut()
            && *wants_key
        {
            *key = Some(text);
        }
    }

    fn flush_literal(&mut self) {
        if self.literal.is_empty() {
            return;
        }
        let literal = std::mem::take(&mut self.literal);
        let ignored =
            literal == "true" && self.path_is(&["rust-suites", "*", "testcases", "*", "ignored"]);
        if ignored && let Some(pending) = &mut self.pending {
            pending.ignored = true;
        }
    }

    /// The keys that lead to where the scanner is, outermost first.
    fn path(&self) -> Vec<&str> {
        self.levels
            .iter()
            .filter_map(|level| match level {
                Level::Object { key: Some(key), .. } => Some(key.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Whether the path is `shape`, where `*` stands for any key.
    fn path_is(&self, shape: &[&str]) -> bool {
        let path = self.path();
        path.len() == shape.len()
            && path
                .iter()
                .zip(shape)
                .all(|(key, wanted)| *wanted == "*" || key == wanted)
    }

    fn open_object(&mut self) {
        if self.path_is(&["rust-suites", "*", "testcases", "*"]) {
            let path = self.path();
            self.pending = Some(Pending {
                suite: path[1].to_owned(),
                name: path[3].to_owned(),
                ignored: false,
            });
        }
        self.levels.push(Level::Object {
            key: None,
            wants_key: true,
        });
    }

    fn close_object(&mut self) {
        if !self.path_is(&["rust-suites", "*", "testcases", "*"]) {
            return;
        }
        let Some(pending) = self.pending.take() else {
            return;
        };
        if pending.ignored {
            return;
        }
        self.tests.push(TestItem {
            id: format!("{} {}", pending.suite, pending.name),
            name: pending.name,
            group: pending.suite,
        });
    }
}
